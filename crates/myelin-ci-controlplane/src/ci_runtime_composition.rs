//! Exact-tenant production composition for the durable CI workflow and terminal reporter.
//!
//! The region-wide starter and sandbox runner may discover work across a cell, but Flow workers,
//! manifest resolution, terminal accounting, and CI-run finalization are all tenant-scoped. This
//! module is the one production factory that turns an authoritative tenant plus one durable Flow
//! partition into that complete scope. Construction performs no tenant query; driving the returned
//! worker remains behind the control-plane's refused runner activation seam.

use std::sync::Arc;

use myelin_events::{Actor, MonotonicMinter};
use myelin_flow::{PgFlowExecutor, PgFlowWorker, PgWorkerScope};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{DurableCostLedger, SubstrateProvider, TenantScope};
use myelin_tenancy::{Region, TenantId};

use crate::{
    register_durable_ci_manifest_pipeline, CiCostEventStore, CiDriveManifestStore,
    CiJobAccountingStore, CiJobQueueStore, CiJobSpecStore, CiManifestInputResolver,
    CiPipelineReporter, CiPipelineReporterFactory, CiPipelineReporterFactoryError,
    CiPipelineReporterRouter, CiRunStore, CiWorkflowDefinitionPin, DurableCiJobAccounting,
    DurableCiRunFinalizer, TierPOperationalCiJobPricer,
};

/// Version of the production manifest-native `ci.pipeline` definition.
pub const CI_MANIFEST_PIPELINE_VERSION: i32 = 1;
/// Flow drive lease for a tenant/partition worker.
pub const CI_FLOW_WORKER_LEASE_TTL_SECS: i64 = 60;
/// Schema version stamped on workflow-body outbox facts.
pub const CI_FLOW_OUTBOX_SCHEMA_VERSION: u32 = 1;
const CI_MANIFEST_PIPELINE_DEFINITION_V1_DOMAIN: &str = "myelin.ci.manifest-pipeline-definition.v1";

/// The deployed definition pin, mechanically derived from the exact production workflow source.
///
/// Any source change changes this digest and therefore fails against an already-recorded V1
/// definition until the author deliberately versions the workflow. Starter and worker composition
/// call this same function, so they cannot conventionally drift onto different pins. The complete
/// source files are hashed conservatively, including colocated test-only suffixes, so no later
/// production item can accidentally sit outside the pinned byte range.
pub fn ci_manifest_pipeline_definition() -> CiWorkflowDefinitionPin {
    let mut hasher = blake3::Hasher::new_derive_key(CI_MANIFEST_PIPELINE_DEFINITION_V1_DOMAIN);
    for source in [
        include_bytes!("ci_manifest_pipeline.rs").as_slice(),
        include_bytes!("ci_manifest_job_runner.rs").as_slice(),
    ] {
        hasher.update(&(source.len() as u64).to_be_bytes());
        hasher.update(source);
    }
    let code_hash = format!("blake3:{}", hasher.finalize().to_hex());
    CiWorkflowDefinitionPin::new(CI_MANIFEST_PIPELINE_VERSION, code_hash)
        .expect("the embedded ci.pipeline definition pin is valid")
}

/// Credential-free refusal from exact-tenant runtime composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiRuntimeCompositionError;

impl std::fmt::Display for CiRuntimeCompositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("exact-tenant CI runtime composition refused")
    }
}

impl std::error::Error for CiRuntimeCompositionError {}

/// Production factory for exact-tenant workflow workers and terminal reporter routing.
#[derive(Clone)]
pub struct CiProductionRuntimeFactory {
    pool: sqlx::PgPool,
    region: Region,
    ledger: DurableCostLedger,
    rt: tokio::runtime::Handle,
    definition: CiWorkflowDefinitionPin,
}

/// Compose the dormant production factory from the one validated cell provider.
pub fn ci_production_runtime_factory(
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
) -> Result<CiProductionRuntimeFactory, CiRuntimeCompositionError> {
    let region = Region(provider.config().region.clone());
    let ledger = DurableCostLedger::with_runtime(provider.clone(), rt.clone());
    CiProductionRuntimeFactory::from_parts(provider.db_pool().clone(), region, ledger, rt)
}

impl CiProductionRuntimeFactory {
    fn from_parts(
        pool: sqlx::PgPool,
        region: Region,
        ledger: DurableCostLedger,
        rt: tokio::runtime::Handle,
    ) -> Result<Self, CiRuntimeCompositionError> {
        if !valid_scope_token(&region.0) {
            return Err(CiRuntimeCompositionError);
        }
        Ok(Self {
            pool,
            region,
            ledger,
            rt,
            definition: ci_manifest_pipeline_definition(),
        })
    }

    /// The single region every child scope is pinned to.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The source-derived definition pin shared by starter and worker composition.
    pub fn definition(&self) -> &CiWorkflowDefinitionPin {
        &self.definition
    }

    /// Build and register one exact `(tenant, region, partition)` durable workflow worker.
    ///
    /// The caller must obtain `tenant` from the constrained region discovery capability and
    /// `partition` from Flow's stable `partition_for_run_id` mapping. No default or global worker
    /// exists.
    pub fn worker_for(
        &self,
        tenant: TenantId,
        partition: i16,
        worker_id: impl Into<String>,
    ) -> Result<PgFlowWorker, CiRuntimeCompositionError> {
        if !valid_scope_token(&tenant.0) {
            return Err(CiRuntimeCompositionError);
        }
        let worker_id = worker_id.into();
        if !valid_scope_token(&worker_id) {
            return Err(CiRuntimeCompositionError);
        }
        let principal = service_principal(&tenant, &self.region);
        let scope = TenantScope::from_verified_token(&principal, self.region.clone());
        let manifest =
            CiDriveManifestStore::new(self.pool.clone(), tenant.clone(), self.region.clone())
                .map_err(|_| CiRuntimeCompositionError)?;
        let finalizer = Arc::new(DurableCiRunFinalizer::new(
            CiRunStore::with_pg(self.pool.clone()),
            self.ledger.clone(),
            CiJobAccountingStore::with_pg(self.pool.clone(), self.region.clone()),
            manifest,
            scope,
            self.rt.clone(),
        ));
        let worker_scope = PgWorkerScope::new(
            tenant.clone(),
            self.region.clone(),
            partition,
            worker_id,
            CI_FLOW_WORKER_LEASE_TTL_SECS,
            Actor(principal),
            CI_FLOW_OUTBOX_SCHEMA_VERSION,
        )
        .map_err(|_| CiRuntimeCompositionError)?;
        let mut worker = PgFlowWorker::new(
            self.pool.clone(),
            self.rt.clone(),
            Arc::new(MonotonicMinter::new()),
            worker_scope,
        );
        let resolver = CiManifestInputResolver::new(
            self.pool.clone(),
            tenant,
            self.region.clone(),
            self.definition.clone(),
        )
        .map_err(|_| CiRuntimeCompositionError)?;
        register_durable_ci_manifest_pipeline(
            &mut worker,
            resolver,
            CiJobSpecStore::with_pg(self.pool.clone()),
            finalizer,
            self.rt.clone(),
        )
        .map_err(|_| CiRuntimeCompositionError)?;
        Ok(worker)
    }

    /// Build the production region router whose every reporter is constructed from the claimed
    /// tenant and owns terminal reservation settlement.
    pub fn reporter_router(&self) -> Result<CiPipelineReporterRouter, CiRuntimeCompositionError> {
        let pool = self.pool.clone();
        let bound_region = self.region.clone();
        let ledger = self.ledger.clone();
        let rt = self.rt.clone();
        let factory: CiPipelineReporterFactory = Arc::new(move |tenant, requested_region| {
            if requested_region != &bound_region || !valid_scope_token(&tenant.0) {
                return Err(CiPipelineReporterFactoryError);
            }
            let principal = service_principal(tenant, &bound_region);
            let scope = TenantScope::from_verified_token(&principal, bound_region.clone());
            let manifest =
                CiDriveManifestStore::new(pool.clone(), tenant.clone(), bound_region.clone())
                    .map_err(|_| CiPipelineReporterFactoryError)?;
            let executor = PgFlowExecutor::new(
                pool.clone(),
                rt.clone(),
                Arc::new(MonotonicMinter::new()),
                tenant.clone(),
                bound_region.clone(),
            );
            Ok(CiPipelineReporter::new_accounted(
                executor,
                CiJobSpecStore::with_pg(pool.clone()),
                CiJobQueueStore::with_pg(pool.clone()),
                rt.clone(),
                DurableCiJobAccounting::new(
                    scope,
                    manifest,
                    ledger.clone(),
                    CiCostEventStore::with_pg(pool.clone(), bound_region.clone()),
                    CiJobAccountingStore::with_pg(pool.clone(), bound_region.clone()),
                    Arc::new(TierPOperationalCiJobPricer),
                ),
            ))
        });
        CiPipelineReporterRouter::new(self.region.clone(), factory)
            .map_err(|_| CiRuntimeCompositionError)
    }
}

/// Test-only parts constructor for isolated-schema live integration proofs.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn ci_production_runtime_factory_test_support(
    pool: sqlx::PgPool,
    region: Region,
    ledger: DurableCostLedger,
    rt: tokio::runtime::Handle,
) -> Result<CiProductionRuntimeFactory, CiRuntimeCompositionError> {
    CiProductionRuntimeFactory::from_parts(pool, region, ledger, rt)
}

fn service_principal(tenant: &TenantId, region: &Region) -> Principal {
    Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId("svc:ci-controlplane".into()),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    )
}

fn valid_scope_token(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_definition_pin_is_source_derived_and_stable_within_the_binary() {
        let first = ci_manifest_pipeline_definition();
        let second = ci_manifest_pipeline_definition();
        assert_eq!(first, second);
        assert_eq!(first.version(), CI_MANIFEST_PIPELINE_VERSION);
        assert!(first.code_hash().starts_with("blake3:"));
        assert_eq!(first.code_hash().len(), "blake3:".len() + 64);
    }

    #[test]
    fn scope_tokens_are_canonical_and_bounded() {
        assert!(valid_scope_token("tenant_01"));
        for invalid in ["", " tenant", "tenant ", "tenant/slash", "tenant\nline"] {
            assert!(!valid_scope_token(invalid), "{invalid:?}");
        }
        assert!(!valid_scope_token(&"a".repeat(129)));
    }
}
