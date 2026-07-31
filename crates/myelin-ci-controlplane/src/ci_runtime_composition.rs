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
use myelin_flow::{PgFlowExecutor, PgFlowWorker, PgWorkerScope, CI_PIPELINE_WF_TYPE};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{DurableCostLedger, SubstrateProvider, TenantScope};
use sqlx::Row;
use myelin_tenancy::{Region, TenantId};

use crate::{
    register_durable_ci_manifest_pipeline, CiActiveRunCursor, CiCostEventStore,
    CiDriveManifestStore, CiJobAccountingStore, CiJobQueueStore, CiJobSpecStore,
    CiManifestInputResolver, CiPipelineReporter, CiPipelineReporterFactory,
    CiPipelineReporterFactoryError, CiPipelineReporterRouter, CiRegionRunDiscovery, CiRunStore,
    CiWorkflowDefinitionPin, DurableCiJobAccounting, DurableCiRunFinalizer,
    TierPOperationalCiJobPricer, MAX_ACTIVE_CI_RUN_PAGE, MAX_SUPERSEDED_CI_PIPELINE_RUN_PROBE,
    MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT,
};

/// Version of the production manifest-native `ci.pipeline` definition.
///
/// Bumped 1 -> 2 (2026-07-25): the `ResourceLimits`/`WorkspaceSpec` disk/tmpfs split (CT-007
/// vertical-slice-step-2 workspace-storage work) touched `ci_manifest_job_runner.rs`'s bytes,
/// which this pin hashes. Per this function's own doc, any source change must be a deliberate
/// version bump, never a silent hash update against an already-recorded version.
///
/// Bumped 2 -> 3 (2026-07-30): the CT-007 lease/topology reconciliation makes
/// `manifest_dispatch_parts` derive and persist the immutable `claim_window_secs`, again touching
/// `ci_manifest_job_runner.rs`'s hashed bytes. The dispatch writer's durable output genuinely
/// changed — every new `job_queue` row now carries a window a v2 dispatch would not have written —
/// so this is exactly the deliberate bump the pin exists to force.
pub const CI_MANIFEST_PIPELINE_VERSION: i32 = 3;

/// **The definition version this binary supersedes, and therefore refuses to strand.**
///
/// A version bump is not free: [`PgFlowWorker::register_definition`] registers only the CURRENT
/// body, `run_once` claims only locally-registered `(wf_type, version)` keys, and `drive_claimed`
/// requires an exact version match. So a non-terminal `ci.pipeline@2` row under a v3-only binary is
/// not merely delayed — it is PERMANENTLY unclaimable, silently, with no worker ever erroring. The
/// alternative (keeping a frozen v2 body compiled in forever) is worse debt: duplicated legacy
/// dispatch code that must be maintained in lock-step with the live one.
///
/// So this binary refuses to boot its runner lane while any such row exists, and names it. Scoped to
/// exactly this one superseded version rather than a generic "any lower version" sweep: each bump is
/// a deliberate act whose predecessor is known, and a generic sweep would silently start covering
/// versions nobody reasoned about.
pub const CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION: i32 = 2;

/// Bounded wait for the cutover fence. PostgreSQL's default `lock_timeout` is 0 — wait forever —
/// which would let one abandoned or indefinitely stalled admission transaction hang v3 boot
/// silently instead of reaching the typed fail-closed path. Ten seconds is comfortably longer than
/// any healthy admission transaction (one manifest insert plus a workflow start) and short enough
/// that a stuck boot is diagnosed in the first restart rather than looking like a hang.
pub const CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS: u64 = 10_000;

/// **The cutover's backlog authority, SCHEMA-QUALIFIED (CT-007 round-3 blocker 3).** An unqualified
/// name resolves through `search_path`, and a shadowing function earlier on that path that returns
/// `false` instead of raising is a fail-OPEN cutover — the round-3 review demonstrated exactly that
/// substitution. Qualifying references inside the intended function body does not protect resolution
/// of the CALL. Schema-isolated tests get a dedicated seam
/// ([`CiProductionRuntimeFactory::with_backlog_probe_call_for_tests`]) rather than weakening this.
///
/// The probe lives in the dedicated `myelin_ci_security` schema, owned by the `BYPASSRLS` fence
/// role, so that role never needs `CREATE` on `public` (spec point 1). `myelin_app` gets schema
/// `USAGE` and function `EXECUTE` only — never membership in the fence role.
const CI_PIPELINE_BACKLOG_PROBE_CALL: &str = "\
SELECT myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs($1) \
/* global registry fence: database-wide by construction */";

/// **The v3→v4 activation-READINESS probe call, SCHEMA-QUALIFIED (CT-007 slice 5b.3-6e.1 — DORMANT).**
///
/// The companion to [`CI_PIPELINE_BACKLOG_PROBE_CALL`]. Where the backlog probe answers "is any run
/// still pinned to the superseded version", THIS answers "does any non-terminal queue row still lack a
/// claim window or carry a reservation marker other than 2" — the queue-safety half a v3→v4 activation
/// must clear before it drains v3. It is schema-qualified for the SAME fail-closed reason: an
/// unqualified name resolving through `search_path` to a shadowing `SELECT 0` would be a fail-OPEN
/// activation. It takes NO parameter and returns a single `bigint` unsafe-row count. DORMANT: the
/// production plan carries no readiness predicate ([`CutoverPlan::activation_readiness`] is `None`)
/// until the activating slice (6e.2) selects it.
const CI_V2_ACTIVATION_READINESS_PROBE_CALL: &str = "\
SELECT myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count() \
/* global queue-safety fence: database-wide by construction */";
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
        // CT-007 lease/topology reconciliation: the dispatch writer's durable output now includes
        // `claim_window_secs`, but the DERIVATION lives here, outside the two files above. Without
        // this entry a future change to the topology formula would change what v3 dispatch persists
        // WITHOUT changing the v3 code hash — exactly the silent drift this pin exists to prevent.
        include_bytes!("ci_claim_window.rs").as_slice(),
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

/// **Runs stranded on the definition version this binary superseded.** Loud and typed rather than a
/// bare bool: the whole point is that the operator gets the exact ids to act on, because there is no
/// automatic recovery — a v3-only worker will never claim these rows no matter how long it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiSupersededDefinitionBacklog {
    /// The version no longer registered by this binary.
    pub version: i32,
    /// The stranded `(tenant, run)` pairs, bounded by
    /// [`MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT`](crate::MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT).
    pub runs: Vec<crate::SupersededCiPipelineRun>,
    /// Whether more stranded rows exist beyond the reported bound.
    pub truncated: bool,
}

impl std::fmt::Display for CiSupersededDefinitionBacklog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ci.pipeline definition activation refused: {} non-terminal run(s) are still pinned to \
             the superseded ci.pipeline@{} definition{}. This binary registers only \
             ci.pipeline@{}, and a Flow worker claims only locally-registered (wf_type, version) \
             keys — so these runs are permanently unclaimable, not merely delayed. REMEDIATION: \
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

/// Why the superseded-definition boot guard could not clear the runner lane for activation.
#[derive(Debug)]
pub enum CiSupersededDefinitionGuardError {
    /// Stranded runs exist. Fail-closed: the lane must not start.
    Backlog(CiSupersededDefinitionBacklog),
    /// The guard's own probe failed. Also fail-closed — an unanswered guard is not a passed guard.
    ProbeFailed(String),
    /// The backlog was clean but the registry transition itself would not verify (a divergent
    /// hash, a non-active status, an unexpected drained state). Rolls back with v2 still active.
    ActivationRefused(String),
    /// The superseded definition row is ABSENT. Fail-closed: with no row there is nothing to lock,
    /// so the fence would be vacuous and a concurrently-booting old binary could still register and
    /// admit under the superseded version.
    PredecessorMissing,
    /// The fence could not be taken within its bounded timeout — an abandoned or stalled admission
    /// transaction still holds it. Fail-closed rather than hanging boot forever.
    FenceUnavailable(String),
    /// **CT-007 slice 5b.3-6e.1 (DORMANT).** The activation-readiness predicate found the queue NOT
    /// safe for this activation: at least one non-terminal row still lacks a claim window or carries a
    /// reservation marker other than 2. Rolled back with the predecessor still active. Only a plan
    /// that carries a readiness predicate can reach this (production v2→v3 carries none).
    ActivationNotReady {
        /// The count of unsafe non-terminal queue rows the database-wide probe found.
        unsafe_rows: i64,
    },
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
                 ABSENT, so there is nothing to lock and the fence would be vacuous — a \
                 concurrently-booting older binary could still register and admit under it. This is \
                 never 'nothing to fence'. REMEDIATION: apply the control-plane migrations \
                 (`{}` seeds this predecessor row on a fresh database), then reboot",
                crate::migrations::CI_PIPELINE_CUTOVER_FENCE_ROW_MIGRATION_ID
            ),
            Self::FenceUnavailable(detail) => write!(
                f,
                "ci.pipeline definition cutover refused: the superseded-definition fence could not \
                 be acquired within {CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS}ms — an in-flight or \
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

/// **List local stranded runs for a refusal message.** DIAGNOSTIC ONLY (CT-007 round-2 review): the
/// authority that decides whether the cutover may proceed is the database-wide probe inside
/// [`CiProductionRuntimeFactory::cutover_definition`], taken under the `wf_definition` row lock.
/// This regional read is not serialized with fresh admission and must never gate the transition —
/// it exists so the operator gets concrete `(tenant, run)` ids for their own region.
async fn local_superseded_runs(
    discovery: &CiRegionRunDiscovery,
    region: &str,
    predecessor_version: i32,
) -> (Vec<crate::SupersededCiPipelineRun>, bool) {
    // One OVER the report bound: fetching exactly the bound cannot distinguish "16 stranded rows"
    // from "16 and more", so the probe asks for 17 and reports 16.
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
        // A failed DIAGNOSTIC must not change the verdict the global probe already reached.
        Err(_) => (Vec::new(), false),
    }
}

/// **An optional activation-READINESS predicate for a [`CutoverPlan`]** (CT-007 slice 5b.3-6e.1, DORMANT).
///
/// The queue-safety half of a v3→v4 activation. It carries the schema-qualified call to the
/// database-wide readiness probe (`ci_0022c`); the fence runs it AFTER locking the predecessor `FOR
/// UPDATE` and AFTER the workflow-backlog probe, and BEFORE draining. A probe failure, a NULL result,
/// or any unsafe row all roll the fence back with the predecessor still active. Production's v2→v3
/// plan carries NO predicate; only the synthetic v3→v4 tests (and, in 6e.2, the production v3→v4 plan)
/// select one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationReadinessProbe {
    /// The schema-qualified call returning the `bigint` unsafe-row count. Production default is
    /// [`CI_V2_ACTIVATION_READINESS_PROBE_CALL`]; a schema-isolated test substitutes it.
    unsafe_count_call: std::borrow::Cow<'static, str>,
}

impl ActivationReadinessProbe {
    /// The production readiness probe — the schema-qualified call to the `ci_0022c` function.
    pub fn production() -> Self {
        Self {
            unsafe_count_call: std::borrow::Cow::Borrowed(CI_V2_ACTIVATION_READINESS_PROBE_CALL),
        }
    }

    /// **Substitute the readiness-probe call for a SCHEMA-ISOLATED live test.** Production resolution
    /// stays schema-qualified; this points the fence at a fixture's own schema-local copy of the probe
    /// rather than weakening the production call.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_call_for_tests(call: impl Into<String>) -> Self {
        Self {
            unsafe_count_call: std::borrow::Cow::Owned(call.into()),
        }
    }
}

/// **A typed (predecessor, current) definition-cutover plan.**
///
/// The cutover fence was originally hard-coded to the single v2→v3 transition. Generalizing it to a
/// typed plan lets a future v3→v4 cutover (CT-007 slice 6e) reuse the EXACT same fence — the
/// `wf_definition FOR UPDATE` serialization, the database-wide backlog probe, and every fail-closed
/// path (missing predecessor, divergent current hash, lock timeout, commit ambiguity) — without
/// editing a single line of the heavily-reviewed fence body.
///
/// **Production still constructs and runs ONLY the v2→v3 plan.**
/// [`CiProductionRuntimeFactory::cutover_definition`] builds it through
/// [`CiProductionRuntimeFactory::production_cutover_plan`] from this binary's own source-derived pin;
/// nothing else runs a cutover. The v3→v4 plan exists exclusively in synthetic tests until slice 6e
/// activates it (and ships the retired-v3 fresh-DB sentinel migration this slice deliberately does
/// not).
///
/// The three fields are the complete set the fence parameterizes over:
/// - `predecessor_version` — the version drained and fenced (`FOR UPDATE`d, then backlog-probed);
/// - `current_version` / `current_code_hash` — the version activated and hash-verified.
///
/// Plus, dormant from 5b.3-6e.1, an optional [`ActivationReadinessProbe`] the fence runs before drain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CutoverPlan {
    predecessor_version: i32,
    current_version: i32,
    current_code_hash: String,
    /// CT-007 slice 5b.3-6e.1 (DORMANT): the optional activation-readiness predicate. `None` for the
    /// production v2→v3 plan (byte/behavior-identical to the frozen fence); `Some` only for the
    /// synthetic v3→v4 tests until 6e.2 selects it in production.
    activation_readiness: Option<ActivationReadinessProbe>,
}

impl CutoverPlan {
    /// The predecessor version this plan drains and fences.
    pub fn predecessor_version(&self) -> i32 {
        self.predecessor_version
    }

    /// The current version this plan activates.
    pub fn current_version(&self) -> i32 {
        self.current_version
    }

    /// The source-derived pin hash the activated current version is required to carry.
    pub fn current_code_hash(&self) -> &str {
        &self.current_code_hash
    }

    /// **Construct an ARBITRARY (predecessor, current) plan — test-support only.** Production
    /// constructs exactly the v2→v3 plan through
    /// [`CiProductionRuntimeFactory::production_cutover_plan`] from its own source-derived pin; this
    /// seam exists so the synthetic v3→v4 fence tests can drive the generalized cutover over
    /// in-test-seeded `wf_definition` rows before slice 6e activates that transition in production.
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

    /// **Select an activation-readiness predicate on this plan (CT-007 slice 5b.3-6e.1 — DORMANT).**
    /// Consumes and returns the plan with the predicate attached. No production root calls this in
    /// 6e.1 (the occurrence pin holds it at zero); the activating slice (6e.2) attaches
    /// [`ActivationReadinessProbe::production`] to the production v3→v4 plan here, and the synthetic
    /// tests attach a schema-isolated one.
    pub fn with_activation_readiness(mut self, probe: ActivationReadinessProbe) -> Self {
        self.activation_readiness = Some(probe);
        self
    }

    /// Whether this plan carries an activation-readiness predicate.
    pub fn has_activation_readiness(&self) -> bool {
        self.activation_readiness.is_some()
    }
}

/// Production factory for exact-tenant workflow workers and terminal reporter routing.
#[derive(Clone)]
pub struct CiProductionRuntimeFactory {
    pool: sqlx::PgPool,
    region: Region,
    ledger: DurableCostLedger,
    rt: tokio::runtime::Handle,
    definition: CiWorkflowDefinitionPin,
    /// The schema-qualified backlog-probe call. Only a schema-isolated test may substitute it; the
    /// production default is [`CI_PIPELINE_BACKLOG_PROBE_CALL`].
    backlog_probe_call: std::borrow::Cow<'static, str>,
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
            backlog_probe_call: std::borrow::Cow::Borrowed(CI_PIPELINE_BACKLOG_PROBE_CALL),
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

    /// **The definition CUTOVER FENCE — one transaction serialized with fresh v2 admission.**
    ///
    /// A preflight `SELECT` cannot close this race (CT-007 round-2 review): the v3 process could
    /// observe an empty backlog, an already-in-flight old-binary starter transaction could then
    /// commit a fresh v2 `workflow_run`, and once the old binary retires that run is stranded
    /// forever. Nothing connected the two operations.
    ///
    /// The fix reuses the lock protocol the old binary ALREADY participates in.
    /// `validate_definition_pin` takes `SELECT … FROM wf_definition WHERE wf_type=$1 AND version=$2
    /// FOR SHARE` and holds it until its start transaction commits or rolls back. So taking
    /// `FOR UPDATE` on that same row is a genuine mutual exclusion against fresh admission:
    ///
    /// - **Old admission wins the lock:** this `FOR UPDATE` waits. The old transaction must finish
    ///   first, so its `workflow_run` insert is committed before the backlog probe's later READ
    ///   COMMITTED snapshot — the probe sees it and the cutover refuses.
    /// - **Cutover wins the lock:** every later `FOR SHARE` blocks. The probe scans the complete
    ///   pre-fence backlog, the transition commits atomically, and the old transaction then wakes,
    ///   reads `status='draining'`, and refuses before writing a manifest, jobs, or workflow.
    /// - **Cutover rolls back:** blocked admissions wake to v2 still `active`; the old fleet is
    ///   unaffected.
    ///
    /// Conflicting row locks plus a post-lock READ COMMITTED statement give the required
    /// happens-before; `SERIALIZABLE` is unnecessary.
    ///
    /// The backlog authority is the DATABASE-WIDE probe, not a regional scan, because
    /// `wf_definition` has no region column: flipping v2 to `draining` fences every region this
    /// database serves. Existing v2 runs keep draining — `validate_definition_pin` accepts
    /// `active | draining` on replay — only fresh v2 admission is fenced.
    ///
    /// Idempotent: a retry that finds v2 already `draining` succeeds ONLY when v3 already exists,
    /// `active`, with the exact expected hash. It never reactivates v2.
    /// `diagnostics` is the SCHEDULER-backed regional discovery (round-3 finding 4). It is used
    /// only to turn a refusal into concrete local `(tenant, run)` ids: the app pool this factory
    /// holds is RLS-blind cross-tenant, so a diagnostic built from it would report "0 runs" for a
    /// real backlog. The verdict authority remains the database-wide probe.
    pub async fn cutover_definition(
        &self,
        diagnostics: &CiRegionRunDiscovery,
    ) -> Result<(), CiSupersededDefinitionGuardError> {
        // PRODUCTION runs EXACTLY ONE plan: the v2→v3 transition mechanically derived from this
        // binary's source pin. The fence body below is fully generalized over an arbitrary
        // (predecessor, current) plan so slice 6e can add v3→v4 without re-touching it, but there is
        // no production code path that constructs any plan other than this one.
        self.cutover_with_plan(&self.production_cutover_plan(), diagnostics)
            .await
    }

    /// **The production v2→v3 cutover plan, mechanically derived from this binary's source pin.** The
    /// predecessor is the compiled-in [`CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION`]; the current version
    /// and hash are the exact source-derived pin the starter and `worker_for` also register. This is
    /// the ONLY plan any production root runs — [`Self::cutover_definition`] constructs it and nothing
    /// else can. For the v2→v3 pair its three fields are byte-identical to the values the fence
    /// formerly hard-coded, so the generalized fence produces byte/behavior-identical SQL, binds, and
    /// refusals to the frozen cutover.
    fn production_cutover_plan(&self) -> CutoverPlan {
        CutoverPlan {
            predecessor_version: CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
            current_version: self.definition.version(),
            current_code_hash: self.definition.code_hash().to_string(),
            // DORMANT: the production v2→v3 plan carries NO readiness predicate, so `cutover_with_plan`
            // runs the frozen fence byte/behavior-identically. 6e.2 attaches the predicate here.
            activation_readiness: None,
        }
    }

    /// **Drive the generalized cutover fence over an arbitrary (predecessor, current) plan —
    /// test-support only.** Production always runs [`Self::cutover_definition`]'s v2→v3 plan; this
    /// seam lets the synthetic v3→v4 fence tests exercise the IDENTICAL fence body over
    /// in-test-seeded rows before slice 6e activates that transition.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn cutover_definition_with_plan(
        &self,
        plan: &CutoverPlan,
        diagnostics: &CiRegionRunDiscovery,
    ) -> Result<(), CiSupersededDefinitionGuardError> {
        self.cutover_with_plan(plan, diagnostics).await
    }

    /// The generalized fence body shared by the production v2→v3 wrapper and the test-support seam.
    /// Every version/hash the fence touches comes from `plan`; the queries, lock modes, ordering, and
    /// fail-closed paths are the frozen cutover's, unchanged.
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

        // (0) A BOUNDED wait for the fence. Without this, PostgreSQL's default `lock_timeout = 0`
        // lets one abandoned admission transaction hang boot forever instead of failing closed.
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

        // (1) THE FENCE. Global code registry: `wf_definition` has no tenant/region column, so the
        // usual tenant predicate does not apply — the same carve-out `PgFlowExecutor` annotates.
        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        // registry (wf_type/version/hash/status only — no tenant, region or PII column), and the
        // backlog probe answers a database-wide question by construction. Neither has a tenant
        // column to bind; the fence's whole purpose is that it is not tenant-scoped.
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
            // `55P03 lock_not_available` is the bounded-wait expiring, which is a DIFFERENT
            // operational story from a broken probe: the fence exists and someone else holds it.
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
            // ABSENCE IS NOT "NOTHING TO FENCE" (round-3 blocker 1). With no row there is nothing to
            // lock, so a concurrently-booting older binary could `register_definition` the
            // superseded version with no conflicting lock and reopen late admission; and an orphaned
            // non-terminal run under it would never be probed. The migration seeds this row on a
            // fresh database precisely so this path stays unreachable in practice.
            let _ = transaction.rollback().await;
            return Err(CiSupersededDefinitionGuardError::PredecessorMissing);
        };
        // The row's contents are not consulted: its EXISTENCE is what makes the fence real, and the
        // post-transition verification below re-reads the authoritative status anyway.
        let _ = superseded;

        // (2) THE AUTHORITY: database-wide, boolean-only, fail-closed. Runs while the fence is held,
        // so its READ COMMITTED snapshot is strictly after any admission that beat us to the lock.
        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        // registry (wf_type/version/hash/status only — no tenant, region or PII column), and the
        // backlog probe answers a database-wide question by construction. Neither has a tenant
        // column to bind; the fence's whole purpose is that it is not tenant-scoped.
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
            // Roll the fence back FIRST so the old fleet resumes immediately, then gather local ids
            // purely to make the refusal actionable.
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

        // (2b) THE ACTIVATION-READINESS PREDICATE (CT-007 slice 5b.3-6e.1 — DORMANT). Runs INSIDE the
        // held fence, AFTER the backlog probe and BEFORE any drain/activate, so its READ COMMITTED
        // snapshot is strictly after any admission that beat us to the lock — the same happens-before
        // the backlog probe relies on. Production's v2→v3 plan carries no predicate, so this whole
        // block is skipped and the fence stays byte/behavior-identical to the frozen cutover. When a
        // plan DOES carry a predicate, a probe error, a NULL result, or ANY unsafe row all roll the
        // fence back with the predecessor still active — fail-closed, exactly like the backlog probe.
        if let Some(readiness) = plan.activation_readiness.as_ref() {
            // @tenant-cross-scope: `job_queue` is FORCE-RLS, but the readiness probe is a SECURITY
            // DEFINER function owned by the bypass-RLS fence role that answers a database-wide
            // aggregate by construction — there is no tenant scope to bind, which is the whole point
            // of the fence (the same carve-out the backlog probe above annotates).
            let unsafe_rows: Option<i64> =
                sqlx::query_scalar(readiness.unsafe_count_call.as_ref())
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(|error| {
                        CiSupersededDefinitionGuardError::ProbeFailed(format!(
                            "database-wide activation-readiness probe: {error}"
                        ))
                    })?;
            match unsafe_rows {
                // A NULL result is fail-closed: a shadowing `SELECT NULL` or a degenerate probe must
                // never be read as "zero unsafe rows".
                None => {
                    let _ = transaction.rollback().await;
                    return Err(CiSupersededDefinitionGuardError::ProbeFailed(
                        "database-wide activation-readiness probe returned NULL (fail-closed: a \
                         NULL count is never 'no unsafe rows')"
                            .to_string(),
                    ));
                }
                Some(0) => { /* the queue is safe for this activation — proceed to drain/activate */ }
                Some(unsafe_rows) => {
                    let _ = transaction.rollback().await;
                    return Err(CiSupersededDefinitionGuardError::ActivationNotReady { unsafe_rows });
                }
            }
        }

        self.commit_activation(plan, transaction).await
    }

    /// Substitute the backlog-probe call for a SCHEMA-ISOLATED live test. Production resolution stays
    /// schema-qualified; this exists so a fixture can point the fence at its own schema's copy of the
    /// probe rather than the production call being weakened to an unqualified name.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_backlog_probe_call_for_tests(mut self, call: impl Into<String>) -> Self {
        self.backlog_probe_call = std::borrow::Cow::Owned(call.into());
        self
    }

    /// The transition half of [`Self::cutover_with_plan`], still inside the fenced transaction. Every
    /// version and hash comes from `plan`; for the production v2→v3 plan these equal the values this
    /// method formerly hard-coded, so the drained/activated queries, binds, and refusals are
    /// byte-identical.
    async fn commit_activation(
        &self,
        plan: &CutoverPlan,
        mut transaction: sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<(), CiSupersededDefinitionGuardError> {
        let refuse = |detail: String| CiSupersededDefinitionGuardError::ActivationRefused(detail);
        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        // registry (wf_type/version/hash/status only — no tenant, region or PII column), and the
        // backlog probe answers a database-wide question by construction. Neither has a tenant
        // column to bind; the fence's whole purpose is that it is not tenant-scoped.
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
        // registry (wf_type/version/hash/status only — no tenant, region or PII column), and the
        // backlog probe answers a database-wide question by construction. Neither has a tenant
        // column to bind; the fence's whole purpose is that it is not tenant-scoped.
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

        // Re-read BOTH rows and require the complete post-cutover shape. This is what makes a retry
        // over an already-drained v2 safe: it succeeds only when v3 is genuinely there and exact.
        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        // registry (wf_type/version/hash/status only — no tenant, region or PII column), and the
        // backlog probe answers a database-wide question by construction. Neither has a tenant
        // column to bind; the fence's whole purpose is that it is not tenant-scoped.
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
                 binary's embedded pin — refusing to activate a definition whose source is not the \
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
        // The post-state requirement is that the superseded version is NOT admissible for a fresh
        // start. `draining` is what an active predecessor becomes; `retired` is the seeded
        // fresh-database sentinel, which must not be resurrected into `draining`.
        // @tenant-cross-scope: `wf_definition` is the schema's one deliberate GLOBAL code
        // registry (wf_type/version/hash/status only — no tenant, region or PII column), and the
        // backlog probe answers a database-wide question by construction. Neither has a tenant
        // column to bind; the fence's whole purpose is that it is not tenant-scoped.
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
            // Commit ambiguity is fail-closed: on reboot the cutover is atomic, so the registry is
            // either the old state or the complete new one, never half.
            CiSupersededDefinitionGuardError::ProbeFailed(format!(
                "commit definition cutover (state is ambiguous; re-run to observe it): {error}"
            ))
        })
    }

    /// Register the exact source-derived definition pin in Flow's durable global code registry.
    /// Re-registration is idempotent; a changed hash at the same version fails closed before any
    /// queued run can be admitted.
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

    /// **CT-007 slice 5b.3-6e.1 (DORMANT): the activation-readiness predicate is composition-root
    /// ZERO.** A `for_tests` plan and the production plan both carry `None`, so the fence runs the
    /// frozen byte/behavior-identical body; and no production COMPOSITION ROOT (`main.rs`,
    /// `runner_bind.rs`, this module's own production paths) ever SELECTS the predicate — the
    /// `.with_activation_readiness(` call and the `ActivationReadinessProbe::production` constructor
    /// have zero occurrences in every production source file. Language-level, not merely counting:
    /// the `for_tests`/production constructors return `has_activation_readiness() == false` by
    /// construction, and the source-pin below proves nothing wires the predicate in.
    #[test]
    fn the_activation_readiness_predicate_has_no_production_selection() {
        // Constructed plans carry no predicate by construction.
        assert!(!CutoverPlan::for_tests(3, 4, "blake3:x").has_activation_readiness());

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        fn production_of(source: &str) -> &str {
            match source.find("\n#[cfg(test)]\nmod tests {") {
                Some(end) => &source[..end],
                None => source,
            }
        }
        fn code_occurrences(source: &str, marker: &str) -> usize {
            production_of(source)
                .lines()
                .filter(|line| {
                    let t = line.trim_start();
                    !t.starts_with("//") && !t.starts_with('*')
                })
                .map(|line| line.matches(marker).count())
                .sum()
        }
        // The predicate is never SELECTED by any production composition ROOT (Sol's 6e.1 major 2:
        // include lib.rs; scan the BARE symbol so UFCS `CutoverPlan::with_activation_readiness(..)`
        // cannot evade it). These roots do NOT define the selector, so a bare-symbol count of 0 is the
        // full guarantee — any occurrence at all is a premature wiring.
        for file in ["main.rs", "runner_bind.rs", "ci_runner_composition.rs", "lib.rs"] {
            let source = std::fs::read_to_string(src.join(file)).unwrap();
            assert_eq!(
                code_occurrences(&source, "with_activation_readiness"),
                0,
                "{file} must not SELECT the activation-readiness predicate while it is dormant"
            );
            assert_eq!(
                code_occurrences(&source, "ActivationReadinessProbe::production"),
                0,
                "{file} must not construct the production readiness probe while it is dormant"
            );
        }
        // This module DEFINES `with_activation_readiness`, so the bare symbol legitimately occurs once;
        // the composition-root zero here is that it is never CALLED (`.with_activation_readiness(` and
        // the UFCS `CutoverPlan::with_activation_readiness(` are both absent) and the production probe
        // constructor is never named.
        let this = std::fs::read_to_string(src.join("ci_runtime_composition.rs")).unwrap();
        assert_eq!(code_occurrences(&this, ".with_activation_readiness("), 0);
        assert_eq!(code_occurrences(&this, "CutoverPlan::with_activation_readiness("), 0);
        assert_eq!(code_occurrences(&this, "ActivationReadinessProbe::production"), 0);
        // And the production cutover plan pins `activation_readiness: None` at its one construction
        // site (the frozen v2→v3 byte-equivalence), together with `for_tests`.
        assert!(
            code_occurrences(&this, "activation_readiness: None") >= 2,
            "the production and for_tests plans both pin the predicate to None"
        );
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
