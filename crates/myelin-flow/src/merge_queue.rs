//! # `merge_queue` — the merge-queue durable workflow body (P-FLOW-19 → P-215, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §6.5 (the merge-queue
//! durable workflow + the `ci.result` rollup wait — ONE workflow per target ref; for each queued
//! PR: compute the speculative merge commit, dispatch the required CI via the `SCHEDULE_AND_RUN_JOB`
//! idiom (reserve at dispatch, return immediately), `wait_for_signal("ci.result",
//! idem_key=<merge_attempt_id>)` parking with NO runtime, a timeout branch bounding a vanished CI
//! run; on a `success` `ci.result` for ALL required contexts → merge + emit `git.pr.merged` via the
//! outbox + settle; on `failure`/`error` → dequeue the PR with a humanised reason and continue) +
//! §4.9 (the long-park idiom it rides) + §4.3 (the durable signal).
//!
//! **Contract-index cluster:** OWNS contract 9.4 (the `ci.result` wait — the durable half).
//! CONSUMES contract 5.9 (the CI-owned `CheckStatus` / `ci.result` data shape — imported from
//! [`myelin_events::check_seam`], NEVER redefined here) + 7.3 (humanise — the dequeue reason).
//!
//! ## What this prompt (P-FLOW-19) ships — THE MERGE-QUEUE BODY, IN ISOLATION
//!
//! The merge-queue body is the **durable-execution half of the X-1 seam**, built and drilled in
//! isolation against a **MOCK `ci.result` producer** ([`MockCiResultProducer`]). It owns ONLY the
//! durable-workflow mechanics; it imports the [`CiResult`] / [`CiOverall`] data shape (contract 5.9)
//! from `myelin-events`. The REAL `ci.result` producer is CI (M4, contract 5.9) — the seam goes live
//! end-to-end in **P-FLOW-22** (the NAMED FLOOR this prompt opens).
//!
//! The body, per queued PR ([`run_merge_attempt`]):
//!
//! 1. **Compute the speculative merge commit + dispatch the required CI (an activity, §4.9 step 1).**
//!    The merge-queue mints the deterministic `merge_attempt_id` AT dispatch (so CI and the workflow
//!    agree on the `ci.result` `idem_key` WITHOUT coordination), stamps it on the [`CiDispatch`],
//!    hands it to the [`CiDispatcher`] (the unified-runner seam, contract 8.4 — GATED by AG-D4),
//!    **reserves budget at dispatch** (contract 11.7 — no balance → the CI is NEVER dispatched), and
//!    RETURNS. The activity worker is freed.
//! 2. **Park on `ci.result` (a durable signal wait, §4.3/§6.5).**
//!    `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` flips the run `state='waiting'`,
//!    holding NO runtime while CI runs for hours; the timeout branch (§4.2) bounds a vanished CI run.
//! 3. **Resume on the rollup (a signal, hours later).** CI delivers
//!    `signal(run, "ci.result", {CiResult}, idem_key=<merge_attempt_id>)`. A DOUBLE delivery wakes
//!    the workflow ONCE (the `wf_signal` PK dedup). The body decodes the [`CiResult`] verdict:
//!    - `overall == success` AND all required contexts present → **MERGE** (emit `git.pr.merged`
//!      via the outbox, BUS-2) + **settle** budget → [`MergeOutcome::Merged`];
//!    - `overall == failure` (or a required context missing) → **DEQUEUE** the PR with a humanised
//!      reason (contract 7.3, [`humanise_dequeue_reason`]) → [`MergeOutcome::Dequeued`];
//!    - the timeout fired (a vanished CI run) → dequeue with the vanished-run reason →
//!      [`MergeOutcome::TimedOut`].
//!
//! The whole sequence is fully deterministic under replay (the dispatch + the wait short-circuit
//! their journaled rows; the `merge_attempt_id` re-derives identically) — the merge-queue holds NO
//! runtime across a multi-hour CI run and resumes exactly where it parked, across worker restarts and
//! deploys (the §6.5 / FLOW-D4 property).
//!
//! ## references-not-payloads
//!
//! The `ci.result` signal payload is references-not-payloads: the [`CiResult`] verdict
//! (`commit_oid`, `overall`, `contexts`, `idem_token` — all PII-free machine tokens) is encoded into
//! the signal's `Vec<ArtifactRef>` by [`encode_ci_result`] and decoded by [`decode_ci_result`] — a
//! deterministic codec over the CI-owned shape, never a redefinition of it. No inline PII ever rides
//! the merge-queue signal.
//!
//! ## NAMED FLOORS (this prompt ships the body against a MOCK producer ONLY)
//!
//! - **The X-1 seam end-to-end** (GIT-D10 / CI-D8 against CI's REAL `ci.result` producer) → the M4
//!   gate, follow-on **P-FLOW-22**. This prompt is the merge-queue FLOOR: built + drilled in
//!   isolation against [`MockCiResultProducer`]; the real producer wires in M4.
//! - **The CI dispatch into the unified runner is GATED by AG-D4** (the sandbox-escape drill,
//!   Agent-Fabric / CI-owned, `04-sandbox-AG-D4.md`). The [`CiDispatcher`] is the seam the engine
//!   calls; the production binding (onto `ToolHands::exec`, contract 8.4) lands behind that gate.
//!   RECORDED here, not owned.

use crate::wfctx::{WaitOutcome, WfCtx, WfError, WfResult};
use myelin_events::check_seam::{CiOverall, CiResult};
use myelin_events::{
    AggregateKey, ArtifactRef as EvArtifactRef, DataRole, EventDraft, EventId, EventType,
    Visibility,
};
use myelin_refs::ArtifactRef;

/// **The FROZEN durable-signal name the merge-queue parks on (§6.5/§4.3).** Pinned to the NAMED
/// `ci.result` token via [`CiResult`]'s own substrate name in `myelin-events` (never a literal), so
/// the merge-queue workflow and CI's producer agree on the signal name by construction. One of the
/// FROZEN signal-name vocabulary (`approval` / `cancel` / `ci.result` / `job.done`, §5.1).
pub const CI_RESULT_SIGNAL: &str = myelin_events::check_seam::CiResultWaitSubstrate::SIGNAL_NAME;

/// **The event the merge-queue emits via the outbox on a successful merge (§6.5, BUS-2).** A
/// registered taxonomy token (`myelin_events` seed); emitted ONCE per merge (the co-commit makes it
/// exactly-once with the journaled merge).
pub const GIT_PR_MERGED_EVENT: &str = "git.pr.merged";

/// **A PR queued into the merge queue (§6.5) — references-not-payloads.** One workflow serialises
/// merges into a busy `target_ref`; each [`MergeRequest`] is one queued PR's merge attempt input.
/// All fields are PII-free machine refs/ids (a PR ref, a target branch ref, the speculative merge
/// commit OID) — never an inline PII body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeRequest {
    /// The PR being merged (an opaque PR ref — Git owns the grammar; the engine carries it opaquely).
    pub pr_ref: String,
    /// The target ref this queue serialises into (`refs/heads/main` of a repo — one workflow per
    /// target ref, §6.5).
    pub target_ref: String,
    /// The speculative merge commit OID the workflow computed for this attempt (the commit CI runs
    /// against). PII-free (a git OID).
    pub speculative_commit_oid: String,
    /// The required CI contexts that MUST be `success` for the merge to proceed (Git owns "required",
    /// contract 5.9; the engine carries the list). The merge proceeds ONLY when the `ci.result`
    /// rollup reports `success` AND every one of these contexts is present.
    pub required_contexts: Vec<String>,
}

/// **What the unified runner is handed to dispatch the required CI for one merge attempt (§6.5/§4.9
/// step 1).** references-not-payloads. The `merge_attempt_id` is minted by the workflow at dispatch
/// (DETERMINISTIC on the dispatch position) and stamped here so CI echoes it back on the `ci.result`
/// signal's `idem_key` — the no-coordination dedup agreement (§4.9), exactly the [`crate::JobSpec`]
/// `idem_token` property.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiDispatch {
    /// The speculative merge commit CI runs against (PII-free git OID).
    pub commit_oid: String,
    /// The required contexts CI must report (PII-free context identifiers).
    pub required_contexts: Vec<String>,
    /// **The `merge_attempt_id` the workflow minted at dispatch (DETERMINISTIC on the dispatch
    /// position).** Stamped here so CI echoes it on the `ci.result` signal's `idem_key` — producer
    /// and consumer agree on the dedup key WITHOUT a coordination round-trip (§4.9 / §6.5). A
    /// double-delivered `ci.result` under THIS key wakes the workflow ONCE.
    pub merge_attempt_id: String,
}

/// **The CI-dispatch TARGET seam — the unified runner the merge queue hands its [`CiDispatch`] to
/// (contract 8.4 CONSUMED, §6.5/§4.9).** This is the merge-queue's view of `ToolHands::exec` (the
/// unified sandbox runner, ADR-20 / X-6): the workflow `dispatch`es the required CI and the runner
/// accepts it for asynchronous execution — it does NOT block (CI runs for hours and later delivers
/// `signal(run, "ci.result", {CiResult}, idem_key=<merge_attempt_id>)`).
///
/// **GATED BY AG-D4.** The production binding executes untrusted CI in the sandbox — it MUST NOT run
/// until the sandbox-escape gate AG-D4 is green (Agent-Fabric / CI-owned, `04-sandbox-AG-D4.md`).
/// The merge queue OWNS this trait (the dispatch seam); it does NOT own the sandbox. The
/// `merge_attempt_id` on the [`CiDispatch`] is already stamped when `dispatch` is called.
///
/// `dispatch` returns `Ok(())` if the runner ACCEPTED the CI for execution (the dispatch succeeded,
/// not the CI run), or a [`crate::ActivityError`] if the dispatch itself failed (unreachable /
/// rejected) — a dispatch failure RETRIES like any activity (§4.4), reusing the SAME
/// `merge_attempt_id` (CI dedups a re-dispatched run on it).
pub trait CiDispatcher {
    /// Hand the (already `merge_attempt_id`-stamped) [`CiDispatch`] to the unified runner for
    /// asynchronous CI execution. Returns `Ok(())` on a dispatch the runner ACCEPTED (the rollup
    /// arrives later as the `ci.result` signal), or a [`crate::ActivityError`] if the dispatch failed.
    fn dispatch(&self, ci: &CiDispatch) -> Result<(), crate::ActivityError>;
}

/// **The seam that performs the actual git merge once CI is green (§6.5 step 3).** Distinct from the
/// CI dispatch: this is the Git-owned merge of the speculative commit into the target ref, run as a
/// journaled activity (so a crash mid-merge replays to the un-journaled step — resumable, no
/// re-executed merge). The merge-queue calls it; Git owns the merge mechanics (the merge gate, M3).
/// Returns the merged commit OID on success, or a [`crate::ActivityError`] on a failed merge (a merge
/// conflict the speculative commit could not resolve) — which dequeues the PR with a humanised reason.
pub trait MergePerformer {
    /// Perform the merge of `request.speculative_commit_oid` into `request.target_ref`. Returns the
    /// merged commit OID (PII-free) on success, or a [`crate::ActivityError`] on a failed merge.
    fn merge(&self, request: &MergeRequest) -> Result<String, crate::ActivityError>;
}

/// **The outcome of one merge attempt ([`run_merge_attempt`], §6.5).** A merge attempt either MERGES
/// (CI green for all required contexts → merge + `git.pr.merged` emit + settle), DEQUEUES (CI failed
/// or a required context missing → a humanised reason, the queue continues), PARKS (CI is running —
/// the run is `waiting`, holds NO runtime, until the `ci.result` arrives), or TIMES OUT (CI vanished;
/// the timeout-timer fired → dequeue with the vanished-run reason).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeOutcome {
    /// **CI was `success` for ALL required contexts → the PR MERGED (§6.5 step 3).** Carries the
    /// merged commit OID (the [`MergePerformer`] result) and the `merge_attempt_id` CI echoed (the
    /// dedup agreement held). The body emitted `git.pr.merged` via the outbox EXACTLY once and settled
    /// budget. On replay this returns the SAME journaled merge (one merge per attempt across a
    /// re-drive).
    Merged {
        /// the `merge_attempt_id` CI echoed (= the minted dispatch id — the agreement held).
        merge_attempt_id: String,
        /// the merged commit OID (the [`MergePerformer`] result) — PII-free.
        merged_commit_oid: String,
    },
    /// **CI was `failure` (or a required context was missing) → the PR was DEQUEUED with a humanised
    /// reason (§6.5 step 3, contract 7.3).** The queue CONTINUES with the next PR; no merge, no
    /// `git.pr.merged`. The `reason` is the contract-7.3 humanised dequeue reason
    /// ([`humanise_dequeue_reason`]).
    Dequeued {
        /// the humanised dequeue reason (contract 7.3) — operator-readable, no raw error codes.
        reason: String,
    },
    /// **CI is DISPATCHED + the workflow PARKED on `ci.result` (§6.5 step 2).** The run is `waiting`,
    /// holds NO runtime, until CI delivers `signal(run, "ci.result", …)`. The body returns promptly
    /// on a `Parked`; the dispatcher re-drives it when the signal arrives.
    Parked,
    /// **CI VANISHED — the timeout timer fired before the rollup arrived (§6.5 step 2).** A CI run
    /// that never reports does NOT park the workflow forever: the timeout branch dequeues the PR with
    /// the vanished-run reason ([`MergeOutcome::TimedOut`] carries no reason — the body's caller
    /// dequeues with [`humanise_dequeue_reason`]'s vanished-run text). The queue continues.
    TimedOut,
}

/// **Humanise a merge-queue dequeue reason (contract 7.3, §6.5 step 3).** A dequeue surfaces an
/// operator-readable reason, NEVER a raw error code or a stack — the contract-7.3 humanise rule.
/// Closed over the merge-queue's dequeue causes (a CI failure, a missing required context, a vanished
/// CI run, a failed merge). Deterministic (replay-stable) — the same cause maps to the same string.
pub fn humanise_dequeue_reason(cause: DequeueCause) -> String {
    match cause {
        DequeueCause::CiFailure { ref failing } if !failing.is_empty() => format!(
            "CI failed: the required check(s) {} did not pass. The pull request was removed from \
             the merge queue; push a fix and re-queue.",
            failing.join(", ")
        ),
        DequeueCause::CiFailure { .. } => "CI reported a failure for this pull request. It was \
             removed from the merge queue; push a fix and re-queue."
            .to_string(),
        DequeueCause::MissingRequiredContext { ref missing } => format!(
            "CI did not report the required check(s) {}. The pull request was removed from the \
             merge queue; ensure those checks run and re-queue.",
            missing.join(", ")
        ),
        DequeueCause::CiVanished => "CI did not report a result before the time limit (the run \
             may have stalled). The pull request was removed from the merge queue; re-queue to try \
             again."
            .to_string(),
        DequeueCause::MergeConflict => "The merge could not be completed (the branch likely \
             conflicts with the target). The pull request was removed from the merge queue; rebase \
             and re-queue."
            .to_string(),
    }
}

/// **The closed set of merge-queue dequeue causes (contract 7.3, §6.5).** The merge queue dequeues a
/// PR ONLY for these reasons; each maps to a humanised operator-readable string via
/// [`humanise_dequeue_reason`]. A closed enum (not a free string) so a new dequeue cause is a
/// compile-time addition, never an un-humanised raw error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DequeueCause {
    /// CI reported `overall: failure` — one or more required checks did not pass.
    CiFailure {
        /// the required contexts that were NOT reported `success` (PII-free identifiers).
        failing: Vec<String>,
    },
    /// CI reported `success` overall but a required context was absent from the rollup's `contexts`.
    MissingRequiredContext {
        /// the required contexts missing from the `ci.result` rollup.
        missing: Vec<String>,
    },
    /// CI never reported (the timeout-timer fired) — a vanished CI run.
    CiVanished,
    /// The git merge itself failed (a conflict the speculative commit could not resolve).
    MergeConflict,
}

/// **The DETERMINISTIC `merge_attempt_id` the merge queue mints at dispatch (§6.5/§4.9).** Derived
/// PURELY from `(run_id, dispatch_command_id)` so the producer (CI, which echoes it on `ci.result`)
/// and the consumer (the workflow, which keys its `wait_for_signal("ci.result", …)` on it) agree on
/// the dedup key WITHOUT a coordination round-trip — and a re-drive re-derives the SAME id (the
/// command counter advances identically). The `/merge` suffix distinguishes it from a generic
/// `job.done` long-park's `/job` token at the same position. Exposed so a CI-producer fixture / CDC
/// consumer derives the SAME id the workflow will key on.
pub fn merge_attempt_id(run_id: &str, dispatch_command_id: &str) -> String {
    format!("{run_id}/{dispatch_command_id}/merge")
}

// ---------------------------------------------------------------------------
// The references-not-payloads codec for the ci.result signal (§3.4 / §6.5)
// ---------------------------------------------------------------------------

/// The ref-prefix the `ci.result` rollup verdict is encoded under (a machine token, no PII).
const CI_RESULT_VERDICT_PREFIX: &str = "ci.result:verdict:";
/// The ref-prefix each rolled-up context is encoded under.
const CI_RESULT_CONTEXT_PREFIX: &str = "ci.result:context:";
/// The ref-prefix the rollup's commit OID is encoded under.
const CI_RESULT_COMMIT_PREFIX: &str = "ci.result:commit:";

/// **Encode a [`CiResult`] rollup into the `ci.result` signal's references-not-payloads body
/// (§3.4/§6.5).** The CI-owned [`CiResult`] (`commit_oid`, `overall`, `contexts`, `idem_token`) is
/// flattened into a deterministic `Vec<ArtifactRef>` of PII-free machine tokens — the merge-queue
/// signal NEVER carries an inline PII body. The producer (CI, or [`MockCiResultProducer`]) calls this
/// to build the signal payload; the consumer ([`run_merge_attempt`]) decodes it with
/// [`decode_ci_result`]. The `idem_token` is NOT encoded in the body — it is the signal's `idem_key`
/// (= the `merge_attempt_id`), carried by the signal envelope, not the payload.
pub fn encode_ci_result(result: &CiResult) -> Vec<ArtifactRef> {
    let mut refs = Vec::with_capacity(result.contexts.len() + 2);
    let verdict = match result.overall {
        CiOverall::Success => "success",
        CiOverall::Failure => "failure",
    };
    refs.push(ArtifactRef(format!("{CI_RESULT_VERDICT_PREFIX}{verdict}")));
    refs.push(ArtifactRef(format!(
        "{CI_RESULT_COMMIT_PREFIX}{}",
        result.commit_oid
    )));
    for ctx in &result.contexts {
        refs.push(ArtifactRef(format!("{CI_RESULT_CONTEXT_PREFIX}{ctx}")));
    }
    refs
}

/// **Decode a `ci.result` signal's references-not-payloads body back into a [`CiResult`]
/// (§3.4/§6.5).** The inverse of [`encode_ci_result`]. `idem_token` is the consumed signal's
/// `idem_key` (the `merge_attempt_id`), supplied by the caller from the signal envelope (it is not in
/// the payload refs). Returns `None` if the body is malformed (a missing verdict — a producer
/// protocol violation the caller surfaces LOUD, never a silent wrong-merge).
pub fn decode_ci_result(refs: &[ArtifactRef], idem_token: &str) -> Option<CiResult> {
    let mut overall = None;
    let mut commit_oid = String::new();
    let mut contexts = Vec::new();
    for r in refs {
        if let Some(v) = r.0.strip_prefix(CI_RESULT_VERDICT_PREFIX) {
            overall = match v {
                "success" => Some(CiOverall::Success),
                "failure" => Some(CiOverall::Failure),
                _ => return None,
            };
        } else if let Some(c) = r.0.strip_prefix(CI_RESULT_COMMIT_PREFIX) {
            commit_oid = c.to_string();
        } else if let Some(ctx) = r.0.strip_prefix(CI_RESULT_CONTEXT_PREFIX) {
            contexts.push(ctx.to_string());
        }
    }
    Some(CiResult {
        commit_oid,
        overall: overall?,
        contexts,
        idem_token: idem_token.to_string(),
    })
}

impl WfCtx {
    /// **`run_merge_attempt(request, ci, merger, timeout, cost, units)` (contract 9.4/5.9/7.3, §6.5)
    /// — ONE merge attempt of the merge-queue durable workflow body.** Dispatch the required CI
    /// (reserve at dispatch) + park on `ci.result` + merge-or-dequeue. Composes the existing activity
    /// (§4.4) / signal (§4.3) / timer (§4.2) / reserve-settle (11.7) primitives — NO new engine.
    ///
    /// **The three steps (§6.5):**
    /// 1. **Dispatch the required CI.** Mints the `merge_attempt_id` DETERMINISTIC on the dispatch
    ///    position (so CI and the workflow agree on the `ci.result` `idem_key` WITHOUT coordination),
    ///    stamps it on a [`CiDispatch`], **reserves `cost` budget** (no balance → the CI is NEVER
    ///    dispatched), hands it to `ci` ([`CiDispatcher::dispatch`] = `ToolHands::exec`, GATED by
    ///    AG-D4) as a journaled activity, and RETURNS. The worker is freed.
    /// 2. **Park on `ci.result`.** `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` with
    ///    the optional `timeout_secs` arming a durable timeout-timer that bounds a vanished CI run.
    ///    The run flips `state='waiting'`, holds NO runtime.
    /// 3. **Resume on the rollup.** On the consumed `ci.result`: `success` for ALL `required_contexts`
    ///    → perform the merge (`merger`), emit `git.pr.merged` via the outbox, **settle** budget →
    ///    [`MergeOutcome::Merged`]; `failure` / a missing required context → dequeue with a humanised
    ///    reason → [`MergeOutcome::Dequeued`]; the timeout fired → [`MergeOutcome::TimedOut`].
    ///
    /// **Replay (§4.1):** the dispatch SHORT-CIRCUITS (the CI is NOT re-dispatched), the wait
    /// short-circuits to the SAME journaled `ci.result` (consume-exactly-once), and the merge activity
    /// short-circuits its journaled result (one merge per attempt). The whole attempt is deterministic.
    ///
    /// **A double-delivered `ci.result` wakes the workflow ONCE** (the `wf_signal` PK dedup) → ONE
    /// merge, ONE `git.pr.merged`. A vanished CI run's timeout-timer fires and bounds the wait.
    ///
    /// **NAMED FLOORS (recorded, not owned here):** the X-1 seam end-to-end against CI's REAL producer
    /// is **P-FLOW-22** (M4); the CI dispatch into `ci` is GATED by **AG-D4** (no untrusted code until
    /// the sandbox-escape gate is green).
    #[allow(clippy::too_many_arguments)]
    pub fn run_merge_attempt<D, M>(
        &mut self,
        request: &MergeRequest,
        ci: &D,
        merger: &M,
        timeout_secs: Option<i64>,
        cost: myelin_storage::reserve_settle::MinorUnits,
        units: Vec<myelin_storage::reserve_settle::MeteredUnit>,
    ) -> WfResult<MergeOutcome>
    where
        D: CiDispatcher,
        M: MergePerformer,
    {
        // ── Step 1: COMPUTE the speculative merge commit + DISPATCH the required CI (an activity,
        // §6.5 step 1). The merge_attempt_id is minted DETERMINISTIC on the dispatch position BEFORE
        // the activity consumes the counter, so the wait (step 2) and a re-drive reconstruct the SAME
        // id. The reserve-at-dispatch fronts the dispatch (no balance → no CI), riding the existing
        // metered_activity bookend (11.7) — exactly the SCHEDULE_AND_RUN_JOB reserve property (§4.9).
        let dispatch_command_id = self.peek_next_command_id();
        let attempt_id = merge_attempt_id(self.run_id(), &dispatch_command_id);

        let dispatch = CiDispatch {
            commit_oid: request.speculative_commit_oid.clone(),
            required_contexts: request.required_contexts.clone(),
            merge_attempt_id: attempt_id.clone(),
        };
        // The journaled dispatch marker (references-not-payloads): records WHICH CI was dispatched
        // under WHICH merge_attempt_id, so a journal/holder scan attributes the dispatch.
        let marker = ci_dispatch_marker(&attempt_id, &request.speculative_commit_oid);
        let dispatch_for_closure = dispatch.clone();
        let marker_for_closure = marker.clone();

        // Reserve-at-dispatch + dispatch as a journaled, retried activity. An un-metered WfCtx runs
        // WITHOUT a reserve (the loop-cap is the runaway bound, AG-6) — metered_activity handles both.
        // A refused reserve (exhausted wallet) surfaces LOUD: the CI is NEVER dispatched.
        self.metered_activity(
            crate::RetryPolicy::default_policy(),
            cost,
            units,
            move |_act_idem, _attempt| {
                ci.dispatch(&dispatch_for_closure)?;
                Ok(vec![marker_for_closure.clone()])
            },
        )?;

        // ── Step 2: PARK on the durable `ci.result` signal keyed by the merge_attempt_id (§4.3/§6.5).
        // wait_for_signal scans by signal NAME and returns the consumed signal's idem_key; we VERIFY
        // it is OUR merge_attempt_id below (one workflow per target ref processes merges SERIALLY, so
        // there is exactly one outstanding ci.result wait at a time). The optional timeout arms the
        // vanished-CI-run timeout-timer. A buffered ci.result (CI already finished — a fast run)
        // resumes in one drive; otherwise the run parks, holding no runtime.
        let outcome = self.wait_for_signal(CI_RESULT_SIGNAL, timeout_secs)?;

        // ── Step 3: resume on the rollup — merge / dequeue / park / timeout (§6.5 step 3).
        match outcome {
            WaitOutcome::Signalled {
                idem_key,
                payload,
                payload_key_ref: _,
            } => {
                // CI is REQUIRED to echo the merge_attempt_id on the ci.result signal (the
                // no-coordination agreement, §6.5/§4.9). A mismatch is a producer protocol violation —
                // surface it LOUD (EI-01 §2: never a silent wrong-merge), never merge on the wrong key.
                if idem_key != attempt_id {
                    return Err(WfError::CoCommit(format!(
                        "ci.result idem_key `{idem_key}` does not match the dispatched \
                         merge_attempt_id `{attempt_id}` (CI did not echo the no-coordination dedup \
                         key, §6.5)"
                    )));
                }
                // Decode the CI-owned CiResult verdict from the references-not-payloads body. A
                // malformed body (no verdict) is a producer protocol violation → LOUD, never a silent
                // dequeue/merge on a missing verdict.
                let result = decode_ci_result(&payload, &idem_key).ok_or_else(|| {
                    WfError::CoCommit(format!(
                        "ci.result for merge_attempt `{attempt_id}` carried no decodable verdict \
                         (a producer protocol violation, §6.5)"
                    ))
                })?;

                // The merge proceeds ONLY on `success` AND every required context present (§6.5 — Git
                // owns "required", the engine enforces the rollup carries them).
                if result.overall == CiOverall::Failure {
                    // CI failed → dequeue with a humanised reason; the queue continues.
                    let failing: Vec<String> = request.required_contexts.clone();
                    let reason = humanise_dequeue_reason(DequeueCause::CiFailure { failing });
                    return Ok(MergeOutcome::Dequeued { reason });
                }
                let missing: Vec<String> = request
                    .required_contexts
                    .iter()
                    .filter(|req| !result.contexts.iter().any(|c| c == *req))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    // success overall but a required context was absent → dequeue (not all required
                    // contexts green); the queue continues.
                    let reason =
                        humanise_dequeue_reason(DequeueCause::MissingRequiredContext { missing });
                    return Ok(MergeOutcome::Dequeued { reason });
                }

                // ── SUCCESS FOR ALL REQUIRED CONTEXTS → MERGE (§6.5 step 3). Perform the merge as a
                // journaled activity (a crash mid-merge replays to the un-journaled step — resumable,
                // no re-executed merge). A failed merge (a conflict) → dequeue with a humanised reason.
                let request_for_merge = request.clone();
                let merge_result = self.activity(
                    crate::RetryPolicy::default_policy(),
                    move |_act_idem, _attempt| {
                        let oid = merger.merge(&request_for_merge)?;
                        Ok(vec![ArtifactRef(format!("git:merged:{oid}"))])
                    },
                );
                let merged_commit_oid = match merge_result {
                    Ok(refs) => refs
                        .first()
                        .and_then(|r| r.0.strip_prefix("git:merged:"))
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    Err(WfError::ActivityExhausted(_)) => {
                        // The merge could not be completed (a conflict) → dequeue, NOT merge.
                        let reason = humanise_dequeue_reason(DequeueCause::MergeConflict);
                        return Ok(MergeOutcome::Dequeued { reason });
                    }
                    Err(other) => return Err(other),
                };

                // EMIT `git.pr.merged` via the outbox (BUS-2) — ONCE per merge (the co-commit makes it
                // exactly-once with the journaled merge). references-not-payloads: the payload carries
                // the PR ref + the merged commit OID (PII-free machine tokens), never a PII body.
                self.emit(git_pr_merged_draft(request, &merged_commit_oid), None)?;

                Ok(MergeOutcome::Merged {
                    merge_attempt_id: attempt_id,
                    merged_commit_oid,
                })
            }
            WaitOutcome::Parked => Ok(MergeOutcome::Parked),
            WaitOutcome::TimedOut => Ok(MergeOutcome::TimedOut),
        }
    }
}

/// The references-not-payloads marker the journaled CI-dispatch activity carries (§6.5): a single
/// [`ArtifactRef`] recording the dispatched `merge_attempt_id` + the speculative commit OID. No PII —
/// a machine token recording WHICH CI was dispatched for WHICH merge attempt.
pub fn ci_dispatch_marker(merge_attempt_id: &str, commit_oid: &str) -> ArtifactRef {
    ArtifactRef(format!("ci:dispatched:{merge_attempt_id}:{commit_oid}"))
}

/// Build the `git.pr.merged` [`EventDraft`] the merge queue emits via the outbox on a successful
/// merge (§6.5, BUS-2). references-not-payloads: the payload carries the PR ref, the target ref, and
/// the merged commit OID — all PII-free machine tokens, never a PII body. The aggregate is the PR
/// (so all events about one PR share an ordering partition).
pub fn git_pr_merged_draft(request: &MergeRequest, merged_commit_oid: &str) -> EventDraft {
    EventDraft {
        type_: EventType(GIT_PR_MERGED_EVENT.to_string()),
        subject: EvArtifactRef(request.pr_ref.clone()),
        aggregate: AggregateKey(request.pr_ref.clone()),
        payload: serde_json::json!({
            "pr_ref": request.pr_ref,
            "target_ref": request.target_ref,
            "merged_commit_oid": merged_commit_oid,
        }),
        // The fact a PR merged is controller metadata (the platform controls the merge fact).
        data_role: DataRole::Controller,
        // A merge fact is internal to the repo's members.
        visibility: Visibility::Internal,
        // references-not-payloads — no inline PII.
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

// ---------------------------------------------------------------------------
// The MOCK ci.result producer harness (this engine does NOT own the real producer — that is CI, M4)
// ---------------------------------------------------------------------------

/// **The MOCK `ci.result` producer harness (§6.5 — the merge-queue-in-isolation drill fixture).**
/// This engine does NOT own the real `ci.result` producer (that is CI, M4, contract 5.9 — the NAMED
/// FLOOR P-FLOW-22). The mock lets the merge-queue body be drilled IN ISOLATION: it builds a
/// [`CiResult`] rollup for a `merge_attempt_id` (deriving the SAME id the workflow mints, via
/// [`merge_attempt_id`]) and delivers it into the run's [`crate::SignalStore`] as a `ci.result`
/// signal — including the at-least-once DOUBLE delivery the drill asserts wakes the workflow ONCE.
///
/// It models the producer side of the §6.5 no-coordination agreement: it keys the signal on the
/// `merge_attempt_id` (which it derives independently of the workflow) so a drill proves producer +
/// consumer agree WITHOUT a coordination round-trip.
pub struct MockCiResultProducer<'a> {
    signals: &'a crate::SignalStore,
    tenant: myelin_tenancy::TenantId,
    region: myelin_tenancy::Region,
    run_id: String,
}

impl<'a> MockCiResultProducer<'a> {
    /// A mock producer bound to a run's signal store + `(tenant, region)` partition.
    pub fn new(
        signals: &'a crate::SignalStore,
        tenant: myelin_tenancy::TenantId,
        region: myelin_tenancy::Region,
        run_id: impl Into<String>,
    ) -> Self {
        Self {
            signals,
            tenant,
            region,
            run_id: run_id.into(),
        }
    }

    /// **Deliver a `ci.result` rollup for a merge attempt (§6.5).** Builds the [`CiResult`] verdict,
    /// encodes it references-not-payloads ([`encode_ci_result`]), and delivers it into the run's
    /// signal store keyed on `merge_attempt_id` (= the signal `idem_key`). Returns `true` if this was
    /// a NEW delivery (the first for this `idem_key`) and `false` if it was an at-least-once DUPLICATE
    /// (the `wf_signal` PK deduped it — the workflow still wakes ONCE).
    pub fn deliver(
        &self,
        merge_attempt_id: &str,
        commit_oid: &str,
        overall: CiOverall,
        contexts: Vec<String>,
    ) -> bool {
        let result = CiResult {
            commit_oid: commit_oid.to_string(),
            overall,
            contexts,
            idem_token: merge_attempt_id.to_string(),
        };
        self.signals.deliver(crate::SignalRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            run_id: self.run_id.clone(),
            signal_name: CI_RESULT_SIGNAL.to_string(),
            idem_key: merge_attempt_id.to_string(),
            payload: encode_ci_result(&result),
            payload_key_ref: None,
            consumed_seq: None,
        })
    }
}

/// The type the merge-queue emits (`git.pr.merged`) is a registered taxonomy token. A doc-link
/// re-export so the merge-queue's emit type is discoverable alongside the body. (Suppress a
/// dead-code lint: `EventId` is used in the public signature of [`WfCtx::run_merge_attempt`] via
/// [`WfCtx::emit`].)
#[allow(dead_code)]
type EmittedEventId = EventId;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::SignalStore;
    use crate::WfJournal;
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::reserve_settle::MinorUnits;
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

    fn request() -> MergeRequest {
        MergeRequest {
            pr_ref: "myelin://acme/git/repo/core#pr-7".into(),
            target_ref: "refs/heads/main".into(),
            speculative_commit_oid: "deadbeef".into(),
            required_contexts: vec!["build".into(), "test".into()],
        }
    }

    /// **The unified-runner fixture for the CI dispatch (the contract-8.4 seam, §6.5).** It RECORDS
    /// each dispatched [`CiDispatch`] (so a test asserts the deterministic `merge_attempt_id` was
    /// stamped) and counts the dispatches (so a replay's 0-re-dispatch is provable). `fail_first` makes
    /// the first dispatch fail (to drive the activity retry, reusing the same id).
    #[derive(Default)]
    struct RecordingCi {
        dispatched: Mutex<Vec<CiDispatch>>,
        calls: AtomicUsize,
        fail_first: bool,
    }
    impl CiDispatcher for RecordingCi {
        fn dispatch(&self, ci: &CiDispatch) -> Result<(), crate::ActivityError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && n == 0 {
                return Err(crate::ActivityError("CI runner transiently unreachable".into()));
            }
            self.dispatched.lock().unwrap().push(ci.clone());
            Ok(())
        }
    }

    /// **A [`MergePerformer`] fixture.** It counts the merges (so a test asserts EXACTLY one merge per
    /// attempt) and returns the merged OID. `conflict` makes the merge fail (a conflict → dequeue).
    #[derive(Default)]
    struct RecordingMerger {
        merges: AtomicUsize,
        conflict: bool,
    }
    impl MergePerformer for RecordingMerger {
        fn merge(&self, request: &MergeRequest) -> Result<String, crate::ActivityError> {
            self.merges.fetch_add(1, Ordering::SeqCst);
            if self.conflict {
                return Err(crate::ActivityError("merge conflict".into()));
            }
            Ok(format!("merged-{}", request.speculative_commit_oid))
        }
    }

    fn no_cost() -> (MinorUnits, Vec<myelin_storage::reserve_settle::MeteredUnit>) {
        (MinorUnits(0), vec![])
    }

    /// **The dispatch mints the `merge_attempt_id` DETERMINISTIC on the dispatch position — producer
    /// and consumer agree WITHOUT coordination (§6.5/§4.9).** With no buffered `ci.result` the run
    /// dispatches CI (one call) and PARKS on `ci.result` (holds no runtime). The id the runner
    /// received == the id [`merge_attempt_id`] derives independently.
    #[test]
    fn dispatch_mints_deterministic_attempt_id_and_parks() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("dispatch + park");
        assert_eq!(out, MergeOutcome::Parked, "no ci.result yet → the run parks");
        assert!(ctx.parked_on_signal(), "parked on ci.result (holds no runtime)");
        assert_eq!(ci.calls.load(Ordering::SeqCst), 1, "CI dispatched exactly once");
        assert_eq!(merger.merges.load(Ordering::SeqCst), 0, "no merge — CI still running");

        // The id the runner received == the id derived independently from (run_id, command_id).
        let dispatched = ci.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1);
        let consumer_id = merge_attempt_id("R1", "merge.queue:0");
        assert_eq!(
            dispatched[0].merge_attempt_id, consumer_id,
            "producer + consumer derive the SAME merge_attempt_id without coordination"
        );
        assert_eq!(dispatched[0].merge_attempt_id, "R1/merge.queue:0/merge");
    }

    /// **CI success for ALL required contexts → exactly ONE merge + ONE `git.pr.merged` emit (§6.5).**
    /// A buffered `ci.result` (success, all required contexts) resumes in one drive: the body merges
    /// once and emits `git.pr.merged` once via the outbox.
    #[test]
    fn success_for_all_required_contexts_merges_and_emits_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        // CI already finished green for build+test.
        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        producer.deliver(
            &attempt,
            "deadbeef",
            CiOverall::Success,
            vec!["build".into(), "test".into()],
        );

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("dispatch + merge");
        match out {
            MergeOutcome::Merged {
                merge_attempt_id: id,
                merged_commit_oid,
            } => {
                assert_eq!(id, attempt, "CI echoed the dispatch id");
                assert_eq!(merged_commit_oid, "merged-deadbeef");
            }
            other => panic!("expected Merged, got {other:?}"),
        }
        assert_eq!(merger.merges.load(Ordering::SeqCst), 1, "EXACTLY one merge");
        assert_eq!(ctx.staged_emit_len(), 1, "EXACTLY one git.pr.merged emitted");
    }

    /// **A double-delivered `ci.result` wakes the workflow ONCE → 0 double-merge (§6.5).** The
    /// at-least-once transport delivers the SAME rollup twice; the `wf_signal` PK dedups it to one
    /// buffered row; the body merges ONCE and emits `git.pr.merged` ONCE.
    #[test]
    fn double_delivered_ci_result_wakes_once_zero_double_merge() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        // DELIVERED TWICE (at-least-once) under the same merge_attempt_id.
        let first = producer.deliver(&attempt, "deadbeef", CiOverall::Success, vec!["build".into(), "test".into()]);
        let second = producer.deliver(&attempt, "deadbeef", CiOverall::Success, vec!["build".into(), "test".into()]);
        assert!(first, "first delivery is new");
        assert!(!second, "the at-least-once double-delivery deduped (ON CONFLICT DO NOTHING)");
        assert_eq!(signals.buffered_depth(), 1, "ONE buffered row");

        let mut ctx = begin(&outbox, journal, signals.clone());
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("merge");
        assert!(matches!(out, MergeOutcome::Merged { .. }), "merged, got {out:?}");
        assert_eq!(ctx.consumed_signals().len(), 1, "ONE wake per attempt");
        assert_eq!(merger.merges.load(Ordering::SeqCst), 1, "0 double-merge");
        assert_eq!(ctx.staged_emit_len(), 1, "ONE git.pr.merged");
    }

    /// **CI failure → ONE dequeue with a HUMANISED reason; the queue continues (§6.5, contract 7.3).**
    /// A `failure` `ci.result` dequeues the PR with an operator-readable reason — no merge, no
    /// `git.pr.merged`.
    #[test]
    fn ci_failure_dequeues_with_humanised_reason() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        producer.deliver(&attempt, "deadbeef", CiOverall::Failure, vec!["build".into(), "test".into()]);

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("dequeue");
        match out {
            MergeOutcome::Dequeued { reason } => {
                assert!(reason.contains("CI failed"), "humanised: {reason}");
                assert!(reason.contains("build"), "names the failing checks: {reason}");
                assert!(!reason.contains("ActivityError"), "no raw error code: {reason}");
            }
            other => panic!("expected Dequeued, got {other:?}"),
        }
        assert_eq!(merger.merges.load(Ordering::SeqCst), 0, "no merge on failure");
        assert_eq!(ctx.staged_emit_len(), 0, "no git.pr.merged on failure");
    }

    /// **A required context MISSING from a `success` rollup → dequeue (not all required green, §6.5).**
    /// CI reports `success` overall but the rollup omits a required context → the merge does NOT
    /// proceed; the PR is dequeued with a humanised reason naming the missing context.
    #[test]
    fn success_missing_a_required_context_dequeues() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();
        let (cost, units) = no_cost();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        // success overall but only `build` reported — `test` (required) is missing.
        producer.deliver(&attempt, "deadbeef", CiOverall::Success, vec!["build".into()]);

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, cost, units)
            .expect("dequeue");
        match out {
            MergeOutcome::Dequeued { reason } => {
                assert!(reason.contains("test"), "names the missing required context: {reason}");
            }
            other => panic!("expected Dequeued, got {other:?}"),
        }
        assert_eq!(merger.merges.load(Ordering::SeqCst), 0, "no merge — not all required green");
    }

    /// **A vanished CI run's timeout-timer fires and bounds the wait → TimedOut (§6.5 step 2).** CI is
    /// dispatched but never reports. Drive 1 parks with a timeout. Drive 2 (clock past the deadline)
    /// STILL has no `ci.result` → [`MergeOutcome::TimedOut`] (the queue's caller dequeues), 0
    /// re-dispatch.
    #[test]
    fn vanished_ci_run_times_out_and_bounds_the_wait() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::timer::TimerStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();

        // DRIVE 1 at clock=1000 with a 100s SLA → dispatch + park (deadline 1100 not reached).
        let mut c1 = begin(&outbox, journal.clone(), signals.clone()).with_timers(timers.clone(), 0, 1000);
        let out1 = c1
            .run_merge_attempt(&request(), &ci, &merger, Some(100), MinorUnits(0), vec![])
            .expect("dispatch + park");
        assert_eq!(out1, MergeOutcome::Parked, "dispatched, parked with an SLA timer");
        c1.commit().expect("co-commit the dispatch + the timeout-timer");
        assert_eq!(timers.armed_count(), 1, "the vanished-CI SLA timeout-timer is armed");
        let history = journal.history_for(&tenant(), "R1");

        // DRIVE 2 at clock=2000 (past the 1100 deadline), STILL no ci.result → TimedOut.
        let mut c2 = WfCtx::resume(
            &outbox, minter(), journal.clone(), ctx_base(), "R1", "merge.queue",
            "2026-06-21T00:00:00Z", 42, history,
        )
        .with_signals(signals.clone())
        .with_timers(timers.clone(), 0, 2000);
        let out2 = c2
            .run_merge_attempt(&request(), &ci, &merger, Some(100), MinorUnits(0), vec![])
            .expect("the timeout drive");
        assert_eq!(out2, MergeOutcome::TimedOut, "the SLA fired before CI reported → TimedOut");
        assert_eq!(
            ci.calls.load(Ordering::SeqCst), 1,
            "CI dispatched ONCE — the replay short-circuit did not re-dispatch it"
        );
        assert_eq!(merger.merges.load(Ordering::SeqCst), 0, "no merge on a vanished CI run");
    }

    /// **A failed merge (a conflict) → dequeue, NOT merge (§6.5).** CI is green but the git merge
    /// itself fails (a conflict the speculative commit could not resolve) → the PR is dequeued with a
    /// humanised reason; no `git.pr.merged`.
    #[test]
    fn a_merge_conflict_dequeues() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger {
            conflict: true,
            ..Default::default()
        };

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        producer.deliver(&attempt, "deadbeef", CiOverall::Success, vec!["build".into(), "test".into()]);

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, MinorUnits(0), vec![])
            .expect("dequeue on conflict");
        match out {
            MergeOutcome::Dequeued { reason } => {
                assert!(reason.contains("merge could not be completed"), "humanised: {reason}");
            }
            other => panic!("expected Dequeued, got {other:?}"),
        }
        assert_eq!(ctx.staged_emit_len(), 0, "no git.pr.merged on a failed merge");
    }

    /// **CI that does NOT echo the merge_attempt_id is a LOUD error (§6.5/§4.9, EI-01 §2).** The
    /// no-coordination agreement requires CI to echo the dispatch `merge_attempt_id` on `ci.result`; a
    /// wrong key surfaces a CoCommit error, never a silent merge on the wrong attempt.
    #[test]
    fn ci_result_with_mismatched_idem_key_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();

        // CI delivered ci.result under the WRONG key (a protocol violation).
        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        producer.deliver("the-wrong-attempt-id", "deadbeef", CiOverall::Success, vec!["build".into(), "test".into()]);

        let mut ctx = begin(&outbox, journal, signals);
        let err = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, MinorUnits(0), vec![])
            .expect_err("a mismatched ci.result idem_key is a loud error");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("does not match the dispatched")),
            "loud CoCommit, got {err:?}"
        );
    }

    /// **A failed CI dispatch RETRIES, reusing the SAME merge_attempt_id (§4.4/§6.5).** The first
    /// dispatch fails (transiently unreachable); the activity retries; the second succeeds with the
    /// SAME deterministic id (CI dedups a re-dispatched run on it). The body then parks on ci.result.
    #[test]
    fn a_failed_ci_dispatch_retries_reusing_the_same_attempt_id() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi {
            fail_first: true,
            ..Default::default()
        };
        let merger = RecordingMerger::default();

        let mut ctx = begin(&outbox, journal, signals);
        let out = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, MinorUnits(0), vec![])
            .expect("the retried dispatch parks");
        assert_eq!(out, MergeOutcome::Parked, "the retried dispatch parks on ci.result");
        assert_eq!(ci.calls.load(Ordering::SeqCst), 2, "one failure + one retry");
        let dispatched = ci.dispatched.lock().unwrap();
        assert_eq!(dispatched.len(), 1, "one accepted dispatch (the retry)");
        assert_eq!(
            dispatched[0].merge_attempt_id, "R1/merge.queue:0/merge",
            "the retry reused the SAME merge_attempt_id (CI dedups on it)"
        );
    }

    /// **Replay re-derives the SAME id + short-circuits dispatch, wait, AND merge (§4.1/§6.5).** A run
    /// that dispatched + merged, re-driven (a later step crashed), replays the journaled dispatch (0
    /// re-dispatch), the journaled `ci.result` (consume-exactly-once), AND the journaled merge (0
    /// re-merge) — returning the SAME merge without re-handing the spec or re-merging.
    #[test]
    fn replay_short_circuits_dispatch_wait_and_merge() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();

        let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        producer.deliver(&attempt, "deadbeef", CiOverall::Success, vec!["build".into(), "test".into()]);

        // DRIVE 1: dispatch + merge + emit + journal.
        let mut c1 = begin(&outbox, journal.clone(), signals.clone());
        let out1 = c1
            .run_merge_attempt(&request(), &ci, &merger, None, MinorUnits(0), vec![])
            .expect("drive 1");
        assert!(matches!(out1, MergeOutcome::Merged { .. }));
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        // DRIVE 2 (re-drive): replay all three journaled steps with 0 re-execution.
        let mut c2 = WfCtx::resume(
            &outbox, minter(), journal.clone(), ctx_base(), "R1", "merge.queue",
            "2026-06-21T00:00:00Z", 42, history,
        )
        .with_signals(signals.clone());
        let out2 = c2
            .run_merge_attempt(&request(), &ci, &merger, None, MinorUnits(0), vec![])
            .expect("the replay drive");
        match out2 {
            MergeOutcome::Merged { merge_attempt_id: id, .. } => {
                assert_eq!(id, attempt, "replay returns the SAME journaled merge")
            }
            other => panic!("expected the journaled Merged, got {other:?}"),
        }
        assert_eq!(ci.calls.load(Ordering::SeqCst), 1, "0 RE-DISPATCH on replay");
        assert_eq!(merger.merges.load(Ordering::SeqCst), 1, "0 RE-MERGE on replay");
        assert_eq!(c2.consumed_signals().len(), 0, "replay consumed NOTHING new");
    }

    /// **The codec round-trips a [`CiResult`] through the references-not-payloads signal body
    /// (§3.4/§6.5).** Encode → decode reconstructs the verdict; no inline PII rides the signal.
    #[test]
    fn ci_result_codec_round_trips() {
        let result = CiResult {
            commit_oid: "deadbeef".into(),
            overall: CiOverall::Success,
            contexts: vec!["build".into(), "test".into()],
            idem_token: "R1/merge.queue:0/merge".into(),
        };
        let refs = encode_ci_result(&result);
        // every ref is a PII-free machine token.
        assert!(refs.iter().all(|r| r.0.starts_with("ci.result:")), "machine tokens only");
        let back = decode_ci_result(&refs, "R1/merge.queue:0/merge").expect("decodable");
        assert_eq!(back, result, "encode → decode round-trips the verdict");
    }

    /// **A malformed `ci.result` body (no verdict) is a LOUD error (EI-01 §2).** A producer that
    /// delivers a ci.result with no decodable verdict surfaces a CoCommit error, never a silent
    /// dequeue/merge on a missing verdict.
    #[test]
    fn ci_result_with_no_verdict_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let ci = RecordingCi::default();
        let merger = RecordingMerger::default();

        // Deliver a malformed ci.result: the right idem_key but no verdict ref in the body.
        let attempt = merge_attempt_id("R1", "merge.queue:0");
        signals.deliver(crate::SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: CI_RESULT_SIGNAL.into(),
            idem_key: attempt,
            payload: vec![ArtifactRef("ci.result:context:build".into())], // no verdict ref
            payload_key_ref: None,
            consumed_seq: None,
        });

        let mut ctx = begin(&outbox, journal, signals);
        let err = ctx
            .run_merge_attempt(&request(), &ci, &merger, None, MinorUnits(0), vec![])
            .expect_err("a verdict-less ci.result is a loud error");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("no decodable verdict")),
            "loud CoCommit, got {err:?}"
        );
    }

    /// **The signal name is the NAMED `ci.result` token (the merge queue and CI agree on it).**
    #[test]
    fn ci_result_signal_name_is_the_named_token() {
        assert_eq!(CI_RESULT_SIGNAL, "ci.result");
    }

    /// **`git.pr.merged` carries references-not-payloads (no inline PII).**
    #[test]
    fn git_pr_merged_draft_is_references_not_payloads() {
        let draft = git_pr_merged_draft(&request(), "merged-deadbeef");
        assert_eq!(draft.type_.0, "git.pr.merged");
        assert!(!draft.contains_personal_data, "references-not-payloads");
        assert_eq!(draft.payload["merged_commit_oid"], "merged-deadbeef");
    }

    /// **Each dequeue cause humanises to an operator-readable string with NO raw error code (contract
    /// 7.3).**
    #[test]
    fn every_dequeue_cause_humanises() {
        let causes = [
            DequeueCause::CiFailure { failing: vec!["build".into()] },
            DequeueCause::CiFailure { failing: vec![] },
            DequeueCause::MissingRequiredContext { missing: vec!["test".into()] },
            DequeueCause::CiVanished,
            DequeueCause::MergeConflict,
        ];
        for cause in causes {
            let reason = humanise_dequeue_reason(cause.clone());
            assert!(!reason.is_empty(), "humanised reason is non-empty for {cause:?}");
            assert!(!reason.contains("ActivityError"), "no raw error code in {reason:?}");
            assert!(!reason.contains("Err("), "no debug formatting in {reason:?}");
        }
    }
}
