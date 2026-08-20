use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use myelin_events::{Actor, MonotonicMinter};
use myelin_flow::{PgFlowExecutor, PgFlowWorker, PgWorkerScope, CI_PIPELINE_WF_TYPE};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{DurableCostLedger, SubstrateProvider, TenantScope};
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

use crate::{
    register_durable_ci_manifest_pipeline, CiActiveRunCursor, CiCostEventStore,
    CiDriveManifestStore, CiJobAccountingStore, CiJobQueueStore, CiJobSpecStore,
    CiManifestInputResolver, CiPipelineReporter, CiPipelineReporterFactory,
    CiPipelineReporterFactoryError, CiPipelineReporterRouter, CiRegionRunDiscovery, CiRunStore,
    CiWorkflowDefinitionPin, DurableCiJobAccounting, DurableCiRunFinalizer,
    TierPOperationalCiJobPricer, MAX_ACTIVE_CI_RUN_PAGE, MAX_SUPERSEDED_CI_PIPELINE_RUN_PROBE,
    MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT,
};

pub const CI_MANIFEST_PIPELINE_VERSION: i32 = 7;

pub const CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION: i32 = 6;

pub const CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS: u64 = 10_000;

const CI_PIPELINE_BACKLOG_PROBE_CALL: &str = "\
SELECT myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs($1) \
/* global registry fence: database-wide by construction */";

const CI_V2_ACTIVATION_READINESS_PROBE_CALL: &str = "\
SELECT myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count() \
/* global queue-safety fence: database-wide by construction */";
pub const CI_FLOW_WORKER_LEASE_TTL_SECS: i64 = 60;
pub const CI_FLOW_OUTBOX_SCHEMA_VERSION: u32 = 1;
pub const MAX_CI_WORKFLOW_SCOPES_PER_PASS: usize = MAX_ACTIVE_CI_RUN_PAGE;
pub const MAX_CI_WORKFLOW_DRIVES_PER_SCOPE: usize = 64;
const CI_MANIFEST_PIPELINE_CODE_HASH: &str =
    "blake3:d1ca2afcc81c9ca76f1744795fa43b083bfceb7703a9dce6bda6d614cea0ee9a";

pub fn ci_manifest_pipeline_definition() -> Result<CiWorkflowDefinitionPin, crate::PgCiStarterError>
{
    CiWorkflowDefinitionPin::new(CI_MANIFEST_PIPELINE_VERSION, CI_MANIFEST_PIPELINE_CODE_HASH)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiRuntimeCompositionError;

impl std::fmt::Display for CiRuntimeCompositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("exact-tenant CI runtime composition refused")
    }
}

impl std::error::Error for CiRuntimeCompositionError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiSupersededDefinitionBacklog {
    pub version: i32,
    pub runs: Vec<crate::SupersededCiPipelineRun>,
    pub truncated: bool,
}

impl std::fmt::Display for CiSupersededDefinitionBacklog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ci.pipeline definition activation refused: {} non-terminal run(s) are still pinned to \
             the superseded ci.pipeline@{} definition{}. This binary registers only \
             ci.pipeline@{}, and a Flow worker claims only locally-registered (wf_type, version) \
             keys - so these runs are permanently unclaimable, not merely delayed. REMEDIATION: \
             drain or cancel each run below through the existing cancellation/supersession path \
             (`DurableExecutor::cancel` / `PgCiRunSupersession`), then reboot. Stranded runs:",
            self.runs.len(),
            self.version,
            if self.truncated { " (truncated)" } else { "" },
            CI_MANIFEST_PIPELINE_VERSION,
        )?;
        for run in &self.runs {
            write!(f, " [tenant={} run={}]", run.tenant.0, run.wf_run_id)?;
        }
        if self.truncated {
            f.write_str(" …")?;
        }
        Ok(())
    }
}

impl std::error::Error for CiSupersededDefinitionBacklog {}

#[derive(Debug)]
pub enum CiSupersededDefinitionGuardError {
    Backlog(CiSupersededDefinitionBacklog),
    ProbeFailed(String),
    ActivationRefused(String),
    PredecessorMissing,
    FenceUnavailable(String),
    ActivationNotReady { unsafe_rows: i64 },
}

impl std::fmt::Display for CiSupersededDefinitionGuardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backlog(backlog) => write!(f, "{backlog}"),
            Self::ProbeFailed(error) => write!(
                f,
                "ci.pipeline superseded-definition guard could not be answered (fail-closed, the \
                 runner lane must not start on an unverified definition backlog): {error}"
            ),
            Self::ActivationRefused(detail) => write!(
                f,
                "ci.pipeline definition cutover refused (rolled back; the superseded definition \
                 remains active and the existing fleet is unaffected): {detail}"
            ),
            Self::PredecessorMissing => write!(
                f,
                "ci.pipeline definition cutover refused: the superseded \
                 {CI_PIPELINE_WF_TYPE}@{CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION} registry row is \
                 ABSENT, so there is nothing to lock and the fence would be vacuous - a \
                 concurrently-booting older binary could still register and admit under it. This is \
                 never 'nothing to fence'. REMEDIATION: apply the control-plane migrations \
                 (`{}` seeds this predecessor row on a fresh database), then reboot",
                crate::migrations::CI_PIPELINE_V5_CUTOVER_FENCE_ROW_MIGRATION_ID
            ),
            Self::FenceUnavailable(detail) => write!(
                f,
                "ci.pipeline definition cutover refused: the superseded-definition fence could not \
                 be acquired within {CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS}ms - an in-flight or \
                 abandoned admission transaction still holds it. The superseded definition remains \
                 active; retry once that transaction resolves: {detail}"
            ),
            Self::ActivationNotReady { unsafe_rows } => write!(
                f,
                "ci.pipeline definition cutover refused (rolled back; the superseded definition \
                 remains active): the activation-readiness probe found {unsafe_rows} non-terminal \
                 queue row(s) still lacking a claim window or carrying a reservation marker other \
                 than 2. Drain those rows before activating."
            ),
        }
    }
}

impl std::error::Error for CiSupersededDefinitionGuardError {}

async fn local_superseded_runs(
    discovery: &CiRegionRunDiscovery,
    region: &str,
    predecessor_version: i32,
) -> (Vec<crate::SupersededCiPipelineRun>, bool) {
    match discovery
        .superseded_definition_runs(
            region,
            predecessor_version,
            MAX_SUPERSEDED_CI_PIPELINE_RUN_PROBE,
        )
        .await
    {
        Ok(mut runs) => {
            let truncated = runs.len() > MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT;
            runs.truncate(MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT);
            (runs, truncated)
        }
        Err(_) => (Vec::new(), false),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationReadinessProbe {
    unsafe_count_call: std::borrow::Cow<'static, str>,
}

impl ActivationReadinessProbe {
    pub fn production() -> Self {
        Self {
            unsafe_count_call: std::borrow::Cow::Borrowed(CI_V2_ACTIVATION_READINESS_PROBE_CALL),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_call_for_tests(call: impl Into<String>) -> Self {
        Self {
            unsafe_count_call: std::borrow::Cow::Owned(call.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CutoverPlan {
    predecessor_version: i32,
    current_version: i32,
    current_code_hash: String,
    activation_readiness: Option<ActivationReadinessProbe>,
}

impl CutoverPlan {
    pub fn predecessor_version(&self) -> i32 {
        self.predecessor_version
    }

    pub fn current_version(&self) -> i32 {
        self.current_version
    }

    pub fn current_code_hash(&self) -> &str {
        &self.current_code_hash
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn for_tests(
        predecessor_version: i32,
        current_version: i32,
        current_code_hash: impl Into<String>,
    ) -> Self {
        Self {
            predecessor_version,
            current_version,
            current_code_hash: current_code_hash.into(),
            activation_readiness: None,
        }
    }

    pub fn with_activation_readiness(mut self, probe: ActivationReadinessProbe) -> Self {
        self.activation_readiness = Some(probe);
        self
    }

    pub fn has_activation_readiness(&self) -> bool {
        self.activation_readiness.is_some()
    }
}

#[derive(Clone)]
pub struct CiProductionRuntimeFactory {
    pool: sqlx::PgPool,
    region: Region,
    ledger: DurableCostLedger,
    rt: tokio::runtime::Handle,
    definition: CiWorkflowDefinitionPin,
    backlog_probe_call: std::borrow::Cow<'static, str>,
    activation_readiness_probe: ActivationReadinessProbe,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CiWorkflowFanoutBatch {
    pub discovered: usize,
    pub scopes: usize,
    pub driven: usize,
    pub timers_fired: usize,
    pub saturated: bool,
}

pub struct CiProductionWorkflowPoller {
    discovery: CiRegionRunDiscovery,
    runtime: CiProductionRuntimeFactory,
    worker_prefix: String,
    cursor: Option<CiActiveRunCursor>,
}

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
            definition: ci_manifest_pipeline_definition().map_err(|_| CiRuntimeCompositionError)?,
            backlog_probe_call: std::borrow::Cow::Borrowed(CI_PIPELINE_BACKLOG_PROBE_CALL),
            activation_readiness_probe: ActivationReadinessProbe::production(),
        })
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn definition(&self) -> &CiWorkflowDefinitionPin {
        &self.definition
    }

    pub async fn cutover_definition(
        &self,
        diagnostics: &CiRegionRunDiscovery,
    ) -> Result<(), CiSupersededDefinitionGuardError> {
        self.cutover_with_plan(&self.production_cutover_plan(), diagnostics)
            .await
    }

    fn production_cutover_plan(&self) -> CutoverPlan {
        CutoverPlan {
            predecessor_version: CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
            current_version: self.definition.version(),
            current_code_hash: self.definition.code_hash().to_string(),
            activation_readiness: Some(self.activation_readiness_probe.clone()),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn cutover_definition_with_plan(
        &self,
        plan: &CutoverPlan,
        diagnostics: &CiRegionRunDiscovery,
    ) -> Result<(), CiSupersededDefinitionGuardError> {
        self.cutover_with_plan(plan, diagnostics).await
    }

    async fn cutover_with_plan(
        &self,
        plan: &CutoverPlan,
        diagnostics: &CiRegionRunDiscovery,
    ) -> Result<(), CiSupersededDefinitionGuardError> {
        let mut transaction = self.pool.begin().await.map_err(|error| {
            CiSupersededDefinitionGuardError::ProbeFailed(format!(
                "begin definition cutover: {error}"
            ))
        })?;

        // @tenant-cross-scope: a transaction-local timeout setting, not a tenant-store read.
        sqlx::query(&format!(
            "SET LOCAL lock_timeout = '{CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS}ms'"
        ))
        .execute(&mut *transaction)
        .await
        .map_err(|error| {
            CiSupersededDefinitionGuardError::ProbeFailed(format!(
                "bound the definition fence wait: {error}"
            ))
        })?;

        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        let superseded = sqlx::query(
            "SELECT code_hash, status FROM wf_definition \
             WHERE wf_type = $1 AND version = $2 FOR UPDATE \
             /* global registry: tenant_id and region do not apply */",
        )
        .bind(CI_PIPELINE_WF_TYPE)
        .bind(plan.predecessor_version)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| {
            let timed_out = error
                .as_database_error()
                .and_then(|database| database.code())
                .as_deref()
                == Some("55P03");
            if timed_out {
                CiSupersededDefinitionGuardError::FenceUnavailable(error.to_string())
            } else {
                CiSupersededDefinitionGuardError::ProbeFailed(format!(
                    "lock the superseded definition row: {error}"
                ))
            }
        })?;
        let Some(superseded) = superseded else {
            let _ = transaction.rollback().await;
            return Err(CiSupersededDefinitionGuardError::PredecessorMissing);
        };
        let _ = superseded;

        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        let backlog: bool = sqlx::query_scalar(self.backlog_probe_call.as_ref())
            .bind(plan.predecessor_version)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| {
                CiSupersededDefinitionGuardError::ProbeFailed(format!(
                    "database-wide superseded-definition backlog probe: {error}"
                ))
            })?;
        if backlog {
            let _ = transaction.rollback().await;
            let (runs, truncated) =
                local_superseded_runs(diagnostics, &self.region.0, plan.predecessor_version).await;
            return Err(CiSupersededDefinitionGuardError::Backlog(
                CiSupersededDefinitionBacklog {
                    version: plan.predecessor_version,
                    runs,
                    truncated,
                },
            ));
        }

        if let Some(readiness) = plan.activation_readiness.as_ref() {
            // @tenant-cross-scope: `job_queue` is FORCE-RLS, but the readiness probe is a SECURITY
            let unsafe_rows: Option<i64> = sqlx::query_scalar(readiness.unsafe_count_call.as_ref())
                .fetch_one(&mut *transaction)
                .await
                .map_err(|error| {
                    CiSupersededDefinitionGuardError::ProbeFailed(format!(
                        "database-wide activation-readiness probe: {error}"
                    ))
                })?;
            match unsafe_rows {
                None => {
                    let _ = transaction.rollback().await;
                    return Err(CiSupersededDefinitionGuardError::ProbeFailed(
                        "database-wide activation-readiness probe returned NULL (fail-closed: a \
                         NULL count is never 'no unsafe rows')"
                            .to_string(),
                    ));
                }
                Some(0) => {}
                Some(unsafe_rows) => {
                    let _ = transaction.rollback().await;
                    return Err(CiSupersededDefinitionGuardError::ActivationNotReady {
                        unsafe_rows,
                    });
                }
            }
        }

        self.commit_activation(plan, transaction).await
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_backlog_probe_call_for_tests(mut self, call: impl Into<String>) -> Self {
        self.backlog_probe_call = std::borrow::Cow::Owned(call.into());
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn replace_activation_readiness_probe_call_for_tests(
        mut self,
        call: impl Into<String>,
    ) -> Self {
        self.activation_readiness_probe = ActivationReadinessProbe::with_call_for_tests(call);
        self
    }

    async fn commit_activation(
        &self,
        plan: &CutoverPlan,
        mut transaction: sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), CiSupersededDefinitionGuardError> {
        let refuse = |detail: String| CiSupersededDefinitionGuardError::ActivationRefused(detail);
        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        sqlx::query(
            "UPDATE wf_definition SET status = 'draining' \
             WHERE wf_type = $1 AND version = $2 AND status = 'active' \
             /* global registry: tenant_id and region do not apply */",
        )
        .bind(CI_PIPELINE_WF_TYPE)
        .bind(plan.predecessor_version)
        .execute(&mut *transaction)
        .await
        .map_err(|error| refuse(format!("drain the superseded definition: {error}")))?;
        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        sqlx::query(
            "INSERT INTO wf_definition (wf_type, version, code_hash, status) \
             VALUES ($1, $2, $3, 'active') ON CONFLICT (wf_type, version) DO NOTHING \
             /* global registry: tenant_id and region do not apply */",
        )
        .bind(CI_PIPELINE_WF_TYPE)
        .bind(plan.current_version)
        .bind(plan.current_code_hash())
        .execute(&mut *transaction)
        .await
        .map_err(|error| refuse(format!("activate the current definition: {error}")))?;

        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        let current = sqlx::query(
            "SELECT code_hash, status FROM wf_definition WHERE wf_type = $1 AND version = $2 \
             /* global registry: tenant_id and region do not apply */",
        )
        .bind(CI_PIPELINE_WF_TYPE)
        .bind(plan.current_version)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| refuse(format!("verify the activated definition: {error}")))?
        .ok_or_else(|| refuse("the activated definition row is absent after insert".into()))?;
        let current_hash: String = current
            .try_get("code_hash")
            .map_err(|error| refuse(format!("decode activated code hash: {error}")))?;
        let current_status: String = current
            .try_get("status")
            .map_err(|error| refuse(format!("decode activated status: {error}")))?;
        if current_hash != plan.current_code_hash() {
            return Err(refuse(format!(
                "{CI_PIPELINE_WF_TYPE}@{} is registered with a DIFFERENT code hash than this \
                 binary's embedded pin - refusing to activate a definition whose source is not the \
                 source this process would run",
                plan.current_version
            )));
        }
        if current_status != "active" {
            return Err(refuse(format!(
                "{CI_PIPELINE_WF_TYPE}@{} is `{current_status}`, not `active`",
                plan.current_version
            )));
        }
        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        let drained: String = sqlx::query_scalar(
            "SELECT status FROM wf_definition WHERE wf_type = $1 AND version = $2 \
             /* global registry: tenant_id and region do not apply */",
        )
        .bind(CI_PIPELINE_WF_TYPE)
        .bind(plan.predecessor_version)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| refuse(format!("verify the drained definition: {error}")))?;
        if !matches!(drained.as_str(), "draining" | "retired") {
            return Err(refuse(format!(
                "{CI_PIPELINE_WF_TYPE}@{} is `{drained}` after the cutover, expected `draining` or \
                 `retired`",
                plan.predecessor_version
            )));
        }
        transaction.commit().await.map_err(|error| {
            CiSupersededDefinitionGuardError::ProbeFailed(format!(
                "commit definition cutover (state is ambiguous; re-run to observe it): {error}"
            ))
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn activate_definition(&self) -> Result<(), CiRuntimeCompositionError> {
        PgFlowExecutor::new(
            self.pool.clone(),
            self.rt.clone(),
            Arc::new(MonotonicMinter::new()),
            TenantId("ci-definition-registry".into()),
            self.region.clone(),
        )
        .register_definition(
            CI_PIPELINE_WF_TYPE,
            self.definition.version(),
            self.definition.code_hash(),
        )
        .map_err(|_| CiRuntimeCompositionError)
    }

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
            CiJobAccountingStore::with_pg_and_write_version(
                self.pool.clone(),
                self.region.clone(),
                crate::ci_pipeline_protocol::PRODUCTION_ACCOUNTING_WRITE_VERSION,
            ),
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
                    CiJobAccountingStore::with_pg_and_write_version(
                        pool.clone(),
                        bound_region.clone(),
                        crate::ci_pipeline_protocol::PRODUCTION_ACCOUNTING_WRITE_VERSION,
                    ),
                    Arc::new(TierPOperationalCiJobPricer),
                ),
            ))
        });
        CiPipelineReporterRouter::new(self.region.clone(), factory)
            .map_err(|_| CiRuntimeCompositionError)
    }
}

impl CiProductionWorkflowPoller {
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
        let mut timers_fired = 0usize;
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
            timers_fired = timers_fired.saturating_add(batch.timers_fired);
            saturated |= batch.saturated;
        }
        Ok(CiWorkflowFanoutBatch {
            discovered,
            scopes,
            driven,
            timers_fired,
            saturated,
        })
    }

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
    fn production_definition_pin_is_explicit_and_stable() {
        let first = ci_manifest_pipeline_definition().unwrap();
        let second = ci_manifest_pipeline_definition().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.version(), CI_MANIFEST_PIPELINE_VERSION);
        assert_eq!(first.code_hash(), CI_MANIFEST_PIPELINE_CODE_HASH);
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
