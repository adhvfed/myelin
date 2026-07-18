//! # The CDC pair for `SCHEDULE_AND_RUN_JOB` — contract 9.2 + 9.4 (PROVIDER ↔ runner consumer)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 9.2 (the
//! `WfCtx` `SCHEDULE_AND_RUN_JOB` idiom — OWNED) + 9.4 (the `job.done` durable signal wait — OWNED),
//! consuming row 8.4 (`ToolHands::exec` — the unified runner, the dispatch TARGET). Owning
//! architecture: `durable-workflow.md` §4.9 (the four-step long-park idiom).
//!
//! ## What this pair pins (the PROVIDER ↔ CONSUMER agreement of 9.2/9.4 across the runner seam)
//!
//! **9.2/9.4 PROVIDER (the workflow engine's `schedule_and_run_job`) — the agreement it guarantees:**
//! - it mints the `idem_token` DETERMINISTIC on the dispatch `command_id` and STAMPS it on the
//!   `JobSpec` it hands the runner (so producer + consumer agree on the dedup key WITHOUT a
//!   coordination round-trip);
//! - it dispatches the (already-stamped) spec into the unified runner (`ToolHands::exec`, contract
//!   8.4) and RETURNS — it does not block on the job's completion;
//! - it parks on `wait_for_signal("job.done", idem_key = idem_token)` and, on the runner's `job.done`
//!   signal, CONSUMES it exactly once (a double delivery wakes the workflow ONCE).
//!
//! **8.4 CONSUMER (the unified runner — `ToolHands::exec`, the CI runner-pool / agent-job seam):** it
//! receives the dispatched spec carrying the engine's `idem_token`, runs the job (hours later), and
//! ECHOES that `idem_token` back as the `job.done` signal's `idem_key`. This fixture adapts the REAL
//! `myelin_agent::ToolHands` trait (the frozen contract-8.4 surface) onto the engine's [`JobRunner`]
//! dispatch seam — the agreement is the SAME `idem_token` flows engine → runner → `job.done` → engine.
//!
//! **GATED BY AG-D4.** The production binding of this seam executes untrusted code in the sandbox; it
//! is GATED by the sandbox-escape gate AG-D4 (Agent-Fabric / CI-owned, `04-sandbox-AG-D4.md`) — no
//! `SCHEDULE_AND_RUN_JOB` dispatch runs untrusted code until that gate is green. This CDC proves the
//! HANDSHAKE shape (the no-coordination dedup-key agreement), not the sandbox.

use myelin_agent::{Command, ToolHands, ToolResult};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
};
use myelin_flow::engine::{SignalRow, SignalStore};
use myelin_flow::{
    job_idem_token, ActivityError, JobKind, JobOutcome, JobRunner, JobSpec, WfCtx, WfJournal,
    JOB_DONE_SIGNAL,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
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

/// **The CONSUMER fixture: the unified runner (`ToolHands::exec`, contract 8.4) behind the engine's
/// [`JobRunner`] dispatch seam.** It wraps a REAL `myelin_agent::ToolHands` impl — the frozen
/// contract-8.4 surface — and RECORDS the `idem_token` the engine stamped on each dispatched spec, so
/// the test can prove the runner SAW the engine's deterministic token (the agreement). On dispatch it
/// calls `ToolHands::exec(Command)` (the contract-8.4 hands) — the production sandbox binding is GATED
/// by AG-D4; here a `SimHands`-class echo proves the seam, not the sandbox.
struct UnifiedRunner<H: ToolHands> {
    hands: H,
    /// the idem_tokens the engine stamped on dispatched specs (proves the runner saw the agreement).
    seen_tokens: Mutex<Vec<String>>,
    /// the last `ToolResult` the hands returned (proves `exec` was actually called).
    last_exec: Mutex<Option<ToolResult>>,
}
impl<H: ToolHands> UnifiedRunner<H> {
    fn new(hands: H) -> Self {
        Self {
            hands,
            seen_tokens: Mutex::new(Vec::new()),
            last_exec: Mutex::new(None),
        }
    }
}
impl<H: ToolHands> JobRunner for UnifiedRunner<H> {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), ActivityError> {
        // record the engine's deterministic idem_token (the no-coordination dedup key, §4.9).
        self.seen_tokens
            .lock()
            .unwrap()
            .push(spec.idem_token.clone());
        // hand the job to the contract-8.4 hands (the unified sandbox runner). The Command carries the
        // job's target + the kind (a routing discriminator); the result is recorded as proof of exec.
        let cmd = Command(format!("run {} target={}", spec.kind.as_str(), spec.target));
        let res = self.hands.exec(cmd);
        *self.last_exec.lock().unwrap() = Some(res);
        Ok(())
    }
}

/// A `SimHands` (the contract-8.4 `ToolHands` test double — NO real sandbox, AG-D4-gated): it echoes
/// the command it received, proving `exec` was driven across the seam without executing untrusted code.
struct SimHands;
impl ToolHands for SimHands {
    fn exec(&self, cmd: Command) -> ToolResult {
        ToolResult(format!("dispatched: {}", cmd.0))
    }
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

fn deliver_job_done(signals: &SignalStore, idem_token: &str, result: Vec<ArtifactRef>) {
    signals.deliver(SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: "R1".into(),
        signal_name: JOB_DONE_SIGNAL.into(),
        idem_key: idem_token.into(),
        payload: result,
        payload_key_ref: None,
        received_unix_ms: 0,
        consumed_seq: None,
    });
}

/// **PROVIDER side of 9.2/9.4 (dispatch into the runner + park on job.done).** The engine mints the
/// deterministic `idem_token`, stamps it on the spec, dispatches into the contract-8.4 runner, and
/// parks. The CONSUMER (runner) SAW the engine's token — the no-coordination agreement holds; the
/// runner's `ToolHands::exec` was driven (the dispatch reached the hands).
#[test]
fn provider_dispatches_into_the_runner_and_parks_consumer_sees_the_token() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let runner = UnifiedRunner::new(SimHands);

    let mut ctx = begin(&outbox, journal, signals);
    let out = ctx
        .schedule_and_run_job(
            JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
            &runner,
            Some(3600),
        )
        .expect("dispatch + park");

    // PROVIDER promise: the long-park parks (the runner is running the job; the run holds no runtime).
    assert_eq!(out, JobOutcome::Parked, "dispatched, parked on job.done");

    // CONSUMER (runner) saw the engine's DETERMINISTIC token (the agreement, §4.9).
    let consumer_token = job_idem_token("R1", "merge.queue:0");
    let seen = runner.seen_tokens.lock().unwrap();
    assert_eq!(
        seen.as_slice(),
        &[consumer_token],
        "the runner saw the engine's deterministic token"
    );

    // the contract-8.4 hands were actually driven (the dispatch reached `ToolHands::exec`).
    let exec = runner.last_exec.lock().unwrap();
    assert_eq!(
        *exec,
        Some(ToolResult(
            "dispatched: run ci target=pipeline://acme/ci/pr-7".into()
        )),
        "the dispatch reached the unified runner's ToolHands::exec (contract 8.4 consumed)"
    );
}

/// **CONSUMER side of 9.4 (the runner echoes the token on job.done → the workflow wakes once).** The
/// runner finishes and delivers `signal(run, "job.done", {result}, idem_key = idem_token)` — echoing
/// the SAME token the engine stamped. The workflow's `wait_for_signal` consumes it EXACTLY once and
/// completes with the job's references-not-payloads result. A double-delivery wakes the workflow once.
#[test]
fn consumer_echoes_the_token_on_job_done_and_the_workflow_wakes_once() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let runner = UnifiedRunner::new(SimHands);

    // DRIVE 1: dispatch + park.
    let mut c1 = begin(&outbox, journal.clone(), signals.clone());
    assert_eq!(
        c1.schedule_and_run_job(
            JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
            &runner,
            None
        )
        .expect("dispatch"),
        JobOutcome::Parked
    );
    c1.commit().expect("co-commit the dispatch + park");
    let history = journal.history_for(&tenant(), "R1");

    // the runner finished — it ECHOES the engine's token on job.done, delivered TWICE (at-least-once).
    let token = job_idem_token("R1", "merge.queue:0");
    let result = vec![ArtifactRef("myelin://acme/ci/result/green".into())];
    deliver_job_done(&signals, &token, result.clone());
    deliver_job_done(&signals, &token, result.clone());
    assert_eq!(
        signals.buffered_depth(),
        1,
        "the double delivery deduped to ONE row (wf_signal PK)"
    );

    // DRIVE 2 (re-lease): the long-park resumes, consumes job.done ONCE, completes with the result.
    let mut c2 = WfCtx::resume(
        &outbox,
        minter(),
        journal.clone(),
        ctx_base(),
        "R1",
        "merge.queue",
        "2026-06-21T00:00:00Z",
        42,
        history,
    )
    .with_signals(signals.clone());
    let out = c2
        .schedule_and_run_job(
            JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
            &runner,
            None,
        )
        .expect("resume + complete");
    match out {
        JobOutcome::Completed {
            idem_token,
            result: got,
        } => {
            assert_eq!(
                idem_token, token,
                "the runner echoed the engine's token (agreement held)"
            );
            assert_eq!(got, result, "the job's references-not-payloads result");
        }
        other => panic!("expected Completed, got {other:?}"),
    }
    assert_eq!(
        c2.consumed_signals().len(),
        1,
        "ONE wake per job (the double delivery deduped)"
    );
    assert_eq!(
        signals.buffered_depth(),
        0,
        "the one buffered job.done is consumed once"
    );
}
