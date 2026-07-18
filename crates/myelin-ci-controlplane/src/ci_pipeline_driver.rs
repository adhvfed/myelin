//! # `ci_pipeline_driver` — CT-004d.2 CULMINATION (chunks 2 + 3 + 5): a pushed CI trigger runs a REAL pipeline end-to-end
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §3.1 (the pipeline IS a durable workflow — `run_ci_pipeline_body`), §3.3 (the
//! `SCHEDULE_AND_RUN_JOB` dispatch handshake → the durable `job_queue` row), §2.1 (the pull-lease claim
//! the CT-004c.2 runner drives) + arch 01 §3.1 (`ci_run` is the thin index over the myelin-flow run).
//!
//! ## What this closes — the LAST wire: a `job.done` from a real `runsc` guest WAKES a parked pipeline
//! CT-004b armed a durable `ci_run` (queued) + pre-minted `wf_run_id`; CT-004c.1/c.2 made the runner
//! CLAIM a durable `job_queue` row + EXECUTE it in gVisor + report `job.done`; CT-004d.1 made the
//! dispatch co-persist `job_queue` + `ci_job_spec`; CT-004d.2 chunk 4 co-committed the durable `ci_run`.
//! **NOTHING started the parked `ci.pipeline` run, dispatched its stages through the DURABLE queue, or
//! drove the engine `tick` that consumes the runner's `job.done`.** This module is those three coupled
//! chunks, in ONE process (the SAME process as [`crate::CiRunnerLoop`], so the runner's
//! `job.done` signal lands on the executor that owns the parked run):
//!
//! - **Chunk 5 — [`DurableJobRunner`]:** the [`myelin_flow::JobRunner`] the pipeline body dispatches
//!   each stage through. Instead of [`crate::SchedulerJobRunner`]'s in-memory `SchedulerState`, it
//!   builds a [`DurableEnqueue`] + the digest-pinned sandbox [`SandboxJobSpec`] and calls
//!   [`CiJobSpecStore::co_persist_dispatch`] — so each stage lands a DURABLE `job_queue` row (+ its
//!   `ci_job_spec`) the CT-004c.2 runner claims. **THE SECURITY INVARIANT:** the enqueue's `trust_tier`
//!   + `region` come from the run's real [`JobScheduleTerms`] (stamped from `ci_run.trust_tier` /
//!   `ci_run.region` at trigger time), forwarded UNCHANGED, and the SAME tier is stamped onto the
//!   sandbox spec — so `co_persist_dispatch`'s `enq.trust_tier == spec.trust_tier` gate holds by
//!   construction and an `untrusted_fork` stage can NEVER be enqueued behind a widened `trusted` gate.
//! - **Chunk 2 — [`CiPipelineDriver`]:** constructs a [`FlowExecutor`] + drives a [`FlowDispatcher`]
//!   over a SHARED `RunStore`/`SignalStore`, registers `run_ci_pipeline_body` under [`CI_PIPELINE_WF_TYPE`]
//!   (with the chunk-5 durable runner injected per run), and `tick`s a background driver. The runner
//!   loop's terminal reporter signals THIS executor, so `job.done` wakes the parked run.
//! - **Chunk 3 — [`CiPipelineDriver::start_run`]:** reads a durable `ci_run` (queued) row and calls
//!   [`DurableExecutor::start_with_id`] with the pre-minted `wf_run_id` as `Some(RunId(wf_run_id))` — so
//!   the parked run's id EQUALS the `job_queue` row's `run_id` the runner reports `job.done` to.
//!
//! ## The verdict-vocabulary bridge (why a bespoke reporter, not `EngineTerminalReporter`)
//! The real sandbox runner ([`myelin_ci_sandbox::RunnerAgent::run_one`]) DERIVES `passed` from the guest
//! exit code and reports it as a `myelin://job-done/passed-<bool>` marker; the pipeline body
//! ([`myelin_flow::WfCtx::run_ci_pipeline`]) decodes the stage verdict from a
//! [`myelin_flow::stage_verdict_marker`] (`ci.stage.verdict:<pass|fail>:<stage>`). Neither frozen body
//! is touched (the sandbox `run_one` security body, the engine fixture). The bridge is the
//! [`myelin_ci_sandbox::TerminalReporter`] seam — a legitimate injection point the runner already
//! depends on abstractly: [`CiPipelineReporter`] re-encodes the runner's derived `passed` into the
//! stage-verdict marker the body decodes (the stage NAME comes from the [`StageVerdictBridge`] the
//! [`DurableJobRunner`] populated at dispatch, keyed on the deterministic `idem_token`), then WAKES the
//! parked run so the dispatcher re-drives + consumes the signal. A stage the bridge has no mapping for
//! falls back to the raw `passed` marker (behaviourally identical to [`myelin_ci_sandbox::EngineTerminalReporter`])
//! — never a fabricated pass.
//!
//! ## The durable-RunStore FLOOR (named, not silently skipped)
//! [`CiPipelineDriver`] drives the engine over the IN-MEMORY [`myelin_flow::RunStore`] (the M2 engine's
//! named floor — a restart loses in-flight runs). The durable recovery record is the `ci_run` row
//! (chunk 4) carrying the pre-minted `wf_run_id`: after a restart, the starter re-reads the queued
//! `ci_run` and re-`start_with_id`s the SAME id (idempotent). A durable `RunStore` (a live-PG
//! `workflow_run` lease/replay binding) is NOT built here — it is the myelin-flow M2 named floor.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind as SandboxJobKind, JobSpec as SandboxJobSpec,
    MeterTarget, ResourceLimits, RunTokenRef, TerminalReport, TerminalReporter, TrustTier,
    WorkspaceSpec,
};
use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use myelin_flow::{
    stage_verdict_marker, ActivityError, DriveOutcome, DurableExecutor, ExecutorError,
    FlowDispatcher, FlowExecutor, FlowTelemetry, JobRunner, JobSpec as FlowJobSpec, RunId,
    SignalOutcome, SignalSpec, StartSpec, TimerStore, WfCtx, WfJournal, WorkflowBody,
    CI_PIPELINE_WF_TYPE, JOB_DONE_SIGNAL, PARTITION_COUNT,
};

use crate::ci_pipeline::{run_ci_pipeline_body, PipelineRun, PipelineStage, RunVerdict};
use crate::ci_run_store::CiRunRecord;
use crate::job_queue_store::{trust_from_token, DurableEnqueue, JobQueueStoreError};
use crate::job_spec_store::{CiJobSpecStore, MAX_JOB_TIMEOUT_SECS};
use crate::schedule_and_run_job::JobScheduleTerms;
use crate::scheduler::Lane;

/// Bridge one async durable-store call to a sync body on a dedicated OFF-runtime thread (the SAME
/// convention [`crate::runner_bind`] + `myelin_storage::kms_durable` use). The pipeline `tick` runs on
/// its own thread; the `try_current` guard falls back to `block_in_place` if ever driven on a
/// multi-thread worker.
fn bridge<F: std::future::Future>(rt: &tokio::runtime::Handle, fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(fut)),
        Err(_) => rt.block_on(fut),
    }
}

/// **The seam that resolves a dispatched stage to its digest-pinned sandbox [`SandboxJobSpec`] template
/// (the `.myelin/ci.toml` resolved-snapshot → executable-spec resolution).** Given the flow
/// [`FlowJobSpec`] the pipeline body dispatched (its opaque `target` names the pipeline step), it
/// returns the image/command/limits/egress/workspace the sandbox launches. `Err` is a fail-closed
/// resolve (the stage never becomes a launchable durable job). **The builder does NOT set the
/// security-load-bearing `trust_tier` or the `idem_token`** — [`DurableJobRunner::dispatch`] STAMPS
/// those from the run's terms + the dispatch, so a builder can never widen the trust tier.
///
/// In production the impl resolves the pinned snapshot's per-stage command; the CT-004d.2 integration
/// test injects a real compute spec that runs in a `runsc` guest. Until the snapshot resolver lands
/// (the named follow-on), [`unresolved_stage_spec_builder`] is the fail-closed default.
pub type StageSpecBuilder =
    Arc<dyn Fn(&FlowJobSpec) -> Result<SandboxJobSpec, String> + Send + Sync>;

/// **The fail-closed default stage-spec builder (the snapshot→spec resolver is the named follow-on).**
/// Returns `Err` for every stage — a driver wired with this dispatches NOTHING (the activity retries +
/// the run fails loud), never a fabricated spec. The real resolver (the pinned `.myelin/ci.toml`
/// snapshot → per-stage command/image) is CT-004d.3+; the integration test injects a real builder.
pub fn unresolved_stage_spec_builder() -> StageSpecBuilder {
    Arc::new(|spec: &FlowJobSpec| {
        Err(format!(
            "no pinned-snapshot → JobSpec resolver yet (CT-004d follow-on) for stage target `{}`; \
             the driver cannot fabricate an executable spec — dispatch refused fail-closed",
            spec.target
        ))
    })
}

/// **The in-process idem_token → stage-name bridge (the verdict-vocabulary translation seam).** The
/// [`DurableJobRunner`] records `(idem_token, stage_name)` at dispatch; the [`CiPipelineReporter`] reads
/// it back to re-encode the runner's derived `passed` into the [`stage_verdict_marker`] the pipeline
/// body decodes. Cloneable (shared `Arc<Mutex<…>>`) so the runner + the reporter (same process) share
/// ONE map. Named `…Bridge` (NOT `…Registry`/`…Store`) — it is an in-memory translation cache, not a
/// durable-by-contract store (the durable dispatch record is the `job_queue` + `ci_job_spec` rows).
#[derive(Clone, Default)]
pub struct StageVerdictBridge {
    by_token: Arc<Mutex<HashMap<String, String>>>,
}

impl StageVerdictBridge {
    /// A fresh, empty bridge.
    pub fn new() -> StageVerdictBridge {
        StageVerdictBridge::default()
    }

    /// Record the stage name a dispatched `idem_token` belongs to (the runner echoes `idem_token` on
    /// `job.done`; the reporter maps it back to the stage the body attributes the verdict to).
    fn record(&self, idem_token: &str, stage: &str) {
        self.by_token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(idem_token.to_string(), stage.to_string());
    }

    /// The stage name for a dispatched `idem_token`, if the runner recorded one.
    fn stage_for(&self, idem_token: &str) -> Option<String> {
        self.by_token
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(idem_token)
            .cloned()
    }
}

// =================================================================================================
// Chunk 5 — the DURABLE JobRunner.
// =================================================================================================

/// **Chunk 5 — the DURABLE [`JobRunner`] the pipeline body dispatches each stage through.** Replaces
/// [`crate::SchedulerJobRunner`]'s in-memory `SchedulerState`: on [`JobRunner::dispatch`] it builds a
/// [`DurableEnqueue`] + the sandbox [`SandboxJobSpec`] and calls [`CiJobSpecStore::co_persist_dispatch`]
/// — one atomic tenant-scoped tx writes the `job_queue` row (the claim gate) + the `ci_job_spec` row
/// (what EXECUTES), idempotent on the engine-minted `idem_token`. Constructed PER RUN (it holds the
/// run's [`JobScheduleTerms`]); the pipeline body closure builds it fresh for each drive.
///
/// **THE SECURITY INVARIANT (the adversarial-verifier surface).** The `trust_tier` + `region` the
/// enqueue gates the claim on come from `self.terms` (the run's real facts, stamped from
/// `ci_run.trust_tier` / `ci_run.region` at trigger time), forwarded UNCHANGED — never widened,
/// defaulted, or dropped. The SAME `terms.trust_tier` is stamped onto the sandbox spec, so
/// `co_persist_dispatch`'s `enq.trust_tier == spec.trust_tier` assertion holds BY CONSTRUCTION (it is
/// not bypassed — it is fed the truth). The [`StageSpecBuilder`] never sets the tier, so it cannot
/// widen it. An `untrusted_fork` run therefore enqueues every stage as `untrusted_fork`, and the
/// CT-004c.2 trusted-only runner never claims it (the durable predicate) — the poisoned-pipeline
/// defence, closed at the dispatch.
pub struct DurableJobRunner {
    store: CiJobSpecStore,
    rt: tokio::runtime::Handle,
    /// the run's real scheduling terms — tenant/region/run_id/lane/labels/trust_tier/fair_key, a PURE
    /// function of the resolved snapshot (the trust tier stamped at trigger time). Forwarded UNCHANGED.
    terms: JobScheduleTerms,
    build_spec: StageSpecBuilder,
    verdicts: StageVerdictBridge,
    /// `(stage target → stage name)` for THIS run's pipeline — so a dispatched flow `JobSpec` (which
    /// carries the opaque `target`, not the stage name) maps back to the stage name the verdict codec
    /// needs. Built from the [`PipelineRun`]'s stages at construction.
    targets: Vec<(String, String)>,
}

impl DurableJobRunner {
    /// Build the durable runner for one run: the durable `ci_job_spec` store, the runtime handle the
    /// async co-persist bridges onto, the run's [`JobScheduleTerms`] (the security-load-bearing tier +
    /// region), the [`StageSpecBuilder`], the shared [`StageVerdictBridge`], and the run's pipeline
    /// stages (for the target → name map).
    pub fn new(
        store: CiJobSpecStore,
        rt: tokio::runtime::Handle,
        terms: JobScheduleTerms,
        build_spec: StageSpecBuilder,
        verdicts: StageVerdictBridge,
        stages: &[PipelineStage],
    ) -> DurableJobRunner {
        let targets = stages
            .iter()
            .map(|s| (s.engine.target.clone(), s.engine.name.clone()))
            .collect();
        DurableJobRunner {
            store,
            rt,
            terms,
            build_spec,
            verdicts,
            targets,
        }
    }

    /// The deterministic `job_queue.job_id` (a `uuid`) for a dispatched stage — derived PURELY from the
    /// engine-minted `idem_token` so a re-dispatch (control-plane replay) re-derives the SAME id and the
    /// `(tenant_id, job_id)` PK collapses it to one row. (The `idem_token` itself — `<run_id>/…:<n>/job`
    /// — is NOT a uuid, so it can not be the durable `job_id` directly; it stays the `jq_idem` key.)
    fn stage_job_id(idem_token: &str) -> String {
        deterministic_uuid(&format!("jobq:{idem_token}"))
    }

    /// **The PURE (DB-free) half of a dispatch** — delegates to [`build_dispatch_parts`] (a free fn so
    /// the SECURITY invariant is unit-testable with NO store/pool at all). Returns the enqueue + the
    /// spec whose `trust_tier` equals `enq.trust_tier` by construction (`co_persist_dispatch` re-asserts).
    fn build_dispatch(
        &self,
        flow_spec: &FlowJobSpec,
    ) -> Result<(DurableEnqueue, SandboxJobSpec), ActivityError> {
        build_dispatch_parts(&self.terms, &self.build_spec, flow_spec)
    }
}

/// **The pure (store-free) dispatch builder — the SECURITY-load-bearing half.** Builds the
/// [`DurableEnqueue`] + the sandbox [`SandboxJobSpec`] the co-persist writes, forwarding the run's
/// `trust_tier` + `region` from `terms` UNCHANGED and STAMPING the SAME tier + the echo `idem_token`
/// onto the spec — so `co_persist_dispatch`'s `enq.trust_tier == spec.trust_tier` gate holds BY
/// CONSTRUCTION and the [`StageSpecBuilder`] can never widen the tier. A free fn (no `self`, no store)
/// so the invariant is provable with zero DB/pool surface.
fn build_dispatch_parts(
    terms: &JobScheduleTerms,
    build_spec: &StageSpecBuilder,
    flow_spec: &FlowJobSpec,
) -> Result<(DurableEnqueue, SandboxJobSpec), ActivityError> {
    // Resolve the stage's executable template (image/command/limits/egress/workspace).
    let mut spec = (build_spec)(flow_spec).map_err(ActivityError)?;

    // SECURITY — stamp the run's REAL trust_tier onto the spec (forwarded UNCHANGED from
    // terms.trust_tier), and the engine-minted idem_token the runner echoes on job.done. So the
    // enqueue + the spec carry the SAME tier by construction — never widened by the builder.
    spec.trust_tier = terms.trust_tier;
    spec.idem_token = IdemToken(flow_spec.idem_token.clone());
    // Belt-and-suspenders: clamp the wall-clock timeout to the store's ceiling so a legitimate stage
    // never trips the fail-closed TimeoutTooLong (the lease-outliving double-run guard).
    if spec.limits.timeout_secs > MAX_JOB_TIMEOUT_SECS {
        spec.limits.timeout_secs = MAX_JOB_TIMEOUT_SECS;
    }

    // The DURABLE enqueue — trust_tier + region FROM the run's terms (forwarded UNCHANGED);
    // idem_token = the engine's dispatch token (the jq_idem key + the job.done echo key).
    let enq = DurableEnqueue {
        tenant_id: terms.tenant_id.clone(),
        region: terms.region.clone(),
        job_id: DurableJobRunner::stage_job_id(&flow_spec.idem_token),
        run_id: terms.run_id.clone(),
        lane: terms.lane,
        labels: terms.labels.clone(),
        trust_tier: terms.trust_tier, // == spec.trust_tier (both terms.trust_tier)
        concurrency_group: terms.concurrency_group.clone(),
        fair_key: terms.fair_key.clone(),
        idem_token: flow_spec.idem_token.clone(),
    };
    Ok((enq, spec))
}

impl JobRunner for DurableJobRunner {
    fn dispatch(&self, flow_spec: &FlowJobSpec) -> Result<(), ActivityError> {
        let (enq, spec) = self.build_dispatch(flow_spec)?;

        // Record the stage name so the reporter re-encodes the verdict codec the body decodes.
        if let Some((_, name)) = self.targets.iter().find(|(t, _)| t == &flow_spec.target) {
            self.verdicts.record(&flow_spec.idem_token, name);
        }

        // Co-persist the job_queue row + the ci_job_spec row in ONE tenant-scoped tx (bridged onto the
        // runtime). A dispatch failure surfaces as an ActivityError the engine retries (reusing the
        // SAME idem_token — the durable ON CONFLICT dedups the re-dispatch).
        bridge(&self.rt, self.store.co_persist_dispatch(&enq, &spec))
            .map_err(|e| ActivityError(format!("durable co_persist_dispatch refused: {e}")))?;
        Ok(())
    }
}

// =================================================================================================
// The verdict-vocabulary bridge reporter.
// =================================================================================================

/// **The [`TerminalReporter`] that bridges the runner's `passed` marker to the pipeline body's stage
/// verdict codec (and WAKES the parked run).** The runner ([`myelin_ci_sandbox::RunnerAgent`]) derives
/// `passed` from the real guest exit code and calls `report_done`; this re-encodes it as the
/// [`stage_verdict_marker`] the body's [`myelin_flow::WfCtx::run_ci_pipeline`] decodes (stage name from
/// the [`StageVerdictBridge`] keyed on the echoed `idem_token`), delivers it through the ONE engine
/// signal path ([`DurableExecutor::signal`] — exactly-once on the `wf_signal` PK), and WAKES the parked
/// run (`waiting → running`) so the dispatcher re-drives + consumes it.
///
/// A `job.done` for a stage the bridge has no mapping for (a non-pipeline compute job, or a lost map
/// after a restart) falls back to the raw `myelin://job-done/passed-<bool>` marker (identical to
/// [`myelin_ci_sandbox::EngineTerminalReporter`]) — the body then surfaces "no verdict marker" LOUDLY,
/// never a fabricated pass.
#[derive(Clone)]
pub struct CiPipelineReporter {
    executor: FlowExecutor,
    tenant: TenantId,
    verdicts: StageVerdictBridge,
}

impl CiPipelineReporter {
    /// Build the reporter over the SHARED [`FlowExecutor`] the parked pipeline run runs on, the cell
    /// tenant (the run + signal partition key), and the shared [`StageVerdictBridge`].
    pub fn new(
        executor: FlowExecutor,
        tenant: TenantId,
        verdicts: StageVerdictBridge,
    ) -> CiPipelineReporter {
        CiPipelineReporter {
            executor,
            tenant,
            verdicts,
        }
    }
}

impl TerminalReporter for CiPipelineReporter {
    fn report_done(
        &self,
        run: &RunId,
        idem_token: &str,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, ExecutorError> {
        // Re-encode the derived `passed` into the stage-verdict marker the pipeline body decodes. The
        // stage name is the one the DurableJobRunner recorded for this dispatch's idem_token.
        let mut payload = Vec::with_capacity(report.result_refs.len() + 1);
        match self.verdicts.stage_for(idem_token) {
            Some(stage) => payload.push(stage_verdict_marker(&stage, report.passed)),
            // Fail-loud fallback (never a fabricated pass): the raw marker the body rejects loudly.
            None => payload.push(ArtifactRef(format!(
                "myelin://job-done/passed-{}",
                report.passed
            ))),
        }
        // references-not-payloads: the guest bytes rode the firehose; only the result refs travel here.
        payload.extend(report.result_refs.iter().cloned());

        let outcome = self.executor.signal(SignalSpec {
            run: run.clone(),
            signal_name: JOB_DONE_SIGNAL.to_string(),
            idem_key: idem_token.to_string(),
            payload,
            payload_key_ref: None,
        })?;

        // WAKE the parked run (waiting → running) so the dispatcher re-leases + replays it and consumes
        // the job.done we just buffered. wake is idempotent (a running/terminal run is untouched); a
        // double delivery buffers once (the PK) and wakes a still-running run harmlessly.
        self.executor.runs().wake(&self.tenant, &run.0);
        Ok(outcome)
    }
}

// =================================================================================================
// Chunk 2 + 3 — the pipeline driver (register + drive the body; start with the pre-minted id).
// =================================================================================================

/// One run's plan the registered body resolves by `run_id`: its [`PipelineRun`] (the ordered stages +
/// the X-1 producer facts) + its [`JobScheduleTerms`] (the security-load-bearing tier/region the
/// durable runner forwards). Populated by [`CiPipelineDriver::start_run`] BEFORE the run is started.
#[derive(Clone)]
struct RunPlan {
    pipeline: PipelineRun,
    terms: JobScheduleTerms,
}

/// **Chunks 2 + 3 — the CI pipeline DRIVER (the same-process engine over the shared executor).** Owns
/// the [`FlowExecutor`] the runner's `job.done` wakes, registers `run_ci_pipeline_body` under
/// [`CI_PIPELINE_WF_TYPE`] (with a per-run [`DurableJobRunner`] injected), and `tick`s a background
/// dispatcher over the SHARED `RunStore`/`SignalStore`. [`start_run`](Self::start_run) reads a durable
/// `ci_run` (queued) row and starts the parked run under the pre-minted `wf_run_id` — so the parked
/// run's id EQUALS the `job_queue` row's `run_id` the runner reports to.
///
/// **Durable-RunStore FLOOR (named):** the engine runs over the IN-MEMORY [`myelin_flow::RunStore`] (the
/// M2 named floor); the durable recovery record is the `ci_run` row (chunk 4). A restart re-reads the
/// queued `ci_run` and re-`start_with_id`s the SAME id (idempotent). A durable `RunStore` is NOT built
/// here.
pub struct CiPipelineDriver {
    executor: FlowExecutor,
    tenant: TenantId,
    region: String,
    // the shared durable-workflow substrate the dispatcher drives over (RunStore/SignalStore come from
    // the executor; the rest are the driver's).
    journal: WfJournal,
    outbox: OutboxStore,
    telemetry: FlowTelemetry,
    timers: TimerStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    // the chunk-5 wiring the registered body composes per run.
    spec_store: CiJobSpecStore,
    rt: tokio::runtime::Handle,
    build_spec: StageSpecBuilder,
    verdicts: StageVerdictBridge,
    // run_id → RunPlan (the per-run pipeline + terms the registered body resolves).
    plans: Arc<Mutex<HashMap<String, RunPlan>>>,
    // the run ids this driver started (so drive_once can wake any parked run robustly).
    started: Arc<Mutex<Vec<String>>>,
}

impl CiPipelineDriver {
    /// Build the driver for a cell `(tenant, region)`. Constructs the shared [`FlowExecutor`] +
    /// registers [`CI_PIPELINE_WF_TYPE`]; the `spec_store` + `rt` + `build_spec` are the chunk-5 durable
    /// dispatch seam the registered body composes.
    pub fn new(
        tenant: TenantId,
        region: impl Into<String>,
        spec_store: CiJobSpecStore,
        rt: tokio::runtime::Handle,
        build_spec: StageSpecBuilder,
        outbox: OutboxStore,
    ) -> CiPipelineDriver {
        let region = region.into();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let executor = FlowExecutor::new(
            minter.clone(),
            tenant.clone(),
            Region(region.clone()),
        );
        executor.register_definition(CI_PIPELINE_WF_TYPE);
        CiPipelineDriver {
            executor,
            tenant: tenant.clone(),
            region: region.clone(),
            journal: WfJournal::new(),
            outbox,
            telemetry: FlowTelemetry::new(),
            timers: TimerStore::new(),
            minter,
            ctx_base: service_ctx_base(&tenant, &region),
            spec_store,
            rt,
            build_spec,
            verdicts: StageVerdictBridge::new(),
            plans: Arc::new(Mutex::new(HashMap::new())),
            started: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The SHARED [`FlowExecutor`] the parked pipeline runs on — the runner loop's reporter signals
    /// THIS executor (one signal path). A cloneable handle (shared `Arc<Mutex<…>>` state).
    pub fn executor(&self) -> FlowExecutor {
        self.executor.clone()
    }

    /// The shared [`StageVerdictBridge`] the runner's reporter reads (the durable runner writes it).
    pub fn verdict_bridge(&self) -> StageVerdictBridge {
        self.verdicts.clone()
    }

    /// Build the [`CiPipelineReporter`] the runner loop drives (over this driver's shared executor +
    /// verdict bridge). The runner's `job.done` re-encodes to the stage verdict + wakes the parked run.
    pub fn reporter(&self) -> CiPipelineReporter {
        CiPipelineReporter::new(
            self.executor.clone(),
            self.tenant.clone(),
            self.verdicts.clone(),
        )
    }

    /// The outbox the pipeline body's X-1 producer emits (`ci.run.succeeded` / `ci.check.updated` /
    /// `ci.result`) co-commit into. Shared so a test/driver can read the emitted terminal facts.
    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    /// **Chunk 3 — start the parked `ci.pipeline` run under the pre-minted `wf_run_id`.** Registers the
    /// run's [`RunPlan`] (so the registered body resolves it by `run_id`), then calls
    /// [`DurableExecutor::start_with_id`] with `Some(RunId(record.wf_run_id))`. Idempotent on the
    /// `idem_key` (`ci:<run_id>`): a re-drive (a restart re-reading the queued `ci_run`) returns the
    /// EXISTING run — never a second run. `record.trust_tier` / `record.region` are forwarded UNCHANGED
    /// into the run's [`JobScheduleTerms`] (the durable runner's security-load-bearing source).
    ///
    /// `labels` are the runner-affinity labels the stage jobs require (a job is claimable iff
    /// `labels ⊆ runner_labels`) — from the resolved snapshot (the CT-004d follow-on); the caller
    /// supplies them here.
    pub fn start_run(
        &self,
        record: &CiRunRecord,
        pipeline: PipelineRun,
        labels: Vec<String>,
    ) -> Result<RunId, StartRunError> {
        validate_driver_tenant(&self.tenant, record)?;
        // Forward the run's STAMPED trust tier UNCHANGED (parse the ci_run.trust_tier CHECK token). A
        // corrupt token is a loud refusal — never a silent widen/default.
        let trust_tier = trust_from_token(&record.trust_tier).map_err(StartRunError::TrustTier)?;
        let terms = JobScheduleTerms {
            tenant_id: record.tenant_id.clone(),
            region: record.region.clone(),
            run_id: record.wf_run_id.clone(),
            lane: Lane::Interactive, // a PR/push CI check is the interactive lane (arch 02 §2.3)
            labels,
            trust_tier,
            concurrency_group: None,
            fair_key: record.tenant_id.clone(),
        };
        self.plans.lock().unwrap_or_else(|e| e.into_inner()).insert(
            record.wf_run_id.clone(),
            RunPlan {
                pipeline,
                terms,
            },
        );
        {
            let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
            if !started.contains(&record.wf_run_id) {
                started.push(record.wf_run_id.clone());
            }
        }
        self.executor
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: vec![],
                    budget: None,
                    idem_key: format!("ci:{}", record.run_id),
                },
                Some(RunId(record.wf_run_id.clone())),
            )
            .map_err(StartRunError::Start)
    }

    /// The registered `ci.pipeline` body: resolve the run's [`RunPlan`] by `run_id`, build a per-run
    /// [`DurableJobRunner`] (chunk 5), and drive [`run_ci_pipeline_body`] (which dispatches each stage
    /// through the durable queue + emits the X-1 producer facts). The body is FLOW-DETERMINISTIC: the
    /// plan/terms are fixed at start (no clock/RNG/IO), the dispatch rides the journaled activity, the
    /// verdict rides the journaled `job.done`.
    fn body(&self) -> Box<WorkflowBody> {
        let plans = self.plans.clone();
        let spec_store = self.spec_store.clone();
        let rt = self.rt.clone();
        let build_spec = self.build_spec.clone();
        let verdicts = self.verdicts.clone();
        Box::new(move |ctx: &mut WfCtx| {
            let run_id = ctx.run_id().to_string();
            let plan = plans
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&run_id)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "no PipelineRun registered for ci.pipeline run `{run_id}` — the starter must \
                         register the plan before start_with_id (CT-004d.2 chunk 3)"
                    )
                })?;
            let runner = DurableJobRunner::new(
                spec_store.clone(),
                rt.clone(),
                plan.terms.clone(),
                build_spec.clone(),
                verdicts.clone(),
                &plan.pipeline.stages,
            );
            let verdict =
                run_ci_pipeline_body(ctx, &plan.pipeline, &runner).map_err(|e| format!("{e:?}"))?;
            Ok(match verdict {
                RunVerdict::Succeeded { stages_completed } => {
                    vec![ArtifactRef(format!("outcome:succeeded:{stages_completed}"))]
                }
                RunVerdict::Failed { stage } => vec![ArtifactRef(format!("outcome:failed:{stage}"))],
                RunVerdict::Rejected { stage } => {
                    vec![ArtifactRef(format!("outcome:rejected:{stage}"))]
                }
                RunVerdict::Parked => vec![],
            })
        })
    }

    /// Build a fresh [`FlowDispatcher`] over the SHARED substrate for a partition (the dogfood
    /// per-tick-worker shape). The `RunStore`/`SignalStore` are the executor's (so a `start_with_id`
    /// seeds a run this dispatcher leases + drives, and the runner's `job.done` signal is the one this
    /// consumes); the journal/timers/outbox/telemetry are the driver's persistent shared handles.
    fn dispatcher(&self, partition: i16) -> FlowDispatcher {
        let mut disp = FlowDispatcher::new(
            self.executor.runs().clone(),
            self.outbox.clone(),
            self.journal.clone(),
            self.telemetry.clone(),
            self.minter.clone(),
            self.ctx_base.clone(),
            partition,
            "ci-pipeline-driver",
            30,
        )
        .with_signals(self.executor.signals().clone())
        .with_timers(self.timers.clone());
        disp.register(CI_PIPELINE_WF_TYPE, self.body());
        disp
    }

    /// **One drive pass: wake every started run, then `tick` every partition.** The wake is the robust
    /// re-drive (idempotent — it only flips `waiting → running`): a run with no new `job.done` replays
    /// to its park point and re-parks (cheap); a run whose `job.done` arrived advances. This closes the
    /// report-before-park race the one-shot reporter wake alone could miss. Each `tick` leases + drives
    /// at most one runnable run per partition; the driver loop calls this repeatedly. Returns the
    /// non-idle drive outcomes this pass observed (a test reads `Completed`/`Failed`; the loop ignores).
    pub fn drive_once(&self, now: i64, now_clock: &str) -> Vec<DriveOutcome> {
        for run_id in self
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            self.executor.runs().wake(&self.tenant, run_id);
        }
        let mut outcomes = Vec::new();
        for p in 0..PARTITION_COUNT as i16 {
            let disp = self.dispatcher(p);
            if let Some(o) = disp.tick(now, now_clock, 7) {
                outcomes.push(o);
            }
        }
        outcomes
    }

    /// Whether a started run has reached a TERMINAL engine state (completed/failed/terminated/
    /// nondeterministic) — the `ci_run` is the thin index over this myelin-flow run (arch 01 §3.1).
    /// `None` for an unknown run.
    pub fn is_terminal(&self, run: &RunId) -> Option<bool> {
        self.executor
            .describe(run)
            .ok()
            .map(|status| status.terminal)
    }

    /// The engine `state` of a started run (running/waiting/completed/failed/…), for the driver loop /
    /// a test to poll. `None` for an unknown run.
    pub fn run_state(&self, run: &RunId) -> Option<String> {
        self.executor.describe(run).ok().map(|s| s.state)
    }

    /// The cell region this driver runs in.
    pub fn region(&self) -> &str {
        &self.region
    }
}

/// Why [`CiPipelineDriver::start_run`] refused — a corrupt stamped trust token, or an executor start
/// failure (unknown workflow / a pre-minted-id collision with a DIFFERENT run). Surfaced, never swallowed.
#[derive(Debug)]
pub enum StartRunError {
    /// The durable run belongs to a different tenant than this per-tenant driver. Refused before a
    /// plan is registered or an engine run/job is created, so a region-wide starter cannot stamp
    /// one tenant's authority or fair-queue key onto another tenant's run.
    TenantMismatch {
        /// Tenant this driver was composed for.
        driver_tenant: String,
        /// Authoritative tenant read from `ci_run.tenant_id`.
        record_tenant: String,
    },
    /// The `ci_run.trust_tier` token was outside the frozen CHECK vocabulary (a corrupt run-of-record) —
    /// refused loudly rather than defaulting the tier the durable dispatch gates on.
    TrustTier(JobQueueStoreError),
    /// The executor `start_with_id` failed (unknown workflow, or a pre-minted `wf_run_id` collision with
    /// a DIFFERENT run — fail-closed, never a silent clobber).
    Start(ExecutorError),
}

impl std::fmt::Display for StartRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartRunError::TenantMismatch {
                driver_tenant,
                record_tenant,
            } => write!(
                f,
                "ci.pipeline start refused: driver tenant `{driver_tenant}` does not match durable ci_run tenant `{record_tenant}`"
            ),
            StartRunError::TrustTier(e) => {
                write!(f, "ci.pipeline start refused: corrupt trust_tier token: {e}")
            }
            StartRunError::Start(e) => write!(f, "ci.pipeline start_with_id failed: {e}"),
        }
    }
}

impl std::error::Error for StartRunError {}

/// Enforce the per-tenant driver boundary before any mutable in-memory/durable orchestration state
/// is touched. A future region-wide queued-run poller must route each record to a driver composed for
/// exactly this authoritative tenant; it may never reuse a synthetic service tenant.
fn validate_driver_tenant(
    driver_tenant: &TenantId,
    record: &CiRunRecord,
) -> Result<(), StartRunError> {
    if driver_tenant.0 == record.tenant_id {
        Ok(())
    } else {
        Err(StartRunError::TenantMismatch {
            driver_tenant: driver_tenant.0.clone(),
            record_tenant: record.tenant_id.clone(),
        })
    }
}

// =================================================================================================
// Helpers.
// =================================================================================================

/// The service emit context the driver's dispatcher stamps onto the co-committed X-1 producer events
/// (`ci.run.succeeded` / `ci.check.updated` / `ci.result`). A platform-service principal (no PII), the
/// cell `(tenant, region)`. The deterministic timestamps keep the body replay-stable (the body reads no
/// clock outside `WfCtx`).
fn service_ctx_base(tenant: &TenantId, region: &str) -> EmitContextBase {
    EmitContextBase {
        tenant: tenant.clone(),
        region: Region(region.to_string()),
        actor: Actor(Principal::stub(
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            tenant.clone(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-07-17T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-17T00:00:00Z".into()),
        caused_by: None,
    }
}

/// **A deterministic uuid-shaped string from a seed (2×-salted FNV-1a fill).** Mirrors
/// `myelin_ci_dispatch::deterministic_uuid` (the leaf crate can not be a dependency of this one) so a
/// re-dispatch derives the SAME `job_queue.job_id` (the `(tenant_id, job_id)` PK idempotency anchor).
/// Non-cryptographic — it keys a DEDUP boundary (a collision would merge two stages' durable rows), not
/// an auth boundary; the trust gate is the forwarded `trust_tier`, not this id.
fn deterministic_uuid(seed: &str) -> String {
    let fill = |salt: u64| -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
        for b in seed.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let a = fill(0);
    let b = fill(0x00ff_00ff_00ff_00ff);
    let bytes = [a.to_be_bytes(), b.to_be_bytes()].concat();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
        bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// **A digest-pinned compute [`SandboxJobSpec`] builder for a fixed `command` (the CT-004d.2 test
/// seam and minimal production default).** Produces a `kind=ci` spec running `command` in a `runsc` guest,
/// default-deny egress, a read-only workspace. The `trust_tier` + `idem_token` are placeholders the
/// [`DurableJobRunner`] OVERWRITES from the run's terms + the dispatch (so this builder can never widen
/// the tier). `image` MUST be digest-pinned (fail-closed via [`ImageRef::pinned`]).
pub fn fixed_command_spec_builder(
    image: &str,
    command: Vec<String>,
    timeout_secs: u32,
) -> Result<StageSpecBuilder, String> {
    let image = ImageRef::pinned(image).map_err(|e| e.to_string())?;
    Ok(Arc::new(move |_flow_spec: &FlowJobSpec| {
        SandboxJobSpec::new(
            SandboxJobKind::Ci,
            image.clone(),
            command.clone(),
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1 << 30,
                pids_max: 128,
                timeout_secs,
            },
            WorkspaceSpec::default(),
            // placeholders — DurableJobRunner::dispatch overwrites both from the run's terms + dispatch.
            TrustTier::Trusted,
            RunTokenRef {
                jti: "ci-pipeline-driver-jti".into(),
            },
            MeterTarget {
                reserve_id: "ci-pipeline-driver-reserve".into(),
            },
            IdemToken(String::new()),
        )
        .map_err(|e| e.to_string())
    }))
}

#[cfg(test)]
#[path = "ci_pipeline_driver_tests.rs"]
mod tests;
