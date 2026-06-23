//! # `schedule_and_run_job` — the `SCHEDULE_AND_RUN_JOB` dispatch handshake + effectively-once (CI-P16 → P-359, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §3.3 (THE frozen `SCHEDULE_AND_RUN_JOB` handshake — dispatch+park woken by the `job.done` signal
//! idempotent on `idem_token`, the reaper retry = effectively-once) + §3.1 (the boundary: the
//! scheduler owns *which runner/when/fairness/lanes/affinity/leasing/reaping*, reached INSIDE the
//! dispatch) + §3.2 (the activity boundary is the JOB, not the step).
//! **Reconciliation:** `00-reconciliation-decisions.md` §OQ-F (the `SCHEDULE_AND_RUN_JOB` idiom + the
//! per-effect `idem_key` + the `job.done` signal — the `idem_token` minted at the workflow,
//! producer/consumer agree with NO round-trip).
//! **Contracts (implemented to the FROZEN idiom):** 9.2 (the `WfCtx` `SCHEDULE_AND_RUN_JOB`
//! long-park), 9.4 (the durable `job.done` signal wait).
//!
//! ## What CI-P16 ships — THE DISPATCH HANDSHAKE INTO CI's `job_queue` + EFFECTIVELY-ONCE (CI-D1)
//!
//! The `SCHEDULE_AND_RUN_JOB` long-park *idiom itself* — mint the deterministic `idem_token`, dispatch
//! the activity, park on `job.done`, idempotent completion, the reaper-replay short-circuit — is the
//! FROZEN `myelin-flow` engine ([`myelin_flow::WfCtx::schedule_and_run_job`] /
//! [`myelin_flow::WfCtx::metered_schedule_and_run_job`], P-FLOW-15/16, reconciled in place, NOT
//! re-built here). This module is **CI's half of the handshake**: the concrete [`JobRunner`] that
//! BINDS the engine's frozen dispatch seam onto the CI scheduler's [`SchedulerState`] `job_queue`,
//! and the runner's terminal `job.done` delivery path. The boundary the arch §3.1 draws:
//!
//! - **The engine** owns the lifecycle/replay/park/settle + mints the `idem_token` (deterministic
//!   from the dispatch `command_id` — so the runner echoes it on `job.done` with no coordination
//!   round-trip) + the idempotent-completion `wf_signal` PK dedup.
//! - **CI's scheduler** (this module's [`SchedulerJobRunner`]) owns *which runner/when; fairness;
//!   lanes; affinity; leasing; reaping* — reached INSIDE the dispatch. On
//!   [`JobRunner::dispatch`] it ENQUEUES the job into `job_queue` (idempotent on the engine-minted
//!   `idem_token` via the scheduler's `jq_idem` unique) with the **lane / labels / trust-tier /
//!   concurrency-group / fair-key derived from the snapshot** ([`JobScheduleTerms`]). The activity
//!   boundary is the JOB (§3.2): one enqueue per stage, never per step.
//!
//! ## EFFECTIVELY-ONCE (the CI-D1 drill: kill runner mid-job + kill control plane mid-run)
//!
//! Three idempotency keys compose to **effectively-once** — 0 lost runs, 0 double-deploys, 0 duplicate
//! artifact publishes (arch §3.3 step 4):
//!
//! 1. **The `idem_token` is deterministic on the dispatch position** (the engine mints it; a re-drive
//!    re-derives the SAME token). The runner echoes it on `job.done` — the no-coordination agreement.
//! 2. **The enqueue is idempotent on `idem_token`** ([`SchedulerState::enqueue`] → `jq_idem`): a
//!    DEAD-runner reaper re-queue + a redundant `SCHEDULE_AND_RUN_JOB` re-dispatch is ONE `job_queue`
//!    row, never a duplicate (so the job runs once, never twice — 0 double-deploy).
//! 3. **The `job.done` signal is idempotent on `idem_token`** (the engine's `wf_signal` PK): the
//!    runner can deliver "done" twice (at-least-once) and the workflow wakes ONCE.
//!
//! A killed control plane mid-run REPLAYS the journaled prefix (0 re-dispatch — the dispatch activity
//! short-circuits) and idempotently re-dispatches the un-journaled stage (the `jq_idem` unique
//! collapses the re-enqueue). A killed runner mid-job leaves an expired lease the reaper re-queues
//! (one row); the engine's dispatch-position `idem_token` makes the re-claim + re-run a re-attempt of
//! the SAME job, whose terminal effect is dedup-keyed. **at-least-once activity + idempotent job =
//! effectively-once.**
//!
//! ## MUTATION-SCORE FLOOR (mandatory-core)
//!
//! The handshake module ([`SchedulerJobRunner::dispatch`] + [`complete_job`] + [`JobScheduleTerms`])
//! is **mandatory-core** — the effectively-once invariant is a hard correctness gate (a missed dedup
//! double-deploys). The cargo-mutants mutation-score floor for this module is **≥ 0.90** (the
//! mandatory-core floor, EI-01 §3: every mutant of the kind-guard / the `idem_token`-as-job_id / the
//! `pr:%`-supersede branch / the terminal-transition must be caught by the unit + CI-D1 drill tests).
//! The cargo-mutants run is the M0 permanent-suite cadence (it is not run on every prompt commit; the
//! floor is stated here as the gate the suite enforces, never weakened).
//!
//! ## NAMED FLOORS (recorded here, filled later)
//!
//! - **The reserve-at-dispatch / settle-on-`job.done` METERING bookends** (the wholesale→retail markup
//!   into the `cost_event` ledger) are **CI-P17 (P-360)**. This module dispatches over the engine's
//!   reserve/settle bookend ([`myelin_flow::WfCtx::metered_schedule_and_run_job`], 11.7) — CI-P17
//!   wires the real `cost_event` rows + the parity CI↔agent metering. State this.
//! - **The LIVE runner lease + the in-sandbox EXECUTION of the dispatched job** (`ToolHands::exec`,
//!   contract 8.4) is GATED by **AG-D4** (the sandbox-escape drill, `04-sandbox-AG-D4.md`). This
//!   module ENQUEUES into `job_queue` (the dispatch handshake) + delivers the runner's terminal
//!   `job.done`; the claim→lease→sandbox-launch of untrusted code lands behind that gate. The CI-D1
//!   drill here exercises the handshake + the effectively-once invariant over the scheduler model +
//!   the engine, with a recording runner standing in for the sandboxed execution.

use crate::scheduler::{EnqueueOutcome, Lane, QueuedJob, SchedulerState, TrustTier};
use myelin_flow::{ActivityError, JobKind, JobRunner, JobSpec};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// **The per-run scheduling terms the dispatch derives from the snapshot (arch §3.1/§3.3).** When a
/// `SCHEDULE_AND_RUN_JOB` enqueues a stage's job, the engine has already minted the `idem_token`; CI's
/// scheduler stamps the *scheduling* columns the claim orders/filters on — the lane, the affinity
/// labels, the trust tier, the concurrency group, the fair-key. These are a PURE function of the run's
/// resolved+pinned snapshot (the trust tier stamped at trigger time, the repo/PR the concurrency group
/// keys on, the plan the fair-key keys on), NEVER recomputed per dispatch — so a re-drive's re-enqueue
/// carries the IDENTICAL terms (the `jq_idem` unique then collapses it to one row). PII-free: every
/// field is an opaque id / vocabulary token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobScheduleTerms {
    /// The tenant partition (the `job_queue` PK first component; never crossed by a claim).
    pub tenant_id: String,
    /// The residency region — a runner claims only in-region (no global pool, arch 00 §5).
    pub region: String,
    /// The owning CI run id (the `job_queue.run_id` — the `job.done` signal's run target).
    pub run_id: String,
    /// The lane the stage runs in (interactive PR-check > batch matrix > deploy, arch 02 §2.3).
    pub lane: Lane,
    /// The affinity labels the job requires (a job is claimable iff `labels ⊆ runner_labels`).
    pub labels: Vec<String>,
    /// The trust tier STAMPED at trigger time (an `untrusted_fork` job never reaches a trusted
    /// self-hosted runner, contract 4.9). Read off the run, NEVER recomputed here (X-1).
    pub trust_tier: TrustTier,
    /// The concurrency group (`deploy:prod` serialize / `pr:web:42` cancel-superseded) or `None`.
    pub concurrency_group: Option<String>,
    /// The DRR fairness key (`tenant` or `tenant:project`) — the claim's fair-share term.
    pub fair_key: String,
}

impl JobScheduleTerms {
    /// Build the minimal scheduling terms for a run (tenant/region/run + lane + trust + fair-key). The
    /// labels + concurrency group are optional builder add-ons (most stages need neither).
    pub fn new(
        tenant_id: impl Into<String>,
        region: impl Into<String>,
        run_id: impl Into<String>,
        lane: Lane,
        trust_tier: TrustTier,
        fair_key: impl Into<String>,
    ) -> JobScheduleTerms {
        JobScheduleTerms {
            tenant_id: tenant_id.into(),
            region: region.into(),
            run_id: run_id.into(),
            lane,
            labels: Vec::new(),
            trust_tier,
            concurrency_group: None,
            fair_key: fair_key.into(),
        }
    }

    /// Builder: the affinity labels the stage's job requires.
    pub fn with_labels(mut self, labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    /// Builder: the concurrency group (`deploy:prod` serialize / `pr:web:42` cancel-superseded).
    pub fn with_concurrency_group(mut self, group: impl Into<String>) -> Self {
        self.concurrency_group = Some(group.into());
        self
    }
}

/// **The CI half of the `SCHEDULE_AND_RUN_JOB` handshake — the concrete [`JobRunner`] that enqueues a
/// dispatched stage into the scheduler's `job_queue` (arch §3.1/§3.3).** The engine's
/// [`myelin_flow::WfCtx::schedule_and_run_job`] mints the deterministic `idem_token`, stamps it on the
/// [`JobSpec`], and hands the spec HERE; this runner ENQUEUES the job into the shared
/// [`SchedulerState`] with the run's [`JobScheduleTerms`] (lane/labels/trust/concurrency/fair-key), so
/// CI's pull-lease scheduler can claim+lease+run it (CI-P12/13) and the dead-runner reaper can
/// re-queue it (CI-P12). The enqueue is **idempotent on the engine-minted `idem_token`** (the
/// `jq_idem` unique): a re-dispatch (control-plane replay) + a reaper re-queue (runner death) is ONE
/// row, never a duplicate — the effectively-once invariant (CI-D1).
///
/// **The `idem_token` is the job id.** The engine mints ONE deterministic-on-position token per stage
/// dispatch; CI uses it as BOTH the `job_queue.job_id` (the PK) AND the `jq_idem` idempotency key (so
/// a re-enqueue is a no-op) AND the `job.done` `idem_key` the runner echoes (the no-coordination
/// agreement). One token, three roles, zero drift.
///
/// **GATED BY AG-D4.** This runner ENQUEUES into `job_queue` (the dispatch handshake) — it does NOT
/// launch the sandbox. The claim→lease→`ToolHands::exec` of untrusted code lands behind the
/// sandbox-escape gate (`04-sandbox-AG-D4.md`); the scheduler's claim (CI-P12) is the seam to that
/// binding.
#[derive(Clone)]
pub struct SchedulerJobRunner {
    /// The shared scheduler the dispatch enqueues into — the SAME `job_queue` the pull-lease claim +
    /// the reaper operate on (the boundary the arch §3.1 draws: the dispatch reaches INTO the
    /// scheduler).
    scheduler: Arc<Mutex<SchedulerState>>,
    /// The per-run scheduling terms (a PURE function of the snapshot) the enqueue stamps onto every
    /// stage's `job_queue` row.
    terms: JobScheduleTerms,
    /// A monotonic enqueue counter (the `enqueued_at ASC` tie-break the claim orders on — lower =
    /// older). Shared across this run's dispatches so each stage gets a strictly-increasing seq.
    next_seq: Arc<AtomicU64>,
}

impl SchedulerJobRunner {
    /// Build a `SchedulerJobRunner` for one run: the shared scheduler + the run's [`JobScheduleTerms`].
    /// The dispatches this runner accepts are enqueued into `scheduler` with `terms` stamped on each
    /// `job_queue` row.
    pub fn new(
        scheduler: Arc<Mutex<SchedulerState>>,
        terms: JobScheduleTerms,
    ) -> SchedulerJobRunner {
        SchedulerJobRunner {
            scheduler,
            terms,
            next_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The shared scheduler this runner enqueues into (read-only handle — for the caller's claim/reap
    /// drive + assertions).
    pub fn scheduler(&self) -> &Arc<Mutex<SchedulerState>> {
        &self.scheduler
    }

    /// Build the `job_queue` row for a dispatched [`JobSpec`] (the enqueue shape). The engine-minted
    /// `idem_token` is BOTH the `job_id` (the PK) and the `idem_token` (the `jq_idem` key). The
    /// scheduling terms are the run's [`JobScheduleTerms`]. (A pure function so a re-drive's re-enqueue
    /// builds the IDENTICAL row — the `jq_idem` unique then makes it a no-op.)
    fn queued_job(&self, spec: &JobSpec) -> QueuedJob {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let mut job = QueuedJob::enqueued(
            self.terms.tenant_id.clone(),
            self.terms.region.clone(),
            // the engine-minted idem_token IS the job id (the PK) — one token, no second id language.
            spec.idem_token.clone(),
            self.terms.run_id.clone(),
            self.terms.lane,
            self.terms.trust_tier,
            self.terms.fair_key.clone(),
            // ... and the jq_idem idempotency key (so a re-enqueue is a no-op — effectively-once).
            spec.idem_token.clone(),
            seq,
        );
        if !self.terms.labels.is_empty() {
            job = job.with_labels(self.terms.labels.clone());
        }
        if let Some(group) = &self.terms.concurrency_group {
            job = job.with_concurrency_group(group.clone());
        }
        job
    }
}

impl JobRunner for SchedulerJobRunner {
    /// **Dispatch = ENQUEUE into `job_queue`, idempotent on the engine-minted `idem_token` (arch
    /// §3.3 step 1).** The engine has stamped the deterministic `idem_token` on `spec`; this enqueues
    /// the job into the shared scheduler with the run's [`JobScheduleTerms`]. A `pr:%` concurrency
    /// group enqueues SUPERSEDING (the prior queued/leased rows of the group are cancelled — only the
    /// latest PR head is tested, arch §2.3); any other group (incl. `deploy:%`) enqueues plainly. The
    /// enqueue is idempotent on `(tenant_id, idem_token)` (the `jq_idem` unique): a re-dispatch
    /// (control-plane replay) or a reaper re-queue (runner death) collapses to ONE row — never a
    /// duplicate job (0 double-deploy, CI-D1).
    ///
    /// Returns `Ok(())` whether the enqueue INSERTED a fresh row or was a DUPLICATE no-op — both are a
    /// *successful dispatch* (the job is in the queue exactly once). A dispatch failure (a poisoned
    /// scheduler lock) surfaces as an [`ActivityError`] the engine retries (reusing the SAME
    /// `idem_token` — the runner dedups the re-dispatch on it).
    fn dispatch(&self, spec: &JobSpec) -> Result<(), ActivityError> {
        if spec.kind != JobKind::Ci {
            return Err(ActivityError(format!(
                "SchedulerJobRunner dispatches kind=ci jobs into job_queue; got kind={} \
                 (an agent job dispatches into the agent runner, not CI's job_queue)",
                spec.kind.as_str()
            )));
        }
        let job = self.queued_job(spec);
        let mut scheduler = self
            .scheduler
            .lock()
            .map_err(|_| ActivityError("the CI scheduler lock was poisoned".into()))?;
        // A `pr:%` group supersedes (cancel the prior head); any other group enqueues plainly. Both
        // paths are idempotent on `idem_token` (a re-enqueue is a no-op) — the effectively-once floor.
        let is_pr_group = job
            .concurrency_group
            .as_deref()
            .is_some_and(|g| g.starts_with("pr:"));
        let outcome = if is_pr_group {
            scheduler.enqueue_superseding(job)
        } else {
            scheduler.enqueue(job)
        };
        // Both Inserted and DuplicateIdem are a successful dispatch: the job is in the queue ONCE. The
        // DuplicateIdem branch is the effectively-once guarantee firing (a re-dispatch / reaper
        // re-queue did NOT create a second job).
        debug_assert!(matches!(
            outcome,
            EnqueueOutcome::Inserted | EnqueueOutcome::DuplicateIdem
        ));
        let _ = outcome;
        Ok(())
    }
}

/// **Deliver the runner's terminal `job.done` into the engine, idempotent on the dispatch `idem_token`
/// (arch §3.3 step 3).** The runner finished the leased job; it marks the `job_queue` row terminal
/// (so the reaper never re-queues a *completed* job) and delivers `signal(run, "job.done", {result},
/// idem_key = idem_token)` to wake the parked workflow. The signal is idempotent on the `idem_token`
/// (the engine's `wf_signal` PK): a DOUBLE delivery (at-least-once) wakes the run ONCE. The result
/// carries the references-not-payloads stage verdict ([`myelin_flow::stage_verdict_marker`]).
///
/// `mark_terminal` returns whether the `job_queue` row was moved to `terminal` (true the first time,
/// false on a re-delivery of an already-terminal job — itself idempotent). The actual `job.done`
/// signal delivery rides the engine's signal store (the caller holds the [`myelin_flow::SignalStore`]
/// / dispatcher); this helper owns the `job_queue` side of the terminal transition.
pub fn complete_job(
    scheduler: &Arc<Mutex<SchedulerState>>,
    tenant_id: &str,
    idem_token: &str,
) -> Result<bool, ActivityError> {
    let mut scheduler = scheduler
        .lock()
        .map_err(|_| ActivityError("the CI scheduler lock was poisoned".into()))?;
    // The job_id IS the idem_token (one token, three roles). Mark it terminal so the reaper never
    // re-queues a completed job (a completed job is not a dead-runner orphan). Idempotent: a re-mark
    // of an already-terminal job is a no-op (returns false).
    Ok(scheduler.complete_job(tenant_id, idem_token))
}

#[cfg(test)]
#[path = "schedule_and_run_job_tests.rs"]
mod tests;
