use std::sync::Arc;

use myelin_ci_sandbox::{
    CompletionClaim, CompletionSettlementOwner, PreparationReportClaim, PreparationRetryReport,
    PreparationTerminalDisposition, RetryableAttemptFailure, RetryableAttemptOutcome, TerminalReport,
    TerminalReporter,
};
use myelin_flow::{ExecutorError, SignalOutcome};
use myelin_tenancy::{Region, TenantId};

use crate::ci_pipeline_driver::{
    token_request_from_preparation_report_claim, PreparationRetryOutcome,
};
use crate::CiPipelineReporter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiPipelineReporterFactoryError;

impl std::fmt::Display for CiPipelineReporterFactoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("tenant terminal reporter is unavailable")
    }
}

impl std::error::Error for CiPipelineReporterFactoryError {}

pub type CiPipelineReporterFactory = Arc<
    dyn Fn(&TenantId, &Region) -> Result<CiPipelineReporter, CiPipelineReporterFactoryError>
        + Send
        + Sync,
>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiPipelineReporterRouterError;

impl std::fmt::Display for CiPipelineReporterRouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CI terminal reporter router region must be non-empty")
    }
}

impl std::error::Error for CiPipelineReporterRouterError {}

#[derive(Clone)]
pub struct CiPipelineReporterRouter {
    region: Region,
    factory: CiPipelineReporterFactory,
}

impl CiPipelineReporterRouter {
    pub fn new(
        region: Region,
        factory: CiPipelineReporterFactory,
    ) -> Result<Self, CiPipelineReporterRouterError> {
        if region.0.trim().is_empty() {
            return Err(CiPipelineReporterRouterError);
        }
        Ok(Self { region, factory })
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    fn preparation_reporter(&self, tenant_id: &str) -> Result<CiPipelineReporter, ExecutorError> {
        if tenant_id.trim().is_empty() {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline preparation report refused: claimed tenant is empty".into(),
            ));
        }
        let tenant = TenantId(tenant_id.to_string());
        let reporter = (self.factory)(&tenant, &self.region).map_err(|_| {
            ExecutorError::InvalidInput(
                "ci.pipeline preparation report refused: exact-tenant reporter is unavailable".into(),
            )
        })?;
        if reporter.tenant() != &tenant
            || reporter.region() != self.region.0
            || reporter.completion_settlement_owner() != self.completion_settlement_owner()
        {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline preparation report refused: reporter scope or accounting owner mismatch"
                    .into(),
            ));
        }
        Ok(reporter)
    }
}

impl TerminalReporter for CiPipelineReporterRouter {
    fn completion_settlement_owner(&self) -> CompletionSettlementOwner {
        CompletionSettlementOwner::TerminalReporter
    }

    fn report_done(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, ExecutorError> {
        if claim.tenant.0.trim().is_empty() {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline job.done refused: claimed tenant is empty".into(),
            ));
        }
        let reporter = (self.factory)(&claim.tenant, &self.region).map_err(|_| {
            ExecutorError::InvalidInput(
                "ci.pipeline job.done refused: exact-tenant reporter is unavailable".into(),
            )
        })?;
        if reporter.tenant() != &claim.tenant || reporter.region() != self.region.0 {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline job.done refused: reporter factory returned a mismatched scope".into(),
            ));
        }
        if reporter.completion_settlement_owner() != self.completion_settlement_owner() {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline job.done refused: reporter factory returned an unaccounted reporter"
                    .into(),
            ));
        }
        reporter.report_done(claim, report)
    }

    fn report_retryable_attempt(
        &self,
        claim: &CompletionClaim,
        failure: &RetryableAttemptFailure,
    ) -> Result<RetryableAttemptOutcome, ExecutorError> {
        if claim.tenant.0.trim().is_empty() {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline retryable attempt refused: claimed tenant is empty".into(),
            ));
        }
        let reporter = (self.factory)(&claim.tenant, &self.region).map_err(|_| {
            ExecutorError::InvalidInput(
                "ci.pipeline retryable attempt refused: exact-tenant reporter is unavailable"
                    .into(),
            )
        })?;
        if reporter.tenant() != &claim.tenant
            || reporter.region() != self.region.0
            || reporter.completion_settlement_owner() != self.completion_settlement_owner()
        {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline retryable attempt refused: reporter scope or accounting owner mismatch"
                    .into(),
            ));
        }
        reporter.report_retryable_attempt(claim, failure)
    }

    fn report_preparation_terminal(
        &self,
        claim: &PreparationReportClaim,
        disposition: PreparationTerminalDisposition,
        diagnostic: Option<&str>,
    ) -> Result<SignalOutcome, ExecutorError> {
        let reporter = self.preparation_reporter(&claim.tenant_id)?;
        CiPipelineReporter::report_preparation_terminal_with_diagnostic(
            &reporter,
            &token_request_from_preparation_report_claim(claim),
            disposition,
            diagnostic,
        )
    }

    fn report_preparation_retry(
        &self,
        claim: &PreparationReportClaim,
    ) -> Result<PreparationRetryReport, ExecutorError> {
        let reporter = self.preparation_reporter(&claim.tenant_id)?;
        match CiPipelineReporter::report_preparation_retry(
            &reporter,
            &token_request_from_preparation_report_claim(claim),
        )? {
            PreparationRetryOutcome::Requeued => Ok(PreparationRetryReport::Requeued),
            PreparationRetryOutcome::NoOp => Ok(PreparationRetryReport::NoOp),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use myelin_ci_sandbox::ResourceUsage;
    use myelin_events::MonotonicMinter;
    use myelin_flow::{PgFlowExecutor, RunId};
    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::{CiJobQueueStore, CiJobSpecStore};

    fn lazy_reporter(tenant: &TenantId, region: &Region) -> CiPipelineReporter {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .expect("syntactically valid lazy pool");
        CiPipelineReporter::new(
            PgFlowExecutor::new(
                pool.clone(),
                tokio::runtime::Handle::current(),
                Arc::new(MonotonicMinter::new()),
                tenant.clone(),
                region.clone(),
            ),
            CiJobSpecStore::with_pg(pool.clone()),
            CiJobQueueStore::with_pg(pool),
            tokio::runtime::Handle::current(),
            tenant.clone(),
            region.0.clone(),
        )
    }

    fn claim(tenant: &str) -> CompletionClaim {
        CompletionClaim {
            tenant: TenantId(tenant.into()),
            run: RunId("run-1".into()),
            job_id: "00000000-0000-0000-0000-000000000001".into(),
            idem_token: "idem-1".into(),
            lease_owner: "worker-1".into(),
            lease_epoch: 1,
            claim_nonce: "10000000-0000-0000-0000-000000000001".into(),
        }
    }

    fn contradictory_report() -> TerminalReport {
        TerminalReport {
            passed: true,
            timed_out: true,
            usage: ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            },
            result_refs: Vec::new(),
        }
    }

    #[test]
    fn empty_router_region_is_rejected() {
        let factory: CiPipelineReporterFactory =
            Arc::new(|_, _| Err(CiPipelineReporterFactoryError));
        assert!(CiPipelineReporterRouter::new(Region(String::new()), factory).is_err());
    }

    #[tokio::test]
    async fn routes_each_claim_to_its_exact_tenant_and_region_and_refuses_test_bypass() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed = calls.clone();
        let factory: CiPipelineReporterFactory = Arc::new(move |tenant, region| {
            observed
                .lock()
                .expect("call recorder")
                .push((tenant.clone(), region.clone()));
            Ok(lazy_reporter(tenant, region))
        });
        let router = CiPipelineReporterRouter::new(Region("fr-par".into()), factory).unwrap();
        for tenant in ["tenant-a", "tenant-b"] {
            let error = router
                .report_done(&claim(tenant), &contradictory_report())
                .expect_err("the router must reject a test-bypass reporter before database access");
            assert!(
                error.to_string().contains("unaccounted reporter"),
                "{error}"
            );
        }
        assert_eq!(
            *calls.lock().expect("call recorder"),
            vec![
                (TenantId("tenant-a".into()), Region("fr-par".into())),
                (TenantId("tenant-b".into()), Region("fr-par".into())),
            ]
        );
    }

    #[tokio::test]
    async fn mismatched_factory_scope_is_refused_before_reporter_database_access() {
        let factory: CiPipelineReporterFactory =
            Arc::new(|_, region| Ok(lazy_reporter(&TenantId("wrong-tenant".into()), region)));
        let router = CiPipelineReporterRouter::new(Region("fr-par".into()), factory).unwrap();
        let error = router
            .report_done(&claim("tenant-a"), &contradictory_report())
            .expect_err("mis-routed reporter is refused");
        assert!(error.to_string().contains("mismatched scope"));
    }
}
