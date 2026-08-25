use std::sync::Arc;

use myelin_ci_sandbox::{
    CompletionClaim, CompletionSettlementOwner, PreparationReportClaim, PreparationRetryReport,
    PreparationTerminalDisposition, ResourceUsage, RetryableAttemptFailure,
    RetryableAttemptOutcome, TerminalReport, TerminalReporter,
};
#[cfg(any(test, feature = "test-support"))]
use myelin_flow::{DurableExecutor, FlowExecutor};
use myelin_flow::{
    ExecutorError, PgFlowExecutor, SignalOutcome, SignalPayload, TypedSignalSpec, JOB_DONE_SIGNAL,
};
use myelin_storage::with_tenant_tx_error;
use myelin_tenancy::TenantId;
use sqlx::types::Uuid;

use crate::ci_manifest_job_runner::CiJobTokenRequest;
use crate::ci_prelaunch_usage_journal::{
    CiPrelaunchParentExpectation, CiPrelaunchUnresolvedPolicy,
};
use crate::job_accounting_store::{
    disposition_receipt_v4, CiJobAccountingWriteVersion, CiJobTerminalDisposition,
};
use crate::job_queue_store::{CiJobQueueStore, ClaimConsumeOutcome, ClaimConsumeSpec};
use crate::job_spec_store::CiJobSpecStore;

use super::accounting::ReporterAccounting;
use super::completion::{
    completion_receipts_v4, verify_claimed_identity, workload_disposition, ClaimRefusal,
    CompletionReceiptInput, CompletionReceipts,
};
use super::retry::{
    aggregate_usage, record_retryable_attempt_on_conn, retry_attempts_for_terminal_on_conn,
};
use super::{
    bridge, close_cancelled_run_if_accounted, co_commit_terminal_accounting,
    resolve_terminal_usage_on_conn, CompletionTxError, DurableCiJobAccounting,
    TerminalAccountingInput, TerminalUsageResolutionInput,
};

mod preparation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationRetryOutcome {
    Requeued,
    NoOp,
}

#[derive(Clone)]
pub struct CiPipelineReporter {
    pg_executor: PgFlowExecutor,
    spec_store: CiJobSpecStore,
    queue_store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
    tenant: TenantId,
    region: String,
    accounting: ReporterAccounting,
    #[cfg(any(test, feature = "test-support"))]
    test_executor: Option<FlowExecutor>,
}

impl CiPipelineReporter {
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn new_accounted(
        pg_executor: PgFlowExecutor,
        spec_store: CiJobSpecStore,
        queue_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
        accounting: DurableCiJobAccounting,
    ) -> CiPipelineReporter {
        let tenant = accounting.scope.tenant().clone();
        let region = accounting.scope.region().as_str().to_owned();
        CiPipelineReporter {
            pg_executor,
            spec_store,
            queue_store,
            rt,
            tenant,
            region,
            accounting: ReporterAccounting::Durable(Arc::new(accounting)),
            #[cfg(any(test, feature = "test-support"))]
            test_executor: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        pg_executor: PgFlowExecutor,
        spec_store: CiJobSpecStore,
        queue_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
        tenant: TenantId,
        region: impl Into<String>,
    ) -> CiPipelineReporter {
        CiPipelineReporter {
            pg_executor,
            spec_store,
            queue_store,
            rt,
            tenant,
            region: region.into(),
            accounting: ReporterAccounting::TestBypass,
            #[cfg(any(test, feature = "test-support"))]
            test_executor: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn with_test_executor(mut self, executor: FlowExecutor) -> Self {
        self.test_executor = Some(executor);
        self
    }
}

pub(crate) fn token_request_from_preparation_report_claim(
    claim: &PreparationReportClaim,
) -> CiJobTokenRequest {
    CiJobTokenRequest {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        project_id: claim.project_id.clone(),
        wf_run_id: claim.wf_run_id.clone(),
        ci_run_id: claim.ci_run_id.clone(),
        job_id: claim.job_id.clone(),
        token_authority_handle: claim.token_authority_handle.clone(),
        idem_token: claim.idem_token.clone(),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
    }
}

impl TerminalReporter for CiPipelineReporter {
    fn completion_settlement_owner(&self) -> CompletionSettlementOwner {
        match self.accounting {
            ReporterAccounting::Durable(_) => CompletionSettlementOwner::TerminalReporter,
            #[cfg(any(test, feature = "test-support"))]
            ReporterAccounting::TestBypass => CompletionSettlementOwner::Hook,
        }
    }

    fn report_done(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, ExecutorError> {
        let CompletionClaim {
            tenant,
            run,
            job_id,
            idem_token,
            lease_owner,
            lease_epoch,
            claim_nonce,
        } = claim;
        let lease_epoch = *lease_epoch;
        if tenant != &self.tenant {
            return Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline job.done refused (unverified claim, fail-closed): {}",
                ClaimRefusal::TenantMismatch {
                    reporter: self.tenant.0.clone(),
                    claimed: tenant.0.clone(),
                }
            )));
        }
        if report.passed && report.timed_out {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline job.done refused: a timed-out job cannot pass".into(),
            ));
        }

        let job_uuid = Uuid::parse_str(job_id)
            .map_err(|_| ExecutorError::InvalidInput(format!("invalid job_id UUID `{job_id}`")))?;
        let nonce_uuid = Uuid::parse_str(claim_nonce).map_err(|_| {
            ExecutorError::InvalidInput("invalid claim_nonce UUID (completion refused)".into())
        })?;
        let tenant_owned = self.tenant.0.clone();
        let region_owned = self.region.clone();
        let run_owned = run.clone();
        let job_owned = job_id.to_string();
        let idem_owned = idem_token.to_string();
        let owner_owned = lease_owner.to_string();
        let nonce_owned = claim_nonce.to_string();
        let report_owned = report.clone();
        let spec_store = self.spec_store.clone();
        let pg_executor = self.pg_executor.clone();
        let accounting = self.accounting.clone();

        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &self.tenant.0,
                &self.region,
                move |conn| {
                    Box::pin(async move {
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                &tenant_owned,
                                job_uuid,
                                &job_owned,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        let stage = verify_claimed_identity(
                            &TenantId(tenant_owned.clone()),
                            &TenantId(tenant_owned.clone()),
                            &run_owned.0,
                            &job_owned,
                            &idem_owned,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let signal = TypedSignalSpec {
                            run: run_owned.clone(),
                            signal_name: JOB_DONE_SIGNAL.to_string(),
                            idem_key: idem_owned.clone(),
                            payload: SignalPayload::CiJobDone {
                                stage: stage.clone(),
                                passed: report_owned.passed,
                                result_refs: report_owned.result_refs.clone(),
                            },
                            payload_key_ref: None,
                        };
                        let outcome = pg_executor
                            .signal_typed_on_conn(conn, signal.clone())
                            .await
                            .map_err(CompletionTxError::Signal)?;
                        let attempts = retry_attempts_for_terminal_on_conn(
                            conn,
                            &TenantId(tenant_owned.clone()),
                            &region_owned,
                            job_uuid,
                        )
                        .await?;
                        let mut accounted_report = report_owned.clone();
                        accounted_report.usage =
                            aggregate_usage(attempts.as_ref(), report_owned.usage)?;
                        let (ci_run_id, usage) = match &accounting {
                            ReporterAccounting::Durable(accounting) => {
                                resolve_terminal_usage_on_conn(
                                    conn,
                                    accounting,
                                    TerminalUsageResolutionInput {
                                        tenant: &TenantId(tenant_owned.clone()),
                                        wf_run_id: &run_owned.0,
                                        job_id: &job_owned,
                                        reserve_handle: &reserve_handle,
                                        base_usage: accounted_report.usage,
                                        parent_expectation: CiPrelaunchParentExpectation::Required,
                                        unresolved_policy: CiPrelaunchUnresolvedPolicy::Refuse,
                                    },
                                )
                                .await?
                            }
                            #[cfg(any(test, feature = "test-support"))]
                            ReporterAccounting::TestBypass => {
                                (String::new(), accounted_report.usage)
                            }
                        };
                        accounted_report.usage = usage;
                        let disposition = workload_disposition(&accounted_report);
                        let receipts = completion_receipts_v4(
                            CompletionReceiptInput {
                                tenant: &TenantId(tenant_owned.clone()),
                                region: &region_owned,
                                run: &run_owned,
                                job_id: &job_owned,
                                idem_token: &idem_owned,
                                stage: &stage,
                                passed: accounted_report.passed,
                                timed_out: accounted_report.timed_out,
                                usage: accounted_report.usage,
                                result_refs: &accounted_report.result_refs,
                                lease_owner: &owner_owned,
                                lease_epoch,
                                claim_nonce: &nonce_owned,
                            },
                            disposition,
                        );
                        let write_version = match &accounting {
                            ReporterAccounting::Durable(accounting) => {
                                accounting.receipt_store.write_version()
                            }
                            #[cfg(any(test, feature = "test-support"))]
                            ReporterAccounting::TestBypass => CiJobAccountingWriteVersion::V3,
                        };
                        let (completion_receipt, alternate_replay_receipt) = match write_version {
                            CiJobAccountingWriteVersion::V3 => (
                                receipts.legacy_v3.as_str(),
                                Some(receipts.current_v4.as_str()),
                            ),
                            CiJobAccountingWriteVersion::V4 => (
                                receipts.current_v4.as_str(),
                                Some(receipts.legacy_v3.as_str()),
                            ),
                        };
                        let claim = CiJobQueueStore::consume_claim_on_conn(
                            conn,
                            ClaimConsumeSpec {
                                tenant_id: &tenant_owned,
                                job_id: job_uuid,
                                lease_owner: &owner_owned,
                                lease_epoch,
                                claim_nonce: nonce_uuid,
                                stage: &stage,
                                completion_receipt,
                                alternate_replay_receipt,
                            },
                        )
                        .await?;
                        if claim == ClaimConsumeOutcome::Refused {
                            return Err(CompletionTxError::Refused);
                        }
                        match &accounting {
                            ReporterAccounting::Durable(accounting) => {
                                co_commit_terminal_accounting(
                                    conn,
                                    accounting,
                                    TerminalAccountingInput {
                                        tenant: &TenantId(tenant_owned.clone()),
                                        wf_run: &run_owned,
                                        ci_run_id: &ci_run_id,
                                        job_id: &job_owned,
                                        reserve_handle: &reserve_handle,
                                        report: &accounted_report,
                                        receipts: &receipts,
                                        disposition,
                                        diagnostic: None,
                                        replay: claim == ClaimConsumeOutcome::AlreadyConsumed,
                                    },
                                )
                                .await?;
                                close_cancelled_run_if_accounted(conn, accounting, &run_owned.0)
                                    .await?;
                            }
                            #[cfg(any(test, feature = "test-support"))]
                            ReporterAccounting::TestBypass => {}
                        }
                        Ok((outcome, signal))
                    })
                },
            ),
        );
        let (outcome, signal) = match durable {
            Ok(value) => value,
            Err(CompletionTxError::Refused) => {
                return Err(ExecutorError::InvalidInput(format!(
                    "ci.pipeline job.done refused (unverified, stale, or divergent claim): job \
                     `{job_id}` owner `{lease_owner}` epoch `{lease_epoch}`"
                )))
            }
            Err(CompletionTxError::Spec(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "durable claimed-job read refused: {error}"
                )))
            }
            Err(CompletionTxError::Manifest) => {
                return Err(ExecutorError::Storage(
                    "durable CI launch authority could not be verified".into(),
                ))
            }
            Err(CompletionTxError::Pricing(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI accounting refused: {error}"
                )))
            }
            Err(CompletionTxError::Money(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI money settlement refused: {error}"
                )))
            }
            Err(CompletionTxError::Projection(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI cost projection refused: {error}"
                )))
            }
            Err(CompletionTxError::Accounting(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI accounting receipt refused: {error}"
                )))
            }
            Err(CompletionTxError::CancelledClosure) => {
                return Err(ExecutorError::Storage(
                    "cancelled CI run accounting closure was refused".into(),
                ))
            }
            Err(CompletionTxError::Signal(error)) => return Err(error),
            Err(CompletionTxError::RetryStore) => {
                return Err(ExecutorError::Storage(
                    "durable retry-attempt store failed".into(),
                ))
            }
            Err(CompletionTxError::RetryCorrupt) => {
                return Err(ExecutorError::Storage(
                    "durable retry-attempt state is corrupt".into(),
                ))
            }
            Err(CompletionTxError::Prelaunch(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI prelaunch usage resolution refused: {error}"
                )))
            }
            Err(CompletionTxError::Usage(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI usage aggregation refused: {error}"
                )))
            }
            Err(CompletionTxError::Scope(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "atomic completion transaction failed: {error}"
                )))
            }
        };

        #[cfg(any(test, feature = "test-support"))]
        if let Some(executor) = &self.test_executor {
            executor.signal_typed(signal)?;
            executor.runs().wake(&self.tenant, &run.0);
        }
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = signal;
        Ok(outcome)
    }

    fn report_retryable_attempt(
        &self,
        claim: &CompletionClaim,
        failure: &RetryableAttemptFailure,
    ) -> Result<RetryableAttemptOutcome, ExecutorError> {
        if claim.tenant != self.tenant {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline retryable attempt refused: reporter tenant mismatch".into(),
            ));
        }
        let job_uuid = Uuid::parse_str(&claim.job_id).map_err(|_| {
            ExecutorError::InvalidInput("invalid job_id UUID in retryable attempt".into())
        })?;
        Uuid::parse_str(&claim.claim_nonce).map_err(|_| {
            ExecutorError::InvalidInput("invalid claim_nonce UUID in retryable attempt".into())
        })?;
        let tenant_owned = self.tenant.0.clone();
        let region_owned = self.region.clone();
        let claim_owned = claim.clone();
        let failure_owned = *failure;
        let spec_store = self.spec_store.clone();
        let accounting = self.accounting.clone();
        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &self.tenant.0,
                &self.region,
                move |conn| {
                    Box::pin(async move {
                        let flow_state: String = sqlx::query_scalar(
                            "SELECT state FROM workflow_run
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3
                             FOR UPDATE",
                        )
                        .bind(&tenant_owned)
                        .bind(&region_owned)
                        .bind(&claim_owned.run.0)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|_| CompletionTxError::RetryStore)?
                        .ok_or(CompletionTxError::Refused)?;
                        let requeue = matches!(flow_state.as_str(), "running" | "waiting");
                        let cancelled_ci_run = if !requeue {
                            if flow_state != "terminated" {
                                return Err(CompletionTxError::Refused);
                            }
                            let ci_run: Option<(String, String)> = sqlx::query_as(
                                "SELECT run_id::text, state FROM ci_run
                                 WHERE tenant_id = $1 AND region = $2 AND wf_run_id = $3::uuid",
                            )
                            .bind(&tenant_owned)
                            .bind(&region_owned)
                            .bind(&claim_owned.run.0)
                            .fetch_optional(&mut *conn)
                            .await
                            .map_err(|_| CompletionTxError::RetryStore)?;
                            let (ci_run_id, ci_state) = ci_run.ok_or(CompletionTxError::Refused)?;
                            if ci_state != "cancelled" {
                                return Err(CompletionTxError::Refused);
                            }
                            Some(ci_run_id)
                        } else {
                            None
                        };
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                &tenant_owned,
                                job_uuid,
                                &claim_owned.job_id,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        verify_claimed_identity(
                            &TenantId(tenant_owned.clone()),
                            &claim_owned.tenant,
                            &claim_owned.run.0,
                            &claim_owned.job_id,
                            &claim_owned.idem_token,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let outcome = record_retryable_attempt_on_conn(
                            conn,
                            &region_owned,
                            &claim_owned,
                            &failure_owned,
                            requeue,
                        )
                        .await?;
                        if !requeue {
                            let attempts = retry_attempts_for_terminal_on_conn(
                                conn,
                                &TenantId(tenant_owned.clone()),
                                &region_owned,
                                job_uuid,
                            )
                            .await?
                            .ok_or(CompletionTxError::RetryCorrupt)?;
                            let report = TerminalReport {
                                passed: false,
                                timed_out: false,
                                usage: ResourceUsage {
                                    cpu_seconds: attempts.cpu_seconds,
                                    mem_byte_seconds: attempts.mem_byte_seconds,
                                },
                                result_refs: Vec::new(),
                            };
                            match &accounting {
                                ReporterAccounting::Durable(accounting) => {
                                    let (ci_run_id, usage) = resolve_terminal_usage_on_conn(
                                        conn,
                                        accounting,
                                        TerminalUsageResolutionInput {
                                            tenant: &TenantId(tenant_owned.clone()),
                                            wf_run_id: &claim_owned.run.0,
                                            job_id: &claim_owned.job_id,
                                            reserve_handle: &reserve_handle,
                                            base_usage: report.usage,
                                            parent_expectation:
                                                CiPrelaunchParentExpectation::Required,
                                            unresolved_policy: CiPrelaunchUnresolvedPolicy::Refuse,
                                        },
                                    )
                                    .await?;
                                    let report = TerminalReport { usage, ..report };
                                    let legacy_v3 = crate::ci_run_supersession::superseded_receipt(
                                        &accounting.scope,
                                        cancelled_ci_run
                                            .as_deref()
                                            .ok_or(CompletionTxError::Refused)?,
                                        &claim_owned.run.0,
                                        &claim_owned.job_id,
                                        &reserve_handle,
                                        report.usage,
                                        false,
                                    );
                                    let disposition =
                                        CiJobTerminalDisposition::CancelledAfterWorkloadLaunch;
                                    let receipts = CompletionReceipts {
                                        current_v4: disposition_receipt_v4(&legacy_v3, disposition),
                                        legacy_v3,
                                    };
                                    co_commit_terminal_accounting(
                                        conn,
                                        accounting,
                                        TerminalAccountingInput {
                                            tenant: &TenantId(tenant_owned.clone()),
                                            wf_run: &claim_owned.run,
                                            ci_run_id: &ci_run_id,
                                            job_id: &claim_owned.job_id,
                                            reserve_handle: &reserve_handle,
                                            report: &report,
                                            receipts: &receipts,
                                            disposition,
                                            diagnostic: None,
                                            replay: outcome == RetryableAttemptOutcome::ExactReplay,
                                        },
                                    )
                                    .await?;
                                    close_cancelled_run_if_accounted(
                                        conn,
                                        accounting,
                                        &claim_owned.run.0,
                                    )
                                    .await?;
                                }
                                #[cfg(any(test, feature = "test-support"))]
                                ReporterAccounting::TestBypass => {
                                    return Err(CompletionTxError::Refused);
                                }
                            }
                        }
                        Ok(outcome)
                    })
                },
            ),
        );
        match durable {
            Ok(outcome) => Ok(outcome),
            Err(CompletionTxError::Refused) => Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline retryable attempt refused (unverified, stale, or divergent claim): \
                 job `{}` owner `{}` epoch `{}`",
                claim.job_id, claim.lease_owner, claim.lease_epoch
            ))),
            Err(CompletionTxError::Spec(error)) => Err(ExecutorError::Storage(format!(
                "durable retryable-attempt dispatch read refused: {error}"
            ))),
            Err(CompletionTxError::Scope(error)) => Err(ExecutorError::Storage(format!(
                "atomic retryable-attempt transaction failed: {error}"
            ))),
            Err(CompletionTxError::RetryStore) => Err(ExecutorError::Storage(
                "durable retry-attempt store failed".into(),
            )),
            Err(CompletionTxError::RetryCorrupt) => Err(ExecutorError::Storage(
                "durable retry-attempt state is corrupt".into(),
            )),
            Err(CompletionTxError::Prelaunch(error)) => Err(ExecutorError::Storage(format!(
                "terminal retry prelaunch usage resolution refused: {error}"
            ))),
            Err(CompletionTxError::Usage(error)) => Err(ExecutorError::Storage(format!(
                "terminal retry usage aggregation refused: {error}"
            ))),
            Err(_) => Err(ExecutorError::Storage(
                "retryable-attempt transaction reached an invalid accounting path".into(),
            )),
        }
    }

    fn report_preparation_terminal(
        &self,
        claim: &PreparationReportClaim,
        disposition: PreparationTerminalDisposition,
        diagnostic: Option<&str>,
    ) -> Result<SignalOutcome, ExecutorError> {
        CiPipelineReporter::report_preparation_terminal_with_diagnostic(
            self,
            &token_request_from_preparation_report_claim(claim),
            disposition,
            diagnostic,
        )
    }

    fn report_preparation_retry(
        &self,
        claim: &PreparationReportClaim,
    ) -> Result<PreparationRetryReport, ExecutorError> {
        match CiPipelineReporter::report_preparation_retry(
            self,
            &token_request_from_preparation_report_claim(claim),
        )? {
            PreparationRetryOutcome::Requeued => Ok(PreparationRetryReport::Requeued),
            PreparationRetryOutcome::NoOp => Ok(PreparationRetryReport::NoOp),
        }
    }
}
