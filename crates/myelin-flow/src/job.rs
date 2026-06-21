//! # `job` — the `SCHEDULE_AND_RUN_JOB` long-park idiom (P-FLOW-15 → P-211, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §4.9 (the
//! `SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal idiom — the four-step mechanics: dispatch as
//! a journaled activity minting `idem_token` deterministic on `command_id`, stamp it on the
//! `JobSpec{kind: ci|agent}`, hand to the unified runner, journal
//! `activity_completed{job_dispatched: true, idem_token}`, and RETURN; then
//! `wait_for_signal("job.done", idem_key=idem_token)` + a timeout timer; idempotent completion) +
//! §3.5 (`wf_activity_attempt` — the dispatch attempt). Carried from recon §OQ-F.
//!
//! **Contract-index cluster:** OWNS contract 9.2 (the `WfCtx` `SCHEDULE_AND_RUN_JOB` idiom) + 9.4
//! (the `job.done` durable signal wait). CONSUMES contract 8.4 (`ToolHands::exec` — the unified
//! runner, ADR-20 / X-6, the dispatch TARGET).
//!
//! ## What this prompt (P-FLOW-15) ships — DISPATCH + PARK + IDEMPOTENT-COMPLETION
//!
//! The idiom is **no new engine and no new table** — it composes the existing activity (§4.4),
//! durable signal (§4.3), and durable timer (§4.2) primitives ([`crate::wfctx`]). It is the seam
//! between an external scheduler/runner (CI's runner pool, an agent job) and the engine: a job whose
//! completion arrives **hours later** as a durable signal.
//!
//! [`WfCtx::schedule_and_run_job`](crate::WfCtx::schedule_and_run_job) does the four steps:
//!
//! 1. **Dispatch (an activity, §4.4).** Mints the `idem_token` *at the workflow* — DETERMINISTIC
//!    from the dispatch `command_id` (so producer (runner) and consumer (workflow) agree on the dedup
//!    key **without coordination**), stamps it on the [`JobSpec`], hands the spec to the unified
//!    runner ([`JobRunner::dispatch`], the contract-8.4 `ToolHands::exec` seam), journals
//!    `activity_completed{job_dispatched: true, idem_token}` (one `wf_history` row + the
//!    `wf_activity_attempt` ledger row), and **RETURNS** — it does NOT block on completion (the
//!    worker is freed).
//! 2. **Park (a durable signal wait, §4.3).** Immediately `wait_for_signal("job.done",
//!    idem_key = idem_token)` with an optional timeout timer (§4.2) bounding a vanished runner. The
//!    run flips `state='waiting'`, holds NO runtime for however long the job runs.
//! 3. **Completion (a signal, hours later).** The runner delivers `signal(run, "job.done", {result},
//!    idem_key = idem_token)`. The `wf_signal` PK (`ON CONFLICT (tenant, run_id, signal_name,
//!    idem_key) DO NOTHING`) makes a DOUBLE delivery idempotent: the workflow wakes **once**.
//! 4. **(Settle.)** On the consumed result the workflow settles budget — **reserve/settle is
//!    P-FLOW-16** (the bookend the dispatch fronts); this prompt ships only dispatch + park + the
//!    idempotent completion.
//!
//! ## The DISPATCH PATH IS GATED BY AG-D4 (recorded, NOT owned here)
//!
//! The dispatch hands the [`JobSpec`] to the unified runner ([`ToolHands::exec`], contract 8.4) — the
//! ONE sandbox shared by CI's `kind=ci` jobs and agent `kind=agent` jobs (ADR-20 / X-6). **No
//! `SCHEDULE_AND_RUN_JOB` dispatch may execute untrusted code until the sandbox-escape GATE AG-D4 is
//! GREEN** (Agent-Fabric / CI-owned — the real-kernel zero-escape drill, `04-sandbox-AG-D4.md`).
//! This engine DISPATCHES into the runner; it does **not** own the sandbox (external-insights/04 §5:
//! the runner is the sandbox, not this engine). The [`JobRunner`] seam is therefore a trait the
//! engine calls — the production binding to `ToolHands::exec` lands behind that gate.
//!
//! ## FLOORS named (this prompt ships dispatch + park + idempotent completion ONLY)
//!
//! - **Reserve/settle bookend** (the cost gate that fronts every dispatch, FLOW-D6) → **P-FLOW-16**
//!   (P-212). The dispatch here journals the `idem_token`; the reserve-at-dispatch / settle-on-
//!   completion meter wraps it next.
//! - **mint_run_token mid-workflow re-mint on resume** (token life == activity life) → **P-FLOW-17**
//!   (P-213). The dispatch into the runner re-mints a short-lived per-run token on resume there.
//! - **Loop safety** (causal-depth ceiling + shared-root tripwire + bounded activity pool, FLOW-D7)
//!   → **P-FLOW-18** (P-214). A `SCHEDULE_AND_RUN_JOB` that self-feeds a loop is bounded there.
//! - **The AG-D4 sandbox-escape gate** (the dispatch into the runner executes untrusted code) →
//!   Agent-Fabric / CI-owned, `04-sandbox-AG-D4.md`. RECORDED here, not owned.

use crate::wfctx::{WaitOutcome, WfCtx, WfError, WfResult};

/// **The FROZEN durable-signal name a `SCHEDULE_AND_RUN_JOB` long-park parks on (§4.9/§4.3).** The
/// runner delivers `signal(run, "job.done", {result}, idem_key = idem_token)` when the job finishes;
/// the workflow's `wait_for_signal(JOB_DONE_SIGNAL, …)` consumes it. One of the FROZEN signal-name
/// vocabulary (`approval` / `cancel` / `ci.result` / `job.done`, §5.1).
pub const JOB_DONE_SIGNAL: &str = "job.done";

/// **The kind of job a `SCHEDULE_AND_RUN_JOB` dispatches (§4.9).** Both `Ci` and `Agent` jobs ride the
/// ONE unified runner (`ToolHands::exec`, contract 8.4 — `= the CI runner's kind=agent job on the
/// unified sandbox`, ADR-20 / X-6); the kind is the runner's routing discriminator, not a second
/// sandbox. Stamped on the [`JobSpec`] the dispatch hands the runner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// A CI job (a build/test/lint stage) — the merge-queue's required CI, a CI-pipeline stage.
    Ci,
    /// An agent job (a long sandboxed computation an agent tool dispatched).
    Agent,
}

impl JobKind {
    /// The machine token for the kind (stamped on the runner command / the journaled dispatch — no
    /// PII, a routing discriminator).
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Ci => "ci",
            JobKind::Agent => "agent",
        }
    }
}

/// **The spec a `SCHEDULE_AND_RUN_JOB` hands the unified runner (§4.9).** References-not-payloads: the
/// `target` is an opaque job descriptor (a `kind=ci` pipeline ref, a `kind=agent` command — the CI /
/// Agent-Fabric surface owns its shape, an `ArtifactRef`-class identifier), never an inline PII body.
/// The `idem_token` is minted by the workflow at dispatch (DETERMINISTIC on the dispatch
/// `command_id`) and stamped here so the runner echoes it back on the `job.done` signal — the
/// no-coordination dedup agreement (§4.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSpec {
    /// The job kind (`ci | agent`) — the runner's routing discriminator (§4.9).
    pub kind: JobKind,
    /// The opaque job descriptor (references-not-payloads): a pipeline ref / an agent command id.
    /// The runner (CI / Agent Fabric) owns the shape; the engine carries it opaquely.
    pub target: String,
    /// **The `idem_token` the workflow minted at dispatch (DETERMINISTIC on the dispatch
    /// `command_id`, §4.9).** Stamped here so the runner echoes it on the `job.done` signal's
    /// `idem_key` — producer and consumer agree on the dedup key WITHOUT a coordination round-trip.
    /// Set by [`WfCtx::schedule_and_run_job`](crate::WfCtx::schedule_and_run_job); a caller-supplied
    /// value is OVERWRITTEN (the token is the engine's, not the caller's — the determinism anchor).
    pub idem_token: String,
}

impl JobSpec {
    /// Build a `JobSpec` for a `kind`/`target` with an EMPTY `idem_token` — the token is minted +
    /// stamped by [`WfCtx::schedule_and_run_job`](crate::WfCtx::schedule_and_run_job) (it must be
    /// deterministic on the dispatch position, so the caller never supplies it).
    pub fn new(kind: JobKind, target: impl Into<String>) -> Self {
        Self {
            kind,
            target: target.into(),
            idem_token: String::new(),
        }
    }
}

/// **The dispatch TARGET seam — the unified runner the `SCHEDULE_AND_RUN_JOB` hands its spec to
/// (contract 8.4 CONSUMED, §4.9).** This is the engine's view of `ToolHands::exec` (Agent
/// Fabric / CI's unified sandbox runner, ADR-20 / X-6): the engine `dispatch`es a [`JobSpec`] and
/// the runner accepts it for asynchronous execution (it does NOT block — the runner runs the job for
/// however long it takes and later delivers `signal(run, "job.done", …, idem_key = idem_token)`).
///
/// **GATED BY AG-D4.** The production binding (an adapter onto `ToolHands::exec`) executes untrusted
/// code in the sandbox — it MUST NOT run until the sandbox-escape gate AG-D4 is green (Agent-Fabric /
/// CI-owned, `04-sandbox-AG-D4.md`). The engine OWNS this trait (the dispatch seam); it does NOT own
/// the sandbox. The `idem_token` on the [`JobSpec`] is already stamped when `dispatch` is called.
///
/// `dispatch` returns `Ok(())` if the runner ACCEPTED the job for execution (the dispatch succeeded,
/// not the job), or an [`crate::ActivityError`] if the dispatch itself failed (the runner is
/// unreachable / rejected the spec) — a dispatch failure RETRIES like any activity (§4.4), reusing
/// the SAME `idem_token` (the runner dedups a re-dispatched job on it).
pub trait JobRunner {
    /// Hand the (already `idem_token`-stamped) [`JobSpec`] to the unified runner for asynchronous
    /// execution. Returns `Ok(())` on a dispatch the runner ACCEPTED (the completion arrives later as
    /// the `job.done` signal), or an [`crate::ActivityError`] if the dispatch failed (retried).
    fn dispatch(&self, spec: &JobSpec) -> Result<(), crate::ActivityError>;
}

/// **The outcome of a [`WfCtx::schedule_and_run_job`](crate::WfCtx::schedule_and_run_job) (§4.9).**
/// A long-park either RESUMES with the job's references-not-payloads result (the `job.done` signal
/// arrived and was consumed EXACTLY once), PARKS (the job is dispatched + the runner is running it —
/// the run is `waiting`, holding NO runtime, until `signal(run, "job.done", …)` delivers), or TIMES
/// OUT (the timeout timer fired before the runner reported — a vanished-runner bound, §4.9 step 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobOutcome {
    /// **The job COMPLETED — the `job.done` signal arrived and was CONSUMED exactly once (§4.9).**
    /// Carries the runner's `idem_token` (= the dispatch `idem_token`, echoed back — the dedup
    /// agreement held) and the job's references-not-payloads `result` refs. A double-delivered
    /// `job.done` wakes the workflow ONCE (the `wf_signal` PK dedup); on replay this returns the SAME
    /// journaled completion (consume-exactly-once across a re-drive).
    Completed {
        /// the `idem_token` the runner echoed (= the minted dispatch token — the agreement held).
        idem_token: String,
        /// the job's result refs (references-not-payloads, §3.4) — never a PII body.
        result: Vec<myelin_refs::ArtifactRef>,
    },
    /// **The job is DISPATCHED + the workflow PARKED on `job.done` (§4.9 step 2).** The run is
    /// `waiting`, holding NO runtime, until the runner delivers `signal(run, "job.done", …)`. The body
    /// should RETURN promptly on a `Parked` (it made no progress past the wait); the dispatcher re-
    /// drives it when the signal arrives.
    Parked,
    /// **The job TIMED OUT — the timeout timer fired before the runner reported (§4.9 step 2).** A
    /// runner that vanished does NOT park the workflow forever: the timeout branch fails the job (the
    /// body retries / compensates / dequeues, exactly like a failed synchronous activity). The wait
    /// carried a `timeout`; the durable `wf_timer` armed for it fired first.
    TimedOut,
}

impl WfCtx {
    /// **`schedule_and_run_job(spec, runner, timeout)` (contract 9.2/9.4, §4.9) — the
    /// `SCHEDULE_AND_RUN_JOB` long-park-completed-by-signal idiom.** Dispatch-and-return + park-on-
    /// `job.done` + idempotent completion, composing the existing activity (§4.4) / signal (§4.3) /
    /// timer (§4.2) primitives — NO new engine, NO new table.
    ///
    /// **The four steps (§4.9):**
    /// 1. **Dispatch.** Mints the `idem_token` DETERMINISTIC on the dispatch `command_id` (so the
    ///    runner and the workflow agree on the dedup key WITHOUT coordination), stamps it on `spec`,
    ///    hands `spec` to `runner` ([`JobRunner::dispatch`] = `ToolHands::exec`, contract 8.4), and
    ///    journals `activity_completed{job_dispatched: true}` carrying the `idem_token` (one
    ///    `wf_history` row + the `wf_activity_attempt` ledger row). Retries the DISPATCH on failure
    ///    (reusing the same `idem_token`). The activity worker is FREED — it does NOT block on the
    ///    job's completion.
    /// 2. **Park.** `wait_for_signal("job.done", idem_key = idem_token)` with the optional `timeout`
    ///    (seconds) arming a durable timeout-timer that bounds a vanished runner. The run flips
    ///    `state='waiting'`, holds NO runtime.
    /// 3. **Completion.** When `signal(run, "job.done", {result}, idem_key = idem_token)` is
    ///    delivered, the wait consumes it EXACTLY once (the `wf_signal` PK dedups a double delivery)
    ///    and this returns [`JobOutcome::Completed`].
    ///
    /// **Replay (§4.1):** the dispatch SHORT-CIRCUITS (the `activity_completed` is journaled — the job
    /// is NOT re-dispatched), the `idem_token` is RE-DERIVED identically (the command counter advances
    /// the same way), and the wait short-circuits to the SAME journaled `job.done` (consume-exactly-
    /// once across a re-drive). A `SCHEDULE_AND_RUN_JOB` is fully deterministic under replay.
    ///
    /// **Returns** [`JobOutcome::Completed`] (the job.done arrived — consumed once),
    /// [`JobOutcome::Parked`] (dispatched, the run waits — the body returns promptly), or
    /// [`JobOutcome::TimedOut`] (the runner vanished and the timeout fired). A [`WfError`] surfaces a
    /// dispatch that exhausted its retries ([`WfError::ActivityExhausted`]), a missing signal store /
    /// timer wheel ([`WfError::CoCommit`]), or a replay divergence ([`WfError::Nondeterministic`]).
    ///
    /// **NAMED FLOORS (recorded, not owned here):** reserve/settle (P-FLOW-16) fronts this dispatch;
    /// the dispatch into `runner` is GATED by AG-D4 (Agent-Fabric / CI-owned — no untrusted code runs
    /// until the sandbox-escape gate is green); re-mint (P-FLOW-17) + loop-safety (P-FLOW-18) follow.
    pub fn schedule_and_run_job<R>(
        &mut self,
        spec: JobSpec,
        runner: &R,
        timeout_secs: Option<i64>,
    ) -> WfResult<JobOutcome>
    where
        R: JobRunner,
    {
        // ── Step 1: DISPATCH (an activity, §4.4) — mint the deterministic idem_token, stamp it on the
        // spec, hand to the runner, journal activity_completed{job_dispatched}, and RETURN. The
        // `activity` primitive owns the journal row + the wf_activity_attempt ledger row + the retry +
        // the replay short-circuit; the closure here is the dispatch INTO the runner.
        //
        // The `idem_token` MUST be derived deterministically from the dispatch position so the wait
        // (step 2) and a re-drive both reconstruct the SAME token. `activity` mints its own internal
        // BUS-2 idem_token as `<run_id>/<command_id>/act`; the JOB idem_token is the parallel
        // `<run_id>/<command_id>/job` (a distinct, deterministic-on-position token the runner echoes).
        // We peek the NEXT command_id (the dispatch position) to derive the job token BEFORE the
        // activity consumes it, so the token is stable + available for the wait.
        let dispatch_command_id = self.peek_next_command_id();
        let idem_token = job_idem_token(self.run_id(), &dispatch_command_id);

        // Stamp the deterministic token on the spec the runner receives (the no-coordination
        // agreement: the runner echoes THIS token on the job.done signal — §4.9).
        let dispatched = JobSpec {
            idem_token: idem_token.clone(),
            ..spec
        };

        // The dispatch result refs the activity journals (references-not-payloads): a single marker
        // ref encoding the dispatched job's idem_token + kind, so the journaled
        // `activity_completed{job_dispatched: true, idem_token}` carries the dispatch evidence (§4.9).
        let marker_for_closure = job_dispatch_marker(&idem_token, dispatched.kind);
        let spec_for_closure = dispatched.clone();

        // Run the dispatch as an ordinary journaled activity. The closure hands the (already-stamped)
        // spec to the runner; a dispatch failure RETRIES (reusing the same idem_token — the runner
        // dedups a re-dispatched job on it, §4.9). On replay the activity SHORT-CIRCUITS (the job is
        // NOT re-dispatched) and returns the journaled marker — so the worker never re-hands the spec.
        self.activity(crate::RetryPolicy::default_policy(), move |act_idem, _attempt| {
            debug_assert!(
                act_idem.ends_with("/act"),
                "the activity's own BUS-2 token is the /act token; the JOB token is /job"
            );
            runner.dispatch(&spec_for_closure)?;
            Ok(vec![marker_for_closure.clone()])
        })?;

        // ── Step 2: PARK on the durable `job.done` signal keyed by the idem_token (§4.3). The wait
        // composes the existing wait_for_signal: it consumes a buffered job.done (the runner already
        // finished — a fast job) EXACTLY once, or parks (the run is `waiting`, holds no runtime), or
        // times out (the timeout timer bounds a vanished runner). The idem_key the wait matches is the
        // DISPATCH idem_token (the runner echoes it) — but wait_for_signal scans by signal NAME, and
        // the consume returns the consumed signal's idem_key; we VERIFY it is our token below.
        //
        // **MID-WORKFLOW TOKEN RE-MINT ON RESUME (P-FLOW-17, contract 4.7, §6.2).** The long-park
        // resume re-mints a fresh short-lived attenuated per-run token THROUGH this wait: when a prior
        // drive parked here (journaling `signal_waited` on `job.done`) and a later drive resumes by
        // consuming the runner's `job.done` (or timing out), `wait_for_signal`'s resume leg calls the
        // re-mint hook ([`WfCtx::remint_if_resuming`]) BEFORE returning — so the resumed body (which
        // settles the job + runs its continuation) executes under a token whose life == activity life,
        // never the days-long workflow life. No separate re-mint call is needed here: the long-park
        // resume IS a wait resume, so the wait owns the one re-mint (no double-mint per resume).
        let outcome = self.wait_for_signal(JOB_DONE_SIGNAL, timeout_secs)?;

        // Step 3: map the wait outcome to the job outcome. A consumed job.done is the completion (the
        // runner echoed the idem_token; we surface the result refs). A park / timeout pass through.
        match outcome {
            WaitOutcome::Signalled {
                idem_key,
                payload,
                payload_key_ref: _,
            } => {
                // The runner is REQUIRED to echo the dispatch idem_token on the job.done signal (the
                // no-coordination agreement, §4.9). A mismatch is a runner protocol violation — surface
                // it as a loud CoCommit error rather than silently completing on the wrong key (EI-01
                // §2: never a silent data-correctness bug). On replay the journaled signal carries the
                // same idem_key, so this check is replay-stable.
                if idem_key != idem_token {
                    return Err(WfError::CoCommit(format!(
                        "job.done idem_key `{idem_key}` does not match the dispatched idem_token \
                         `{idem_token}` (the runner did not echo the no-coordination dedup key, §4.9)"
                    )));
                }
                Ok(JobOutcome::Completed {
                    idem_token,
                    result: payload,
                })
            }
            WaitOutcome::Parked => Ok(JobOutcome::Parked),
            WaitOutcome::TimedOut => Ok(JobOutcome::TimedOut),
        }
    }

    /// **`metered_schedule_and_run_job(spec, runner, timeout, cost, units)` (contract 9.5/9.2/9.4,
    /// §4.9) — the `SCHEDULE_AND_RUN_JOB` long-park FRONTED by the reserve/settle bookend (P-FLOW-16).**
    /// The full step-1+4 §4.9 mechanic: **reserve `cost` minor-units at dispatch** (no balance → the
    /// job is NEVER handed to the runner, the dispatch never starts), dispatch + park (the
    /// [`WfCtx::schedule_and_run_job`] idiom), and **settle the actual `units` on the consumed
    /// `job.done`** (refunding the over-reservation into the SAME wallet a synchronous activity meters
    /// into). An in-flight job (dispatched, `Parked`/`TimedOut`) is NEVER interrupted — its reservation
    /// stays in-flight across the park and settles only on a later drive's `Completed`.
    ///
    /// **The reserve fronts the dispatch (§4.9 step 1):** it is taken BEFORE the dispatch activity, so a
    /// refused reserve (exhausted wallet) returns a loud [`WfError`] and the runner is never called. The
    /// reserve is keyed on the deterministic dispatch-position ledger run-id
    /// ([`WfCtx::dispatch_ledger_run`]) so a re-drive re-keys identically (the duplicate-reserve guard
    /// makes the replay reserve a no-op — 0 double-debit).
    ///
    /// **Settle on completion only (§4.9 step 4):** on [`JobOutcome::Completed`] the bookend settles the
    /// metered `units`. On [`JobOutcome::Parked`] / [`JobOutcome::TimedOut`] the reservation stays
    /// **in-flight** (it is never torn down — the never-interrupt-in-flight invariant); a `Parked`
    /// body returns promptly and the dispatcher re-drives it when `job.done` arrives, at which point the
    /// re-driven call's `Completed` settles. A `TimedOut` runs the body's error branch (the body
    /// settles/compensates the reservation itself, like a failed synchronous activity).
    ///
    /// An UN-METERED `WfCtx` (no [`WfCtx::with_budget`]) delegates to the plain
    /// [`WfCtx::schedule_and_run_job`] (no reserve — the loop-cap depth is the runaway bound, AG-6).
    pub fn metered_schedule_and_run_job<R>(
        &mut self,
        spec: JobSpec,
        runner: &R,
        timeout_secs: Option<i64>,
        cost: myelin_storage::reserve_settle::MinorUnits,
        units: Vec<myelin_storage::reserve_settle::MeteredUnit>,
    ) -> WfResult<JobOutcome>
    where
        R: JobRunner,
    {
        // Un-metered: no bookend wired — the plain long-park (no reserve, AG-6 loop-cap is the bound).
        let Some(gate) = self.budget().cloned() else {
            return self.schedule_and_run_job(spec, runner, timeout_secs);
        };

        // The dispatch activity is the FIRST command of this idiom; its command_id is the next position.
        // Reserve under the SAME deterministic ledger run-id the dispatch will occupy (so a re-drive
        // re-keys identically — the duplicate guard makes the replay reserve a no-op).
        let dispatch_command_id = self.peek_next_command_id();
        let ledger_run = self.dispatch_ledger_run(&dispatch_command_id);
        let tenant = self.tenant_id().clone();

        // ── RESERVE-AT-DISPATCH (§4.9 step 1). No balance → no dispatch: the runner is never called.
        // `fresh` is false on a re-drive (the duplicate guard caught an already-reserved dispatch):
        // its begin already ran on the first drive, so we do NOT re-progress the ledger.
        let fresh = match gate.reserve(&tenant, &ledger_run, cost) {
            Ok(()) => {
                // The reservation is in-flight from here — the job, once dispatched, is never
                // interrupted; it settles on completion.
                gate.begin(&tenant, &ledger_run).map_err(|e| {
                    WfError::CoCommit(format!("schedule_and_run_job begin failed: {e}"))
                })?;
                true
            }
            Err(crate::budget::BudgetError::DuplicateReservation) => false,
            Err(crate::budget::BudgetError::Refused {
                requested,
                available,
            }) => {
                // No balance → the job is NEVER handed to the runner (the dispatch never starts).
                return Err(WfError::CoCommit(format!(
                    "schedule_and_run_job refused at reserve: wallet exhausted (requested {} \
                     minor-units, {} available) — the job was never dispatched (§4.9)",
                    requested.0, available.0
                )));
            }
            Err(other) => {
                return Err(WfError::CoCommit(format!(
                    "schedule_and_run_job reserve failed: {other}"
                )));
            }
        };

        // ── DISPATCH + PARK (the existing §4.9 idiom — composes activity/signal/timer primitives).
        let outcome = self.schedule_and_run_job(spec, runner, timeout_secs)?;

        // ── SETTLE-ON-COMPLETION (§4.9 step 4) — ONLY on a fresh-drive consumed job.done. A Parked /
        // TimedOut leaves the reservation IN-FLIGHT (never interrupted): a re-drive that consumes
        // job.done settles it; a TimedOut runs the body's error branch. A re-drive (`!fresh`) whose
        // settle already ran does NOT re-settle (the replay is a pure short-circuit).
        if fresh {
            if let JobOutcome::Completed { .. } = &outcome {
                gate.settle(&tenant, &ledger_run, &units).map_err(|e| {
                    WfError::CoCommit(format!("schedule_and_run_job settle failed: {e}"))
                })?;
            }
        }
        Ok(outcome)
    }
}

/// **The DETERMINISTIC `idem_token` a `SCHEDULE_AND_RUN_JOB` mints at dispatch (§4.9).** Derived
/// PURELY from `(run_id, dispatch_command_id)` so the producer (the runner, which echoes it on
/// `job.done`) and the consumer (the workflow, which keys its `wait_for_signal` on it) agree on the
/// dedup key WITHOUT a coordination round-trip — and a re-drive re-derives the SAME token (the
/// command counter advances identically). The `/job` suffix distinguishes it from the activity's own
/// internal BUS-2 token (`/act`) at the same position. Exposed so a runner-fixture / CDC consumer can
/// derive the SAME token the workflow will key on.
pub fn job_idem_token(run_id: &str, dispatch_command_id: &str) -> String {
    format!("{run_id}/{dispatch_command_id}/job")
}

/// The references-not-payloads marker the journaled `activity_completed{job_dispatched: true,
/// idem_token}` carries (§4.9): a single [`myelin_refs::ArtifactRef`] encoding the dispatched job's
/// `idem_token` + `kind`. No PII — a machine token recording WHICH job was dispatched, so a journal /
/// holder scan can attribute the dispatch.
pub fn job_dispatch_marker(idem_token: &str, kind: JobKind) -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef(format!("job:dispatched:{}:{idem_token}", kind.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SignalRow, SignalStore};
    use crate::{RetryPolicy, WfJournal};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_signals(signals)
    }

    /// **The unified-runner fixture (the contract-8.4 `ToolHands::exec` consumer side, §4.9).** It
    /// RECORDS each dispatched [`JobSpec`] (so a test can assert the deterministic `idem_token` was
    /// stamped) and counts the dispatches (so a replay's 0-re-dispatch is provable). `fail_first` makes
    /// the first dispatch fail (to drive the activity retry, reusing the same `idem_token`).
    #[derive(Default)]
    struct RecordingRunner {
        dispatched: Mutex<Vec<JobSpec>>,
        calls: AtomicUsize,
        fail_first: bool,
    }
    impl JobRunner for RecordingRunner {
        fn dispatch(&self, spec: &JobSpec) -> Result<(), crate::ActivityError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && n == 0 {
                return Err(crate::ActivityError("runner transiently unreachable".into()));
            }
            self.dispatched.lock().unwrap().push(spec.clone());
            Ok(())
        }
    }

    fn deliver_job_done(
        signals: &SignalStore,
        idem_token: &str,
        result: Vec<ArtifactRef>,
    ) {
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: JOB_DONE_SIGNAL.into(),
            idem_key: idem_token.into(),
            payload: result,
            payload_key_ref: None,
            consumed_seq: None,
        });
    }

    /// **The `idem_token` is DETERMINISTIC from the dispatch `command_id` — producer and consumer
    /// derive the SAME key (the §4.9 no-coordination agreement).** The workflow mints it at dispatch;
    /// the runner-fixture (the producer) derives the SAME token from `(run_id, command_id)` via the
    /// exposed [`job_idem_token`]. The token the runner RECEIVED on the spec equals the one the
    /// consumer would key on — agreement WITHOUT a round-trip.
    #[test]
    fn idem_token_is_deterministic_from_command_id_producer_and_consumer_agree() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        // The dispatch is the FIRST command of this body → command_id = "merge.queue:0".
        let consumer_token = job_idem_token("R1", "merge.queue:0");

        let mut ctx = begin(&outbox, journal.clone(), signals.clone());
        let out = ctx
            .schedule_and_run_job(JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"), &runner, None)
            .expect("dispatch + park");
        assert_eq!(out, JobOutcome::Parked, "no job.done yet → the run parks");

        // PRODUCER side: the token the runner RECEIVED on the dispatched spec.
        let dispatched = runner.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1, "one dispatch");
        let producer_token = dispatched[0].idem_token.clone();

        // The two were derived independently and AGREE (the §4.9 no-coordination dedup key).
        assert_eq!(
            producer_token, consumer_token,
            "producer + consumer derive the SAME idem_token without coordination"
        );
        assert_eq!(producer_token, "R1/merge.queue:0/job", "deterministic on position");
        // the dispatch is journaled activity_completed{job_dispatched} (one history row).
        ctx.commit().expect("co-commit the dispatch + the park marker");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist[0].kind, crate::history_kind::ACTIVITY_COMPLETED, "the dispatch is journaled");
        assert_eq!(
            hist[0].result.as_ref().unwrap()[0],
            job_dispatch_marker("R1/merge.queue:0/job", JobKind::Ci),
            "the journaled dispatch carries job_dispatched: true + the idem_token"
        );
    }

    /// **The dispatch activity RETURNS immediately — the worker is freed, the workflow PARKS (§4.9).**
    /// A `SCHEDULE_AND_RUN_JOB` with no buffered `job.done` dispatches the job (one runner call) and
    /// parks (`state=waiting`, holds no runtime) — it does NOT block on the (hours-long) completion.
    #[test]
    fn dispatch_returns_immediately_and_the_workflow_parks() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .schedule_and_run_job(JobSpec::new(JobKind::Agent, "agent://acme/job/x"), &runner, None)
            .expect("dispatch + park");

        assert_eq!(out, JobOutcome::Parked, "the long-park returns Parked (the worker is freed)");
        assert!(ctx.parked_on_signal(), "the run is waiting on job.done (holds no runtime)");
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1, "the job was dispatched exactly once");
        assert_eq!(ctx.consumed_signals().len(), 0, "nothing consumed — the job is still running");
    }

    /// **A fast job whose `job.done` is ALREADY buffered completes in one drive (§4.9).** If the runner
    /// finished before the wait reached, the buffered `job.done` (keyed by the dispatch idem_token) is
    /// consumed and the long-park returns [`JobOutcome::Completed`] carrying the result refs.
    #[test]
    fn buffered_job_done_completes_with_the_result() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        // the runner already finished: job.done is buffered under the deterministic dispatch token.
        let token = job_idem_token("R1", "merge.queue:0");
        deliver_job_done(&signals, &token, vec![ArtifactRef("myelin://acme/ci/result/green".into())]);

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .schedule_and_run_job(JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"), &runner, None)
            .expect("dispatch + complete");

        match out {
            JobOutcome::Completed { idem_token, result } => {
                assert_eq!(idem_token, token, "the runner echoed the dispatch token");
                assert_eq!(result, vec![ArtifactRef("myelin://acme/ci/result/green".into())]);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(ctx.consumed_signals().len(), 1, "exactly ONE job.done consumed");
    }

    /// **A DOUBLE-delivered `job.done` wakes the workflow ONCE (the `wf_signal` PK dedup, §4.9).** The
    /// runner delivers "done" twice (at-least-once under the bus); the buffer holds ONE row (ON CONFLICT
    /// DO NOTHING on the idem_token); the long-park consumes it EXACTLY once. 1 wake per job.
    #[test]
    fn double_delivered_job_done_wakes_the_workflow_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        let token = job_idem_token("R1", "merge.queue:0");
        // DELIVERED TWICE under the SAME idem_token (at-least-once) — the PK dedups to one buffered row.
        deliver_job_done(&signals, &token, vec![ArtifactRef("myelin://acme/ci/result/green".into())]);
        deliver_job_done(&signals, &token, vec![ArtifactRef("myelin://acme/ci/result/green".into())]);
        assert_eq!(signals.buffered_depth(), 1, "the double delivery deduped to ONE buffered row");

        let mut ctx = begin(&outbox, journal, signals.clone());
        let out = ctx
            .schedule_and_run_job(JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"), &runner, None)
            .expect("dispatch + complete");
        assert!(matches!(out, JobOutcome::Completed { .. }), "the run completes, got {out:?}");
        assert_eq!(ctx.consumed_signals().len(), 1, "ONE wake per job (the double-delivery deduped)");
        assert_eq!(signals.buffered_depth(), 0, "the one buffered row is consumed once");
    }

    /// **A VANISHED runner's timeout timer fires and bounds the wait (§4.9 step 2).** The job is
    /// dispatched but the runner never reports. Drive 1 parks with a timeout. Drive 2 (the engine clock
    /// advanced past the deadline) STILL has no `job.done` → the long-park returns [`JobOutcome::
    /// TimedOut`] (the body fails/retries the job), never parking forever.
    #[test]
    fn vanished_runner_timeout_branch_fires_and_bounds_the_wait() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::timer::TimerStore::new();
        let runner = RecordingRunner::default();

        // DRIVE 1 at clock=1000 with a 100s SLA → dispatch + park (deadline 1100 not reached).
        let mut c1 = begin(&outbox, journal.clone(), signals.clone())
            .with_timers(timers.clone(), 0, 1000);
        let out1 = c1
            .schedule_and_run_job(JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"), &runner, Some(100))
            .expect("dispatch + park");
        assert_eq!(out1, JobOutcome::Parked, "dispatched, parked on job.done with an SLA timer");
        c1.commit().expect("co-commit the dispatch + the timeout-timer");
        assert_eq!(timers.armed_count(), 1, "the vanished-runner SLA timeout-timer is armed");
        let history = journal.history_for(&tenant(), "R1");

        // DRIVE 2 at clock=2000 (past the 1100 deadline), STILL no job.done → TimedOut.
        let mut c2 = WfCtx::resume(
            &outbox, minter(), journal.clone(), ctx_base(), "R1", "merge.queue",
            "2026-06-21T00:00:00Z", 42, history,
        )
        .with_signals(signals.clone())
        .with_timers(timers.clone(), 0, 2000);
        let out2 = c2
            .schedule_and_run_job(JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"), &runner, Some(100))
            .expect("the timeout drive");
        assert_eq!(out2, JobOutcome::TimedOut, "the SLA fired before the runner reported → TimedOut");
        assert_eq!(
            runner.calls.load(Ordering::SeqCst), 1,
            "the job was dispatched ONCE — the replay short-circuit did not re-dispatch it"
        );
    }

    /// **Replay re-derives the SAME token + short-circuits dispatch AND wait (§4.1).** A run that
    /// dispatched + completed, re-driven again (a later step crashed), replays the journaled dispatch
    /// (0 re-dispatch) AND the journaled `job.done` (consume-exactly-once) — it returns the SAME
    /// completion without re-handing the spec to the runner or re-consuming a second buffered signal.
    #[test]
    fn replay_short_circuits_dispatch_and_completion_with_zero_re_dispatch() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        let token = job_idem_token("R1", "merge.queue:0");
        deliver_job_done(&signals, &token, vec![ArtifactRef("myelin://acme/ci/result/green".into())]);

        // DRIVE 1: dispatch + complete + journal.
        let mut c1 = begin(&outbox, journal.clone(), signals.clone());
        let out1 = c1
            .schedule_and_run_job(JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"), &runner, None)
            .expect("drive 1");
        assert!(matches!(out1, JobOutcome::Completed { .. }));
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        // a SECOND buffered job.done under a different key — replay must NOT consume it.
        deliver_job_done(&signals, "R1/other/job", vec![ArtifactRef("myelin://acme/ci/result/other".into())]);
        let depth_before = signals.buffered_depth();

        // DRIVE 2 (re-drive): replay the dispatch (0 re-dispatch) + the journaled completion.
        let mut c2 = WfCtx::resume(
            &outbox, minter(), journal.clone(), ctx_base(), "R1", "merge.queue",
            "2026-06-21T00:00:00Z", 42, history,
        )
        .with_signals(signals.clone());
        let out2 = c2
            .schedule_and_run_job(JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"), &runner, None)
            .expect("the replay drive");
        match out2 {
            JobOutcome::Completed { idem_token, .. } => assert_eq!(
                idem_token, token,
                "replay returns the SAME journaled completion (the original token)"
            ),
            other => panic!("expected the journaled Completed, got {other:?}"),
        }
        assert_eq!(
            runner.calls.load(Ordering::SeqCst), 1,
            "0 RE-DISPATCH on replay (the dispatch short-circuited)"
        );
        assert_eq!(c2.consumed_signals().len(), 0, "replay consumed NOTHING new");
        assert_eq!(signals.buffered_depth(), depth_before, "the second job.done was NOT consumed");
    }

    /// **A runner that does NOT echo the dispatch token is a LOUD error (§4.9, EI-01 §2).** The
    /// no-coordination dedup agreement requires the runner to echo the dispatch `idem_token` on the
    /// `job.done` signal; a wrong key surfaces a CoCommit error, never a silent completion on the wrong
    /// job.
    #[test]
    fn job_done_with_a_mismatched_idem_key_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();

        // the runner delivered job.done under the WRONG key (a protocol violation).
        deliver_job_done(&signals, "the-wrong-token", vec![ArtifactRef("x://y".into())]);

        let mut ctx = begin(&outbox, journal, signals);
        let err = ctx
            .schedule_and_run_job(JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"), &runner, None)
            .expect_err("a mismatched job.done idem_key is a loud error");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("does not match the dispatched idem_token")),
            "the mismatch is a loud CoCommit error, got {err:?}"
        );
    }

    /// **A dispatch that fails RETRIES, reusing the SAME `idem_token` (§4.4/§4.9).** The runner's first
    /// dispatch fails (transiently unreachable); the activity retries (default 3 attempts) and the
    /// second succeeds — with the SAME deterministic `idem_token` (the runner dedups a re-dispatched
    /// job on it). The long-park then parks on job.done.
    #[test]
    fn a_failed_dispatch_retries_reusing_the_same_idem_token() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner {
            fail_first: true,
            ..Default::default()
        };

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .schedule_and_run_job(JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"), &runner, None)
            .expect("the retried dispatch succeeds");
        assert_eq!(out, JobOutcome::Parked, "the retried dispatch parks on job.done");
        assert_eq!(runner.calls.load(Ordering::SeqCst), 2, "one failure + one retry");
        // the SUCCESSFUL dispatch carried the SAME deterministic token the first attempt would have.
        let dispatched = runner.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1, "one accepted dispatch (the retry)");
        assert_eq!(
            dispatched[0].idem_token, "R1/merge.queue:0/job",
            "the retry reused the SAME idem_token (the runner dedups on it)"
        );
    }

    /// **The default retry-policy floor is honoured (the dispatch is an ordinary activity).** A retry
    /// budget of 3 attempts (the §4.4 floor) bounds the dispatch's transient failures; the
    /// [`RetryPolicy::default_policy`] is what `schedule_and_run_job` arms.
    #[test]
    fn dispatch_uses_the_default_retry_policy() {
        assert_eq!(RetryPolicy::default_policy().max_attempts, 3, "the §4.4 retry floor");
    }
}
