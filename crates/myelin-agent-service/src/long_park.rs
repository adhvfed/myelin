//! # `long_park` — the `SCHEDULE_AND_RUN_JOB` long-park idiom CONSUMED by the Agent-Fabric
//! (AG-P16 → P-228, M2-C)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §5.6 (*a run is a durable
//! workflow + the `SCHEDULE_AND_RUN_JOB` long-park idiom (C5)*): a long sandbox job (a `compute`
//! tool whose CI run takes minutes-to-hours) dispatches the `kind=agent` job (reserve at dispatch —
//! 11.7), the run **PARKS holding no runtime**, and completion arrives **hours later** as a durable
//! signal `signal(run, "job.done", {result}, idem_key = idem_token)` idempotent on `idem_token` (the
//! runner can deliver "done" twice; the workflow wakes ONCE). On wake after a long park, the per-run
//! token is RE-MINTED (the §5.7 C6 re-mint-on-resume, AG-P13). **The Fabric CONSUMES this idiom; it
//! does not reinvent durable waits** (§5.6 / TE-20 build-vs-adopt).
//!
//! **Contract-index:** CONSUMES 9.2 (`WfCtx::schedule_and_run_job` — the long-park idiom),
//! 9.4 (the `job.done` durable signal, idempotent on `idem_token`), 11.7 (reserve at dispatch within
//! the idiom — the metered form), 4.7 (`mint_run_token` re-mint on resume). The ENGINE half (9.2/9.4)
//! lives in [`myelin_flow::job`] (P-FLOW-15 → P-211); this is the AGENT-FABRIC consumer that drives a
//! long `kind=agent` `ToolHands::exec` job through it.
//!
//! ## What this prompt (AG-P16) ships — the Fabric's long-park CONSUMER (NO new engine, NO new table)
//!
//! The idiom is **already built in the engine** ([`WfCtx::schedule_and_run_job`], composing the
//! activity / durable-signal / durable-timer primitives). AG-P15 (→ P-226, [`crate::exec`]) shipped
//! the *in-line activity form* of `ToolHands::exec` (a `compute` job that runs synchronously within
//! one activity). **AG-P16 is the LONG-PARK form**: the SAME hardened `kind=agent` job
//! ([`crate::exec::SandboxJob`]) is handed to the engine's `schedule_and_run_job` so a job that takes
//! HOURS dispatches-and-returns (the worker is freed, the run holds no runtime) and resumes on the
//! durable `job.done` signal. The Fabric supplies:
//!
//! - [`AgentJobDispatcher`] — the [`JobRunner`] (contract-8.4 `ToolHands::exec` seam, the engine's
//!   dispatch TARGET) that hands a long `kind=agent` [`crate::exec::SandboxJob`] to the unified
//!   sandbox backend for ASYNCHRONOUS execution (it dispatches and returns; the runner runs the job
//!   for however long it takes and later delivers `job.done`). It reuses [`crate::exec`]'s routing
//!   split (only a `compute` tool builds a job) and the four-guarantee hardening — a long-park job is
//!   the SAME hardened spec, just completed-by-signal instead of in-line.
//! - [`dispatch_long_compute`] — the Fabric entry point: build the hardened long `kind=agent` job for
//!   a `compute` `ToolDef` + `Command`, then drive the engine's [`WfCtx::schedule_and_run_job`]
//!   (dispatch-and-return + park-on-`job.done` + idempotent completion). On wake (a buffered/arriving
//!   `job.done`, or a timeout) the engine's wait-resume leg RE-MINTS the per-run token (if a
//!   [`RunTokenLease`] is wired, §6.2 — the long-park resume IS a wait resume, so the engine owns the
//!   one re-mint; no double-mint).
//! - [`LongParkOutcome`] — the Fabric's view of a long-park: `Completed` (the `job.done` arrived,
//!   consumed exactly once), `Parked` (dispatched, the run waits holding no runtime — the body returns
//!   promptly), or `TimedOut` (the runner vanished and the SLA timer fired). A thin projection of the
//!   engine's [`JobOutcome`] so the Fabric never re-exposes the engine's internal shapes raw.
//!
//! ## The gate this prompt proves (§5.6 / AG-D-long-park)
//! 1. **A doubly-delivered `job.done` wakes the run EXACTLY ONCE** (idempotent on `idem_token`) — the
//!    runner delivers "done" twice (at-least-once under the bus); the run wakes once, the result is
//!    consumed once.
//! 2. **The parked run holds NO runtime** — between dispatch and `job.done` the run is
//!    `state='waiting'`; the worker is free (it dispatched-and-returned). A long compute costs storage
//!    not compute while parked (VISION §3).
//! 3. **Re-mint on wake after a long park** — the resumed run runs under a FRESH short-lived per-run
//!    token (token life == activity life, never the days-long workflow life, §5.7 C6 / AG-P13).
//!
//! ## FLOOR named — NONE. The idiom is CONSUMED from the durable-workflow engine, not reinvented.
//! The engine's [`WfCtx::schedule_and_run_job`] (9.2/9.4) owns the dispatch/park/idempotent-completion
//! mechanics + the reserve/settle bookend (the metered form) + the resume-leg re-mint hook; this
//! module is the Fabric CONSUMER that points a long `kind=agent` `ToolHands::exec` job at it. The real
//! sandbox backend (the Firecracker microVM, CI-P2 → P-237) + the ZERO-escapes real-kernel GATE
//! (AG-P17 → P-229 / CI-P5) are the runner IMPL/GATE follow-ons RECORDED in [`crate::exec`], not owned
//! here; the real `LlmAgentRuntime` dispatching long compute is post-M5 (AG-P25).
//!
//! ## DB-free
//! This module touches NO DB / object-store / cache / bus contract directly: it composes the engine's
//! [`WfCtx`] (proven against the live stack at the durable-workflow tier, P-FLOW-15/16) and the
//! in-memory [`SandboxBackend`] dispatch seam ([`crate::exec`], proven at AG-P15). `cargo build
//! --workspace` stays DB-free; no new `integration` feature here (no new data-layer contract is
//! crossed — recorded in the P-228 report).

use crate::escape_gate::AgentExecGate;
use crate::exec::{RoutingError, SandboxJob};
use myelin_agent::{Command, ToolDef};
use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, MeterTarget, ResourceLimits, RunTokenCredential,
    SandboxBackend, SecretRef, TrustTier,
};
use myelin_flow::{JobKind, JobOutcome, JobRunner, JobSpec, WfCtx, WfResult};
use myelin_refs::ArtifactRef;

/// **The Fabric's view of a long-park `SCHEDULE_AND_RUN_JOB` outcome (§5.6).** A thin projection of
/// the engine's [`JobOutcome`] so the Agent-Fabric never re-exposes the engine's internal shape raw:
/// a long compute either COMPLETED (the `job.done` arrived hours later, consumed exactly once),
/// PARKED (dispatched, the run waits holding NO runtime — the body returns promptly), or TIMED OUT
/// (the runner vanished and the SLA timer bounded the wait).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LongParkOutcome {
    /// **The long compute COMPLETED — the `job.done` signal arrived and was CONSUMED exactly once.**
    /// Carries the runner-echoed `idem_token` (= the dispatch token — the no-coordination dedup
    /// agreement held, §4.9) and the job's references-not-payloads result refs (the trace, never a
    /// PII body). A double-delivered `job.done` produces this ONCE (the engine's `wf_signal` PK dedup).
    Completed {
        /// the `idem_token` the runner echoed (= the minted dispatch token).
        idem_token: String,
        /// the job's result refs (references-not-payloads, §3.4).
        result: Vec<ArtifactRef>,
    },
    /// **The job is DISPATCHED and the run PARKED on `job.done` (holds NO runtime).** The run is
    /// `state='waiting'` until the runner delivers `signal(run, "job.done", …)` — possibly HOURS
    /// later. The body should RETURN promptly on a `Parked` (it made no progress past the wait); the
    /// dispatcher re-drives the run when the signal arrives.
    Parked,
    /// **The job TIMED OUT — the SLA timer fired before the runner reported (a vanished-runner bound).**
    /// A runner that vanished does NOT park the run forever; the body retries / compensates / dequeues.
    TimedOut,
}

impl LongParkOutcome {
    /// Whether the long compute completed (the `job.done` was consumed exactly once).
    pub fn is_completed(&self) -> bool {
        matches!(self, LongParkOutcome::Completed { .. })
    }
    /// Whether the run parked holding no runtime (dispatched, waiting on `job.done`).
    pub fn is_parked(&self) -> bool {
        matches!(self, LongParkOutcome::Parked)
    }
    /// Whether the wait timed out (the runner vanished; the SLA timer bounded it).
    pub fn is_timed_out(&self) -> bool {
        matches!(self, LongParkOutcome::TimedOut)
    }

    /// Project the engine's [`JobOutcome`] (9.2/9.4) into the Fabric's [`LongParkOutcome`]. A pure
    /// re-tag — the Fabric consumes the engine's outcome, it does not re-derive completion.
    fn from_job_outcome(out: JobOutcome) -> LongParkOutcome {
        match out {
            JobOutcome::Completed { idem_token, result } => {
                LongParkOutcome::Completed { idem_token, result }
            }
            JobOutcome::Parked => LongParkOutcome::Parked,
            JobOutcome::TimedOut => LongParkOutcome::TimedOut,
        }
    }
}

/// **The Agent-Fabric [`JobRunner`] — the dispatch TARGET the engine's `schedule_and_run_job` hands
/// the long `kind=agent` [`JobSpec`] to (contract 8.4 CONSUMED, §5.6/§4.9).** It is the long-park
/// twin of [`crate::exec::SandboxToolHands`]: where the in-line form (`dispatch_compute`) BLOCKS on
/// the launch + whole-guest kill within ONE activity, this form DISPATCHES the hardened job onto the
/// unified-sandbox backend for ASYNCHRONOUS execution and RETURNS immediately (the worker is freed;
/// the run parks; the runner runs the job for however long it takes and LATER delivers
/// `signal(run, "job.done", …, idem_key = idem_token)`).
///
/// **The four uniform guarantees ride the SAME hardened spec** — a long-park job is built by the
/// SAME [`crate::exec::SandboxJob::for_compute`] (the routing split: only `compute` builds a job; the
/// digest-pin / pids.max / timeout / egress / token-scrub hardening), it is just completed-by-signal
/// instead of in-line. There is NO second hardening profile and NO host-exec bypass.
///
/// **GATED BY AG-D4 — STRUCTURALLY (AG-P17 → P-229).** The real backend executes untrusted code in
/// the kernel sandbox, so this dispatcher carries an [`AgentExecGate`] — a value obtainable ONLY from
/// a GREEN [`EscapeAttestation`](myelin_ci_sandbox::EscapeAttestation) for the production backend. The
/// dispatcher has no constructor without it (mirroring [`crate::exec::SandboxToolHands`]), so the
/// long-park dispatch is fail-closed on AG-D4 exactly like the in-line `exec` form: no green
/// attestation ⇒ no `AgentJobDispatcher` ⇒ no untrusted compute. The backend is CI's
/// (`myelin-ci-sandbox`); the Fabric feeds it the hardened spec.
pub struct AgentJobDispatcher<'a, B: SandboxBackend> {
    /// The AG-D4 / CI-T1 escape gate (AG-P17 → P-229) — its existence is the proof a green escape
    /// attestation for the production backend was consumed (fail-closed: no green ⇒ no dispatcher).
    gate: AgentExecGate,
    /// The unified-sandbox backend (CI owns it; the Fabric feeds the hardened `kind=agent` spec).
    backend: &'a B,
    /// The pre-built HARDENED `kind=agent` job (the four-guarantee profile, AG-P15). The engine's
    /// `JobRunner::dispatch` hands an OPAQUE engine [`JobSpec`] (a dispatch descriptor + the
    /// deterministic idem_token); the Fabric dispatches THIS hardened sandbox spec onto the backend.
    /// They are bound together at [`dispatch_long_compute`] (the engine target encodes the hardened
    /// job's idem token, so the two refer to the SAME job).
    job: SandboxJob,
}

impl<'a, B: SandboxBackend> AgentJobDispatcher<'a, B> {
    /// Build the long-park dispatcher over the unified-sandbox `backend` for the hardened `job` (the
    /// `kind=agent` four-guarantee spec). Usually constructed by [`dispatch_long_compute`], which
    /// builds the hardened job from a `compute` `ToolDef`+`Command` and binds it here.
    ///
    /// **The AG-D4 `gate` is a REQUIRED argument** — there is no constructor without it. The
    /// dispatcher cannot exist (and therefore cannot hand untrusted compute to the backend) unless the
    /// caller holds a GREEN [`AgentExecGate`] for the production backend (the structural fail-closed,
    /// AG-P17 → P-229; mirrors [`crate::exec::SandboxToolHands::new`]).
    pub fn new(gate: AgentExecGate, backend: &'a B, job: SandboxJob) -> AgentJobDispatcher<'a, B> {
        AgentJobDispatcher { gate, backend, job }
    }

    /// The hardened `kind=agent` job this dispatcher async-dispatches (read-only view).
    pub fn job(&self) -> &SandboxJob {
        &self.job
    }

    /// The AG-D4 / CI-T1 escape gate this dispatcher runs under (read-only). Its existence is the
    /// proof a green escape attestation for the production backend was consumed (AG-P17 → P-229).
    pub fn gate(&self) -> &AgentExecGate {
        &self.gate
    }
}

impl<B: SandboxBackend> JobRunner for AgentJobDispatcher<'_, B> {
    /// Hand the HARDENED `kind=agent` sandbox job to the unified sandbox for ASYNCHRONOUS execution.
    /// Returns `Ok(())` on a dispatch the runner ACCEPTED (the completion arrives LATER as the
    /// `job.done` signal the engine parks on), or an [`myelin_flow::ActivityError`] if the dispatch
    /// itself failed (the engine retries it, reusing the SAME deterministic `idem_token` — the runner
    /// dedups a re-dispatched job on it, §4.9).
    ///
    /// The engine's opaque [`JobSpec`] argument (kind ∈ {Ci, Agent}, an opaque `target` + the
    /// `idem_token`) is the engine's dispatch CONTRACT; the Fabric's HARDENED
    /// [`crate::exec::SandboxJob`] (the four-guarantee profile) is what actually runs — they refer to
    /// the SAME job (the engine target encodes the hardened job's idem token, bound at
    /// [`dispatch_long_compute`]). A `schedule_and_run_job` for a `kind=agent` job routes through THIS
    /// runner; a `kind=ci` job is CI's own merge-queue runner (§5.6 — the SAME idiom, a different
    /// dispatch target).
    fn dispatch(&self, spec: &JobSpec) -> Result<(), myelin_flow::ActivityError> {
        debug_assert_eq!(
            spec.kind,
            JobKind::Agent,
            "the Agent-Fabric dispatcher accepts only kind=agent jobs (a kind=ci job is CI's own \
             merge-queue runner — the SAME §5.6 idiom, a different dispatch target)"
        );
        // Re-stamp the engine's DETERMINISTIC dispatch idem_token (minted on the dispatch position)
        // onto the hardened spec, so the runner echoes THAT token on `job.done` — the no-coordination
        // dedup key the workflow keys its `wait_for_signal` on (§4.9). The four-guarantee profile is
        // untouched; only the dedup token is rebound to the engine's deterministic one.
        let dispatched = self
            .job
            .clone()
            .with_dispatch_idem_token(IdemToken(spec.idem_token.clone()));
        // The async-dispatch seam: the backend ACCEPTS the HARDENED sandbox job for execution and
        // returns — it does NOT block on completion (the run holds no runtime while the multi-hour job
        // runs). The real backend enqueues the guest and reports `job.done` later; the in-memory shape
        // stub records the acceptance. A dispatch failure (the runner is unreachable / rejected the
        // spec) surfaces LOUD as an ActivityError so the engine retries it on the SAME idem_token
        // (never a silent drop — EI-01 §2).
        self.backend
            .accept_async(dispatched.spec())
            .map_err(|e| myelin_flow::ActivityError(format!("async dispatch refused: {e}")))
    }
}

/// **The Fabric entry point: dispatch a long `compute` job as a `SCHEDULE_AND_RUN_JOB` long-park
/// (§5.6 — the idiom CONSUMED).** Builds the hardened `kind=agent` [`crate::exec::SandboxJob`] for the
/// `compute` `def` + `cmd` (the routing split: a non-`compute` tool is REFUSED LOUD — it has no path
/// to the sandbox), then drives the engine's [`WfCtx::schedule_and_run_job`]:
///
/// 1. **Dispatch-and-return.** The engine mints the deterministic `idem_token` (on the dispatch
///    position), stamps it on the engine's [`JobSpec`], and hands it to [`AgentJobDispatcher`] (which
///    accepts the hardened job onto the backend asynchronously and RETURNS). The worker is freed.
/// 2. **Park.** The run `wait_for_signal("job.done", idem_key = idem_token)` with the optional
///    `timeout_secs` SLA — `state='waiting'`, holds NO runtime, for however long the job runs.
/// 3. **Idempotent completion.** When the runner delivers `signal(run, "job.done", {result},
///    idem_key = idem_token)` (possibly TWICE — at-least-once), the engine consumes it EXACTLY once
///    (the `wf_signal` PK dedup) and resumes. The resume leg RE-MINTS the per-run token (if a
///    [`RunTokenLease`](myelin_flow::RunTokenLease) is wired via
///    [`WfCtx::with_run_identity`](myelin_flow::WfCtx::with_run_identity), §5.7 C6 — the long-park
///    resume IS a wait resume, so the engine owns the ONE re-mint; no double-mint).
///
/// **Reserve at dispatch (11.7):** when the `WfCtx` is metered
/// ([`WfCtx::with_budget`](myelin_flow::WfCtx::with_budget)) use [`dispatch_long_compute_metered`]
/// instead — it fronts the dispatch with the reserve/settle bookend (no balance → the job is NEVER
/// handed to the runner). The bare form here is the un-metered long-park (the loop-cap depth is the
/// runaway bound, AG-6).
///
/// Returns the hardened-job build error ([`RoutingError`]) if the tool is not `compute` / the spec is
/// not fail-closed buildable, or the engine [`WfResult`] of the long-park (the [`LongParkOutcome`]).
#[allow(clippy::too_many_arguments)]
pub fn dispatch_long_compute<B: SandboxBackend>(
    ctx: &mut WfCtx,
    gate: AgentExecGate,
    backend: &B,
    def: &ToolDef,
    cmd: &Command,
    profile: LongComputeProfile,
    timeout_secs: Option<i64>,
) -> Result<WfResult<LongParkOutcome>, RoutingError> {
    // Build the HARDENED kind=agent job (the routing split + the four-guarantee profile). A
    // non-`compute` tool is REFUSED LOUD here — it has no path to the sandbox (the type-level safety
    // boundary, AG-P15). The hardened SandboxJob carries the Fabric's own dispatch idem token (the
    // backend dedups a re-dispatch on it); the engine ALSO mints its OWN deterministic idem_token for
    // the `job.done` wait — both are carried, bound through `long_job_target`. The AG-D4 `gate`
    // (AG-P17) gates the dispatcher fail-closed: no green attestation ⇒ no dispatcher.
    let job = build_long_job(def, cmd, &profile)?;
    let target = long_job_target(&job);
    let dispatcher = AgentJobDispatcher::new(gate, backend, job);

    // Drive the engine's long-park idiom (9.2/9.4): dispatch-and-return + park-on-job.done +
    // idempotent completion. The engine owns the deterministic idem_token, the parking, the
    // double-delivery dedup, and the resume-leg re-mint. We CONSUME it — we do not reinvent it.
    let engine_spec = JobSpec::new(JobKind::Agent, target);
    Ok(ctx
        .schedule_and_run_job(engine_spec, &dispatcher, timeout_secs)
        .map(LongParkOutcome::from_job_outcome))
}

/// **The metered long-park form (§5.6 / 11.7 reserve at dispatch).** The same idiom as
/// [`dispatch_long_compute`], FRONTED by the reserve/settle bookend: it reserves `cost` minor-units
/// at dispatch (no balance → the job is NEVER handed to the runner) and settles the actual `units` on
/// the consumed `job.done`. An in-flight parked job is NEVER interrupted (its reservation stays
/// in-flight across the park and settles only on a later drive's completion). Delegates to the
/// engine's [`WfCtx::metered_schedule_and_run_job`] — the Fabric consumes the bookend, it does not
/// re-meter.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_long_compute_metered<B: SandboxBackend>(
    ctx: &mut WfCtx,
    gate: AgentExecGate,
    backend: &B,
    def: &ToolDef,
    cmd: &Command,
    profile: LongComputeProfile,
    timeout_secs: Option<i64>,
    cost: myelin_storage::reserve_settle::MinorUnits,
    units: Vec<myelin_storage::reserve_settle::MeteredUnit>,
) -> Result<WfResult<LongParkOutcome>, RoutingError> {
    let job = build_long_job(def, cmd, &profile)?;
    let target = long_job_target(&job);
    let dispatcher = AgentJobDispatcher::new(gate, backend, job);
    let engine_spec = JobSpec::new(JobKind::Agent, target);
    Ok(ctx
        .metered_schedule_and_run_job(engine_spec, &dispatcher, timeout_secs, cost, units)
        .map(LongParkOutcome::from_job_outcome))
}

/// **The hardened `compute` profile a long-park job is built under (the four-guarantee carrier).**
/// The SAME hardening [`crate::exec::SandboxJob::for_compute`] applies to an in-line `compute` job —
/// a long-park job is not "less hardened" because it runs for hours; if anything the isolation floor
/// matters MORE. Bundles the per-run profile so [`dispatch_long_compute`] takes one argument, not ten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LongComputeProfile {
    /// The digest-pinned hardened image (an un-digested tag fails-closed at build).
    pub image: ImageRef,
    /// The untrusted command the long compute runs (a multi-hour test/build/script).
    pub command: Vec<String>,
    /// The in-boundary secret refs (names/handles, resolved inside the boundary — never the clear
    /// material).
    pub secret_refs: Vec<SecretRef>,
    /// The egress policy (default-deny unless the run opts in).
    pub egress: EgressPolicy,
    /// The resource limits (pids_max + timeout > 0; zero swap structural).
    pub limits: ResourceLimits,
    /// The run's trust tier (gates secrets/cache/egress; X-1).
    pub trust_tier: TrustTier,
    /// The per-run attenuated token (guarantee #2; minted at dispatch, re-minted on resume).
    pub run_token: RunTokenCredential,
    /// The reserve this run settles against (guarantee #1; reserved at dispatch).
    pub meter_to: MeterTarget,
    /// The dispatch idempotency token stamped on the hardened job (the backend dedups a re-dispatch
    /// on it; the engine ALSO mints its own deterministic idem_token for the `job.done` wait — both
    /// are carried).
    pub idem_token: IdemToken,
}

/// Build the hardened long `kind=agent` [`crate::exec::SandboxJob`] for a `compute` `def`+`cmd` under
/// the `profile`. Reuses [`crate::exec::SandboxJob::for_compute`] (the routing split + the
/// fail-closed hardening) — a long-park job is the SAME hardened spec as an in-line one.
fn build_long_job(
    def: &ToolDef,
    cmd: &Command,
    profile: &LongComputeProfile,
) -> Result<SandboxJob, RoutingError> {
    // The command is the profile's command (a multi-hour script) with the brain's `cmd` appended (the
    // specific compute call) — the loop builds the profile once per run and supplies the per-call cmd.
    let mut command = profile.command.clone();
    command.push(cmd.0.clone());
    SandboxJob::for_compute(
        def,
        profile.image.clone(),
        command,
        Vec::new(),
        profile.secret_refs.clone(),
        profile.egress.clone(),
        profile.limits,
        profile.trust_tier,
        profile.run_token.clone(),
        profile.meter_to.clone(),
        profile.idem_token.clone(),
    )
}

/// **The opaque engine-`JobSpec` target for a hardened long-park job (references-not-payloads).** The
/// engine carries a `kind=agent` job as an opaque `target` (a job descriptor, never an inline PII
/// body); the Fabric encodes the hardened job's dispatch identity (its idem token) so the dispatcher
/// can resolve the hardened spec to launch. No PII — a machine handle naming WHICH agent job to run.
fn long_job_target(job: &SandboxJob) -> String {
    format!("agent-job:{}", job.spec().idem_token.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{EffectKind, ToolName};
    use myelin_ci_sandbox::{
        JobSpec as SandboxJobSpec, ResourceUsage, SandboxHandle, SandboxLaunch, SandboxResult,
        SpecError,
    };
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_flow::engine::{SignalRow, SignalStore};
    use myelin_flow::{
        job_idem_token, DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenLease,
        RunTokenMinter, WfJournal, JOB_DONE_SIGNAL,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    // ───────────────────────────── the async-dispatch backend fixture ───────────────────────────

    /// **The unified-sandbox backend in its ASYNC-DISPATCH role (the §5.6 long-park seam).** Where the
    /// in-line `SandboxBackend::launch` (AG-P15) BLOCKS, `accept_async` DISPATCHES the hardened spec and
    /// RETURNS (the run parks; the runner runs the job for hours and later delivers `job.done`). The
    /// fixture RECORDS each accepted spec (so a test asserts the engine's deterministic idem_token was
    /// stamped) and counts dispatches (so a replay's 0-re-dispatch is provable). `fail_first` drives the
    /// engine's dispatch-retry (reusing the same idem_token). There is NO host-exec path (no
    /// `process::Command`) — the async dispatch is the ONLY execution seam (the `no-host-exec` lint, 1.6).
    #[derive(Default)]
    struct RecordingAsyncBackend {
        accepted: Mutex<Vec<SandboxJobSpec>>,
        calls: AtomicUsize,
        fail_first: bool,
    }

    impl SandboxBackend for RecordingAsyncBackend {
        type Error = SpecError;
        // The in-line launch is unused on the long-park path; a shape stub keeps the trait satisfied.
        fn launch(
            &self,
            _spec: &SandboxJobSpec,
            _hooks: &myelin_ci_sandbox::RunnerHooks,
        ) -> Result<SandboxLaunch, Self::Error> {
            Ok(SandboxLaunch {
                handle: SandboxHandle {
                    guest_id: "unused-inline".into(),
                },
                result: SandboxResult::stub_ok(ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                }),
                output_complete: true,
            })
        }
        fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
            Ok(())
        }
        fn accept_async(&self, spec: &SandboxJobSpec) -> Result<(), Self::Error> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && n == 0 {
                // A transient dispatch failure (the runner is briefly unreachable) — the engine
                // retries on the SAME idem_token.
                return Err(SpecError::NoTimeout);
            }
            self.accepted.lock().unwrap().push(spec.clone());
            Ok(())
        }
    }

    // ───────────────────────────────── the engine substrate fixtures ────────────────────────────

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }
    fn begin(outbox: &OutboxStore, journal: WfJournal, signals: SignalStore) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals)
    }

    fn pinned() -> ImageRef {
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef000000000000000000000000000000000000000000000000").unwrap()
    }
    fn limits() -> ResourceLimits {
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 7200, // a TWO-HOUR job (the long-park point).
        }
    }
    fn compute_def(name: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "agent".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Compute,
            side_effecting: false,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }
    fn profile() -> LongComputeProfile {
        LongComputeProfile {
            image: pinned(),
            command: vec!["cargo".into(), "test".into(), "--release".into()],
            secret_refs: vec![],
            egress: EgressPolicy::deny_all(),
            limits: limits(),
            trust_tier: TrustTier::UntrustedFork,
            run_token: RunTokenCredential::new("test-bearer", "agent-jti", 300).unwrap(),
            meter_to: MeterTarget {
                reserve_id: "agent-res".into(),
            },
            idem_token: IdemToken("agent-idem".into()),
        }
    }

    /// A real GREEN AG-D4 gate for the long-park test (minted from the corpus parser — never
    /// hardcoded). The long-park dispatcher requires it to exist at all (the structural fail-closed,
    /// AG-P17 → P-229): no green attestation ⇒ no `AgentJobDispatcher` ⇒ no untrusted long compute.
    fn green_gate() -> AgentExecGate {
        use crate::escape_gate::ProductionBackendId;
        use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
        use myelin_ci_sandbox::{
            parse_console, Backend, BackendRun, EscapeAttestation, CORPUS, CORPUS_VERSION,
        };
        let id = ProductionBackendId {
            backend: Backend::FirecrackerMicrovm,
            rootfs_sha256: "rootfs-digest".into(),
            kernel_sha256: "kernel-digest".into(),
            corpus_version: CORPUS_VERSION,
        };
        let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
        for atk in CORPUS {
            console.push_str(&format!("{} CONTAINED\n", atk.id));
        }
        console.push_str(&format!("{END_MARKER}\n"));
        let report = parse_console(&console);
        let att = EscapeAttestation::from_green_drill(
            "2026-06-21",
            &report,
            vec![BackendRun {
                backend: Backend::FirecrackerMicrovm,
                exercised: true,
                residual_note: None,
            }],
            Backend::FirecrackerMicrovm,
            "rootfs-digest",
            "kernel-digest",
            "6.1.168",
        )
        .unwrap();
        AgentExecGate::admit(Some(&att), &id).unwrap()
    }

    /// Deliver a `job.done` keyed on the engine's deterministic dispatch token (the runner echoes it).
    fn deliver_job_done(signals: &SignalStore, idem_token: &str, result: Vec<ArtifactRef>) {
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: JOB_DONE_SIGNAL.into(),
            idem_key: idem_token.into(),
            payload: result,
            payload_key_ref: None,
            consumed_seq: None,
            received_unix_ms: 0,
        });
    }

    // ───────────────────── gate #2: dispatch-and-return — the run holds no runtime ───────────────

    /// **A long compute dispatches-and-returns; the run PARKS holding NO runtime (§5.6 gate #2).** No
    /// buffered `job.done` → the dispatcher accepts the hardened job asynchronously (one accept) and
    /// the run parks (`state=waiting`); the worker is freed (it did NOT block on the two-hour job).
    #[test]
    fn long_compute_dispatches_and_parks_holding_no_runtime() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();

        let mut ctx = begin(&outbox, journal, signals);
        let out = dispatch_long_compute(
            &mut ctx,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("a compute tool builds a long-park job")
        .expect("dispatch + park");

        assert!(
            out.is_parked(),
            "the long-park returns Parked (the worker is freed): {out:?}"
        );
        assert!(
            ctx.parked_on_signal(),
            "the run is waiting on job.done (holds NO runtime)"
        );
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "the job was dispatched exactly once"
        );
        assert_eq!(
            ctx.consumed_signals().len(),
            0,
            "nothing consumed — the job is still running"
        );
    }

    /// **The dispatched spec carries the engine's DETERMINISTIC idem_token (the §4.9 no-coordination
    /// agreement).** The engine mints the token on the dispatch position; the dispatcher's accepted
    /// spec carries it; a runner-fixture deriving from `(run_id, command_id)` would AGREE — the dedup
    /// key the `job.done` echoes is fixed without a coordination round-trip.
    #[test]
    fn the_dispatched_spec_carries_the_deterministic_idem_token() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();

        // the dispatch is the FIRST command of this body → command_id = "agent.run:0".
        let consumer_token = job_idem_token("R1", "agent.run:0");

        let mut ctx = begin(&outbox, journal, signals);
        let _ = dispatch_long_compute(
            &mut ctx,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("build")
        .expect("park");

        let accepted = backend.accepted.lock().unwrap();
        assert_eq!(accepted.len(), 1, "one async dispatch");
        assert_eq!(
            accepted[0].idem_token.0, consumer_token,
            "the engine stamped the deterministic dispatch token the runner echoes on job.done"
        );
        assert_eq!(
            accepted[0].kind,
            myelin_ci_sandbox::JobKind::Agent,
            "the hardened spec the backend received is a kind=agent job"
        );
    }

    // ───────────────────── gate #1: a doubly-delivered job.done wakes the run ONCE ───────────────

    /// **GATE #1 — a DOUBLE-delivered `job.done` wakes the long-parked run EXACTLY ONCE (§5.6).** The
    /// runner delivers "done" TWICE (at-least-once under the bus, both on the SAME `idem_token`); the
    /// engine's `wf_signal` PK dedups to one buffered row; the long-park consumes it EXACTLY once → ONE
    /// `Completed` carrying the result. 1 wake per job, never two.
    #[test]
    fn a_doubly_delivered_job_done_wakes_the_run_exactly_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();

        // the runner already finished a fast-ish job: job.done buffered under the deterministic token —
        // DELIVERED TWICE (at-least-once). The PK dedups to ONE buffered row.
        let token = job_idem_token("R1", "agent.run:0");
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/agent/trace/ok".into())],
        );
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/agent/trace/ok".into())],
        );
        assert_eq!(
            signals.buffered_depth(),
            1,
            "the double delivery deduped to ONE buffered row"
        );

        let mut ctx = begin(&outbox, journal, signals.clone());
        let out = dispatch_long_compute(
            &mut ctx,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("build")
        .expect("dispatch + complete");

        match out {
            LongParkOutcome::Completed { idem_token, result } => {
                assert_eq!(idem_token, token, "the runner echoed the dispatch token");
                assert_eq!(
                    result,
                    vec![ArtifactRef("myelin://acme/agent/trace/ok".into())]
                );
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(
            ctx.consumed_signals().len(),
            1,
            "EXACTLY ONE wake per job (the double-delivery deduped)"
        );
        assert_eq!(
            signals.buffered_depth(),
            0,
            "the one buffered row is consumed once"
        );
    }

    // ───────────────────── gate #3: re-mint on wake after a long park ────────────────────────────

    /// A recording minter — a REAL impl of the contract-4.7 mint surface (each mint a DISTINCT token).
    #[derive(Default)]
    struct RecordingMinter {
        calls: Mutex<Vec<(String, DelegationCaveats, u64)>>,
    }
    impl RunTokenMinter for RecordingMinter {
        fn mint_run_token(
            &self,
            agent_id: &str,
            run_id: &str,
            caveats: &DelegationCaveats,
            ttl_secs: u64,
        ) -> Result<RunTokenHandle, RunTokenError> {
            let mut c = self.calls.lock().unwrap();
            let n = c.len();
            c.push((agent_id.into(), caveats.clone(), ttl_secs));
            Ok(RunTokenHandle {
                token: format!("tok:{run_id}:{n}"),
                jti: format!("jti:{run_id}:{n}"),
                ttl_secs,
            })
        }
    }

    /// **GATE #3 — on wake after a long park the per-run token is RE-MINTED (§5.6 / §5.7 C6).** A run
    /// that DISPATCHED + parked on drive 1, re-driven on the arriving `job.done`, resumes through the
    /// engine's wait-resume leg, which RE-MINTS a fresh short-lived per-run token (token life ==
    /// activity life — never the days-long workflow life). The resumed body runs under the FRESH token.
    #[test]
    fn on_wake_after_a_long_park_the_per_run_token_is_reminted() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();
        let mint = Arc::new(RecordingMinter::default());
        let lease = RunTokenLease::new(
            mint.clone(),
            "psn:agent-7",
            DelegationCaveats(vec!["delegated:human-x".into()]),
        );

        // DRIVE 1: dispatch + park (no job.done yet). A run-identity lease is wired so a resume
        // re-mints. The cold first drive PARKS — it does NOT re-mint (nothing resumed yet).
        let mut c1 =
            begin(&outbox, journal.clone(), signals.clone()).with_run_identity(lease.clone());
        let out1 = dispatch_long_compute(
            &mut c1,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("build")
        .expect("dispatch + park");
        assert!(out1.is_parked(), "drive 1 parks holding no runtime");
        assert_eq!(
            c1.reminted_tokens(),
            0,
            "the cold dispatch drive does NOT re-mint (nothing resumed)"
        );
        c1.commit()
            .expect("co-commit the dispatch + the park marker");
        let history = journal.history_for(&tenant(), "R1");

        // ... HOURS later the runner delivers job.done (the long compute finished) ...
        let token = job_idem_token("R1", "agent.run:0");
        deliver_job_done(
            &signals,
            &token,
            vec![ArtifactRef("myelin://acme/agent/trace/ok".into())],
        );

        // DRIVE 2 (the wake): resume on the arriving job.done. The engine's wait-resume leg RE-MINTS
        // the per-run token BEFORE the resumed body runs.
        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone())
        .with_run_identity(lease);
        let out2 = dispatch_long_compute(
            &mut c2,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            None,
        )
        .expect("build")
        .expect("the wake drive");
        assert!(
            out2.is_completed(),
            "drive 2 completes on the arrived job.done: {out2:?}"
        );
        assert_eq!(
            c2.reminted_tokens(),
            1,
            "the wake RE-MINTED a fresh per-run token (gate #3)"
        );

        // the re-mint was a SHORT-LIVED attenuated per-run token (token life == activity life).
        let calls = mint.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "exactly one re-mint on the wake");
        let (agent, cav, ttl) = calls[0].clone();
        assert_eq!(agent, "psn:agent-7");
        assert_eq!(
            ttl,
            RunTokenLease::DEFAULT_TTL_SECS,
            "short-lived (the fail-static W, not the workflow life)"
        );
        assert!(
            cav.0.contains(&"run:R1".to_string()),
            "attenuated per-run (cannot act outside R1)"
        );
        assert!(
            cav.0.contains(&"delegated:human-x".to_string()),
            "the SAME grant chain (attenuate-only)"
        );
    }

    // ───────────────────── the vanished-runner SLA bound + the routing split ─────────────────────

    /// **A vanished runner's SLA timer bounds the wait → `TimedOut` (§5.6).** The job is dispatched but
    /// the runner never reports. Drive 1 parks with a 1-hour SLA. Drive 2 (the engine clock past the
    /// deadline) STILL has no `job.done` → `TimedOut` (the body fails/retries the job — never parking
    /// forever). The job was dispatched ONCE (the replay short-circuit did not re-dispatch).
    #[test]
    fn a_vanished_runner_times_out_and_never_parks_forever() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = myelin_flow::timer::TimerStore::new();
        let backend = RecordingAsyncBackend::default();

        // DRIVE 1 at clock=1000 with a 3600s SLA → dispatch + park (deadline 4600 not reached).
        let mut c1 =
            begin(&outbox, journal.clone(), signals.clone()).with_timers(timers.clone(), 0, 1000);
        let out1 = dispatch_long_compute(
            &mut c1,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            Some(3600),
        )
        .expect("build")
        .expect("dispatch + park");
        assert!(
            out1.is_parked(),
            "dispatched, parked on job.done with an SLA timer"
        );
        c1.commit().expect("co-commit the dispatch + the SLA timer");
        let history = journal.history_for(&tenant(), "R1");

        // DRIVE 2 at clock=9000 (past the 4600 deadline), STILL no job.done → TimedOut.
        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone())
        .with_timers(timers.clone(), 0, 9000);
        let out2 = dispatch_long_compute(
            &mut c2,
            green_gate(),
            &backend,
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            profile(),
            Some(3600),
        )
        .expect("build")
        .expect("the timeout drive");
        assert!(
            out2.is_timed_out(),
            "the SLA fired before the runner reported → TimedOut: {out2:?}"
        );
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "the job was dispatched ONCE — the replay short-circuit did not re-dispatch it"
        );
    }

    /// **The routing split holds for a long-park job too: a `mutate` tool can NEVER long-park (§5.0 /
    /// X-6 #3).** A non-`compute` tool is REFUSED LOUD at build — it has no path to the sandbox (the
    /// type-level safety boundary AG-P15 owns). A long-park job is built by the SAME hardened
    /// constructor, so the safety boundary is identical.
    #[test]
    fn a_mutate_tool_can_never_long_park() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let backend = RecordingAsyncBackend::default();

        let mutate = ToolDef {
            name: ToolName("issue.create".into()),
            subsystem: "issues".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval: true,
            exposed_over_mcp: false,
        };
        let mut ctx = begin(&outbox, journal, signals);
        let err = dispatch_long_compute(
            &mut ctx,
            green_gate(),
            &backend,
            &mutate,
            &Command("x".into()),
            profile(),
            None,
        )
        .expect_err("a mutate tool cannot build a long-park job");
        assert!(
            matches!(err, RoutingError::NotComputeBound { ref tool, .. } if tool == "issue.create"),
            "a non-compute tool is REFUSED LOUD (the routing split): {err:?}"
        );
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            0,
            "nothing was dispatched (0 mutate-via-exec)"
        );
    }

    // ───────────────────────────── value-type mutation floor ─────────────────────────────────────

    /// **The `LongParkOutcome` predicates are exact (mutation floor).** Each variant's predicate is
    /// true for itself and false for the others (kills a `-> true`/`-> false` constant mutant).
    #[test]
    fn long_park_outcome_predicates_are_exact() {
        let completed = LongParkOutcome::Completed {
            idem_token: "t".into(),
            result: vec![],
        };
        let parked = LongParkOutcome::Parked;
        let timed_out = LongParkOutcome::TimedOut;
        assert!(completed.is_completed() && !completed.is_parked() && !completed.is_timed_out());
        assert!(parked.is_parked() && !parked.is_completed() && !parked.is_timed_out());
        assert!(timed_out.is_timed_out() && !timed_out.is_completed() && !timed_out.is_parked());
    }

    /// **`from_job_outcome` is a faithful re-tag (mutation floor).** Each engine outcome maps to its
    /// Fabric twin, carrying the payload (kills a variant-swap mutant).
    #[test]
    fn from_job_outcome_is_a_faithful_re_tag() {
        let refs = vec![ArtifactRef("r".into())];
        assert_eq!(
            LongParkOutcome::from_job_outcome(JobOutcome::Completed {
                idem_token: "t".into(),
                result: refs.clone(),
            }),
            LongParkOutcome::Completed {
                idem_token: "t".into(),
                result: refs,
            }
        );
        assert_eq!(
            LongParkOutcome::from_job_outcome(JobOutcome::Parked),
            LongParkOutcome::Parked
        );
        assert_eq!(
            LongParkOutcome::from_job_outcome(JobOutcome::TimedOut),
            LongParkOutcome::TimedOut
        );
    }

    /// **The long-job target is references-not-payloads (no PII; a machine handle).** It encodes the
    /// hardened job's idem token so the dispatcher resolves WHICH job to launch — never an inline body.
    #[test]
    fn the_long_job_target_is_references_not_payloads() {
        let job = build_long_job(
            &compute_def("agent.long_test"),
            &Command("--workspace".into()),
            &profile(),
        )
        .unwrap();
        let target = long_job_target(&job);
        assert_eq!(
            target, "agent-job:agent-idem",
            "a machine handle naming the job, no PII body"
        );
    }
}
