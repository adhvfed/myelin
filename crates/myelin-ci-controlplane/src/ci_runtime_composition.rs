//! Exact-tenant production composition for the durable CI workflow and terminal reporter.
//!
//! The region-wide starter and sandbox runner may discover work across a cell, but Flow workers,
//! manifest resolution, terminal accounting, and CI-run finalization are all tenant-scoped. This
//! module is the one production factory that turns an authoritative tenant plus one durable Flow
//! partition into that complete scope. Construction performs no tenant query; driving the returned
//! worker remains behind the control-plane's refused runner activation seam.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use myelin_events::{Actor, MonotonicMinter};
use myelin_flow::{PgFlowExecutor, PgFlowWorker, PgWorkerScope};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{DurableCostLedger, SubstrateProvider, TenantScope};
use myelin_tenancy::{Region, TenantId};

use crate::{
    register_durable_ci_manifest_pipeline, CiActiveRunCursor, CiCostEventStore,
    CiDriveManifestStore, CiJobAccountingStore, CiJobQueueStore, CiJobSpecStore,
    CiManifestInputResolver, CiPipelineReporter, CiPipelineReporterFactory,
    CiPipelineReporterFactoryError, CiPipelineReporterRouter, CiRegionRunDiscovery, CiRunStore,
    CiWorkflowDefinitionPin, DurableCiJobAccounting, DurableCiRunFinalizer,
    TierPOperationalCiJobPricer, MAX_ACTIVE_CI_RUN_PAGE,
};

/// Version of the production manifest-native `ci.pipeline` definition.
pub const CI_MANIFEST_PIPELINE_VERSION: i32 = 1;
/// Flow drive lease for a tenant/partition worker.
pub const CI_FLOW_WORKER_LEASE_TTL_SECS: i64 = 60;
/// Schema version stamped on workflow-body outbox facts.
pub const CI_FLOW_OUTBOX_SCHEMA_VERSION: u32 = 1;
/// Maximum exact tenant/partition scopes one recovery pass may construct.
pub const MAX_CI_WORKFLOW_SCOPES_PER_PASS: usize = MAX_ACTIVE_CI_RUN_PAGE;
/// Maximum workflow drives one exact scope may perform before yielding.
pub const MAX_CI_WORKFLOW_DRIVES_PER_SCOPE: usize = 64;
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

/// Result of one bounded active-run recovery/fan-out pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CiWorkflowFanoutBatch {
    pub discovered: usize,
    pub scopes: usize,
    pub driven: usize,
    pub saturated: bool,
}

/// Keyset-cycling, bounded router from region-active CI rows to exact Flow workers.
pub struct CiProductionWorkflowPoller {
    discovery: CiRegionRunDiscovery,
    runtime: CiProductionRuntimeFactory,
    worker_prefix: String,
    cursor: Option<CiActiveRunCursor>,
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

    /// Bind restart-safe region discovery to this exact-cell worker factory.
    pub fn workflow_poller(
        &self,
        discovery: CiRegionRunDiscovery,
        worker_prefix: impl Into<String>,
    ) -> Result<CiProductionWorkflowPoller, CiRuntimeCompositionError> {
        let worker_prefix = worker_prefix.into();
        if !valid_scope_token(&worker_prefix) {
            return Err(CiRuntimeCompositionError);
        }
        Ok(CiProductionWorkflowPoller {
            discovery,
            runtime: self.clone(),
            worker_prefix,
            cursor: None,
        })
    }

    /// Build and register one exact `(tenant, region, partition)` durable workflow worker.
    ///
    /// The caller must obtain `tenant` and the persisted `partition` from the constrained region
    /// discovery capability. No default or global worker exists.
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

impl CiProductionWorkflowPoller {
    /// Drive one keyset page. A short page wraps the next pass to the beginning; a full page advances
    /// from its last durable `(created_at, tenant_id, run_id)` key, so a large active set cannot pin
    /// the oldest scope forever.
    pub async fn run_once(
        &mut self,
        max_scopes: usize,
        max_drives_per_scope: usize,
        now_unix_secs: i64,
        now_rfc3339: &str,
    ) -> Result<CiWorkflowFanoutBatch, CiRuntimeCompositionError> {
        self.run_once_inner(
            max_scopes,
            max_drives_per_scope,
            now_unix_secs,
            now_rfc3339,
            None,
        )
        .await
    }

    async fn run_once_or_shutdown(
        &mut self,
        max_scopes: usize,
        max_drives_per_scope: usize,
        now_unix_secs: i64,
        now_rfc3339: &str,
        shutdown: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<CiWorkflowFanoutBatch, CiRuntimeCompositionError> {
        self.run_once_inner(
            max_scopes,
            max_drives_per_scope,
            now_unix_secs,
            now_rfc3339,
            Some(shutdown),
        )
        .await
    }

    async fn run_once_inner(
        &mut self,
        max_scopes: usize,
        max_drives_per_scope: usize,
        now_unix_secs: i64,
        now_rfc3339: &str,
        shutdown: Option<&tokio::sync::watch::Receiver<bool>>,
    ) -> Result<CiWorkflowFanoutBatch, CiRuntimeCompositionError> {
        if !(1..=MAX_CI_WORKFLOW_SCOPES_PER_PASS).contains(&max_scopes)
            || !(1..=MAX_CI_WORKFLOW_DRIVES_PER_SCOPE).contains(&max_drives_per_scope)
        {
            return Err(CiRuntimeCompositionError);
        }
        let mut page = self
            .discovery
            .active_run_page(&self.runtime.region.0, self.cursor.as_ref(), max_scopes)
            .await
            .map_err(|_| CiRuntimeCompositionError)?;
        if page.routes.is_empty() && self.cursor.is_some() {
            self.cursor = None;
            page = self
                .discovery
                .active_run_page(&self.runtime.region.0, None, max_scopes)
                .await
                .map_err(|_| CiRuntimeCompositionError)?;
        }
        let discovered = page.routes.len();
        self.cursor = if discovered == max_scopes {
            page.next_cursor.clone()
        } else {
            None
        };

        let mut seen = BTreeSet::new();
        let mut scopes = 0usize;
        let mut driven = 0usize;
        let mut saturated = discovered == max_scopes;
        for route in page.routes {
            if shutdown.is_some_and(|receiver| *receiver.borrow()) {
                saturated = true;
                break;
            }
            let partition = route.partition;
            if !seen.insert((route.tenant.0.clone(), partition)) {
                continue;
            }
            let worker_id = scoped_worker_id(&self.worker_prefix, &route.tenant, partition);
            let worker = self
                .runtime
                .worker_for(route.tenant, partition, worker_id)?;
            let batch = match shutdown {
                Some(receiver) => {
                    worker
                        .run_until_idle_or_shutdown(
                            max_drives_per_scope,
                            now_unix_secs,
                            now_rfc3339,
                            receiver,
                        )
                        .await
                }
                None => {
                    worker
                        .run_until_idle(max_drives_per_scope, now_unix_secs, now_rfc3339)
                        .await
                }
            }
            .map_err(|_| CiRuntimeCompositionError)?;
            scopes += 1;
            driven = driven.saturating_add(batch.driven);
            saturated |= batch.saturated;
        }
        Ok(CiWorkflowFanoutBatch {
            discovered,
            scopes,
            driven,
            saturated,
        })
    }

    /// Run bounded recovery passes until explicit shutdown or sender closure. Wall-clock values are
    /// sampled once per pass and supplied to Flow's deterministic lease/timestamp boundary.
    pub async fn run_until_shutdown(
        mut self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
        poll_interval: Duration,
        max_scopes: usize,
        max_drives_per_scope: usize,
    ) -> Result<(), CiRuntimeCompositionError> {
        if poll_interval.is_zero()
            || !(1..=MAX_CI_WORKFLOW_SCOPES_PER_PASS).contains(&max_scopes)
            || !(1..=MAX_CI_WORKFLOW_DRIVES_PER_SCOPE).contains(&max_drives_per_scope)
        {
            return Err(CiRuntimeCompositionError);
        }
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let now = Utc::now();
            let now_rfc3339 = now.to_rfc3339_opts(SecondsFormat::Secs, true);
            self.run_once_or_shutdown(
                max_scopes,
                max_drives_per_scope,
                now.timestamp(),
                &now_rfc3339,
                &shutdown,
            )
            .await?;
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(poll_interval) => {}
            }
        }
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

fn scoped_worker_id(prefix: &str, tenant: &TenantId, partition: i16) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("myelin.ci.flow-worker-id.v1");
    hasher.update(&(tenant.0.len() as u64).to_be_bytes());
    hasher.update(tenant.0.as_bytes());
    let tenant_hash = hasher.finalize().to_hex();
    format!("{prefix}-{}-{partition}", &tenant_hash[..16])
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

    #[test]
    fn worker_ids_are_bounded_stable_and_tenant_distinct() {
        let one = scoped_worker_id("ci-flow", &TenantId("tenant-a".into()), 7);
        assert_eq!(
            one,
            scoped_worker_id("ci-flow", &TenantId("tenant-a".into()), 7)
        );
        assert_ne!(
            one,
            scoped_worker_id("ci-flow", &TenantId("tenant-b".into()), 7)
        );
        assert!(valid_scope_token(&one));
    }
}
