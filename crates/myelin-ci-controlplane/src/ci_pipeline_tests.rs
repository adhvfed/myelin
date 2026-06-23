//! Unit tests for the `ci.pipeline` durable workflow body (CI-P15 → P-358, M4).
//!
//! These exercise the body's stage-gating + per-context X-1 producer emit IN PROCESS over the FROZEN
//! `myelin-flow` `WfCtx` surface (a metered `WfCtx` with a signal buffer + timer wheel + outbox), the
//! exact substrate the CI-D9/CI-D1 drills (`myelin-flow/tests/drills_ci_pipeline.rs`) prove
//! bit-identical/effectively-once on the engine the body composes. The producer-side X-1 facts
//! (per-context `ci.check.updated` + `ci.run.*` + the `ci.result` rollup) are asserted off the
//! committed outbox.
//!
//! The end-to-end run UNDER THE DISPATCHER (with the runner-fixture `job.done` delivery + the
//! bit-identical replay assertion) is the CI-P15 drill in `tests/drills_ci_p15_ci_pipeline.rs`.

use super::*;
use myelin_ci_sandbox::events::{
    CI_CHECK_UPDATED, CI_DEPLOYMENT_REJECTED, CI_RESULT, CI_RUN_FAILED, CI_RUN_SUCCEEDED,
};
use myelin_events::{Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore};
use myelin_flow::engine::SignalRow;
use myelin_flow::MinorUnits;
use myelin_flow::{
    job_idem_token, stage_verdict_marker, BudgetGate, CiStage, SignalStore, TimerStore, Wallet,
    WfCtx, WfJournal,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

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
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: myelin_events::Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: myelin_events::Timestamp("2026-06-23T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:run-7".into())),
    }
}
fn minter() -> std::sync::Arc<dyn IdMinter> {
    std::sync::Arc::new(MonotonicMinter::new())
}

/// A recording CI runner fixture (the contract-8.4 `ToolHands::exec` consumer side, §4.9). Counts
/// dispatches so a test proves 0 wasted dispatch on a doomed run.
#[derive(Default)]
struct RecordingRunner {
    calls: std::sync::atomic::AtomicUsize,
}
impl JobRunner for RecordingRunner {
    fn dispatch(&self, spec: &myelin_flow::JobSpec) -> Result<(), myelin_flow::ActivityError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            spec.kind,
            myelin_flow::JobKind::Ci,
            "a CI pipeline dispatches kind=ci jobs"
        );
        Ok(())
    }
}
impl RecordingRunner {
    fn count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The X-1 emit context for the reference run (repo / commit / run ref / trust / rollup idem_token).
fn facts() -> CheckFacts {
    CheckFacts {
        repo: "myelin://acme/git/repo/r1".into(),
        commit_oid: "deadbeef".into(),
        run_ref: "myelin://acme/ci/run/run-7".into(),
        run_attempt: 1,
        trust_tier: "trusted".into(),
        merge_idem_token: "merge-attempt-7".into(),
    }
}

/// A two-context run (`build` + `test`) over two runner stages (no gate). Un-metered (cost 0) so the
/// engine takes the plain long-park path (no wallet needed).
fn job_run() -> PipelineRun {
    PipelineRun {
        stages: vec![
            PipelineStage::job(CiStage::new(
                "build",
                "pipeline://acme/ci/run-7#build",
                MinorUnits(0),
                Some(3600),
            )),
            PipelineStage::job(CiStage::new(
                "test",
                "pipeline://acme/ci/run-7#test",
                MinorUnits(0),
                Some(3600),
            )),
        ],
        contexts: vec!["build".into(), "test".into()],
        facts: facts(),
    }
}

/// Begin a `ci.pipeline` WfCtx with a signal buffer + timer wheel + outbox (no wallet → un-metered).
fn begin(outbox: &OutboxStore, signals: SignalStore, timers: TimerStore, now_secs: i64) -> WfCtx {
    WfCtx::begin(
        outbox,
        minter(),
        WfJournal::new(),
        ctx_base(),
        "run-7",
        CI_PIPELINE_WF_TYPE,
        "2026-06-23T00:00:00Z",
        7,
    )
    .with_signals(signals)
    .with_timers(timers, 0, now_secs)
}

/// Begin a metered WfCtx (a wallet behind it) — for the budget/gate paths.
fn begin_metered(
    outbox: &OutboxStore,
    signals: SignalStore,
    timers: TimerStore,
    balance: MinorUnits,
    now_secs: i64,
) -> WfCtx {
    begin(outbox, signals, timers, now_secs).with_budget(BudgetGate::new(Wallet::new(balance)))
}

/// The dispatch `idem_token` for the Nth RUNNER stage (each stage = 2 command positions: dispatch
/// `2*idx`, wait `2*idx + 1`). With NO leading gate the engine pipeline begins at command position 0.
fn stage_token(stage_idx: usize) -> String {
    job_idem_token("run-7", &format!("{CI_PIPELINE_WF_TYPE}:{}", stage_idx * 2))
}

/// The dispatch `idem_token` for the Nth runner stage when ONE leading gate consumed command
/// position 0 (the gate's `wait_for_signal`), so the engine pipeline begins at command position 1.
fn stage_token_after_one_gate(stage_idx: usize) -> String {
    job_idem_token(
        "run-7",
        &format!("{CI_PIPELINE_WF_TYPE}:{}", 1 + stage_idx * 2),
    )
}

/// Deliver a runner stage's `job.done` carrying the verdict marker, keyed on the dispatch token.
fn deliver_done(signals: &SignalStore, token: &str, stage: &str, pass: bool) {
    signals.deliver(SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: "run-7".into(),
        signal_name: myelin_flow::JOB_DONE_SIGNAL.into(),
        idem_key: token.into(),
        payload: vec![stage_verdict_marker(stage, pass)],
        payload_key_ref: None,
        consumed_seq: None,
    });
}

/// Deliver an `approval:<stage>` decision (approve, or decline via the DECLINE_MARKER ref).
fn deliver_approval(signals: &SignalStore, stage: &str, approve: bool) {
    let payload = if approve {
        vec![myelin_refs::ArtifactRef("approve".into())]
    } else {
        vec![myelin_refs::ArtifactRef(myelin_flow::DECLINE_MARKER.into())]
    };
    signals.deliver(SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: "run-7".into(),
        signal_name: myelin_flow::approval_wait_name(stage),
        idem_key: format!("approval:{stage}"),
        payload,
        payload_key_ref: None,
        consumed_seq: None,
    });
}

/// The committed event types in order (the X-1 producer assertion surface).
fn emitted_types(ctx: WfCtx, outbox: &OutboxStore) -> Vec<String> {
    ctx.commit().expect("co-commit the body's emits");
    outbox
        .committed_rows()
        .into_iter()
        .map(|r| r.envelope.type_.0)
        .collect()
}

/// **The body PARKS at the first runner stage holding no runtime (no `job.done` yet).** The body
/// dispatches stage `build` (one `kind=ci` job) and parks on its `job.done` — it does NOT block on
/// the multi-hour build, and it emits NO terminal X-1 facts yet (a parked run is not terminal).
#[test]
fn body_parks_at_the_first_stage_no_terminal_facts() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    let mut ctx = begin(&outbox, signals, timers, 1000);
    let verdict = run_ci_pipeline_body(&mut ctx, &job_run(), &runner).expect("dispatch + park");

    assert_eq!(verdict, RunVerdict::Parked, "parks on build's job.done");
    assert!(ctx.parked_on_signal(), "the run holds no runtime (waiting)");
    assert_eq!(runner.count(), 1, "ONE stage dispatched (build)");
    // No terminal facts on a parked run.
    let types = emitted_types(ctx, &outbox);
    assert!(
        !types
            .iter()
            .any(|t| t == CI_RUN_SUCCEEDED || t == CI_RUN_FAILED || t == CI_RESULT),
        "a parked run emits no terminal run/result fact, got {types:?}"
    );
}

/// **Every stage passes → SUCCESS: a `ci.check.updated{success}` PER context + `ci.run.succeeded` +
/// the `ci.result{success}` rollup (arch §4).** Both stages' `job.done` buffered green; one drive
/// runs the whole pipeline and emits the X-1 producer facts.
#[test]
fn all_stages_pass_emits_success_checks_run_succeeded_and_ci_result() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    deliver_done(&signals, &stage_token(0), "build", true);
    deliver_done(&signals, &stage_token(1), "test", true);

    let mut ctx = begin(&outbox, signals, timers, 1000);
    let verdict = run_ci_pipeline_body(&mut ctx, &job_run(), &runner).expect("green run");

    assert_eq!(
        verdict,
        RunVerdict::Succeeded {
            stages_completed: 2
        },
        "both stages passed"
    );
    assert_eq!(runner.count(), 2, "TWO stages dispatched");

    let types = emitted_types(ctx, &outbox);
    // TWO per-context terminal checks (build + test), then ci.run.succeeded, then the ci.result rollup.
    let checks = types.iter().filter(|t| *t == CI_CHECK_UPDATED).count();
    assert_eq!(
        checks, 2,
        "one terminal ci.check.updated PER context, got {types:?}"
    );
    assert!(
        types.contains(&CI_RUN_SUCCEEDED.to_string()),
        "ci.run.succeeded emitted"
    );
    assert!(
        types.contains(&CI_RESULT.to_string()),
        "the ci.result rollup emitted"
    );
    assert!(
        !types.contains(&CI_RUN_FAILED.to_string()),
        "a green run never emits ci.run.failed"
    );
}

/// **A failing stage → FAILURE: a `ci.check.updated{failure}` PER context + `ci.run.failed`
/// (structured) + `ci.result{failure}`; the later stage is NEVER dispatched (0 wasted spend).** The
/// `build` stage reports `fail`; `test` is never dispatched.
#[test]
fn a_failing_stage_emits_failure_checks_run_failed_and_stops_fast() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    deliver_done(&signals, &stage_token(0), "build", false); // build FAILS
                                                             // test's job.done is NOT delivered — it must never be dispatched.

    let mut ctx = begin(&outbox, signals, timers, 1000);
    let verdict = run_ci_pipeline_body(&mut ctx, &job_run(), &runner).expect("fails fast at build");

    assert_eq!(
        verdict,
        RunVerdict::Failed {
            stage: "build".into()
        },
        "failed at build"
    );
    assert_eq!(
        runner.count(),
        1,
        "ONLY build dispatched — test was never dispatched"
    );

    let types = emitted_types(ctx, &outbox);
    // Per-context failure checks are emitted for EVERY reported context (build + test), then
    // ci.run.failed, then ci.result{failure} — the gate must learn the whole context set failed.
    let checks = types.iter().filter(|t| *t == CI_CHECK_UPDATED).count();
    assert_eq!(
        checks, 2,
        "a failure fact PER context (the whole gate set), got {types:?}"
    );
    assert!(
        types.contains(&CI_RUN_FAILED.to_string()),
        "ci.run.failed emitted"
    );
    assert!(
        types.contains(&CI_RESULT.to_string()),
        "the ci.result{{failure}} rollup emitted"
    );
    assert!(
        !types.contains(&CI_RUN_SUCCEEDED.to_string()),
        "a failed run never emits ci.run.succeeded"
    );
}

/// **The `ci.result` rollup carries the run's verdict + the merge idem_token (X-1, §4 step 4).** The
/// rollup the body emits decodes to `overall: success` over the run's required context set, keyed on
/// the merge-attempt id the merge queue echoes (a double-delivery wakes the merge queue ONCE).
#[test]
fn the_ci_result_rollup_carries_the_verdict_and_the_merge_idem_token() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    deliver_done(&signals, &stage_token(0), "build", true);
    deliver_done(&signals, &stage_token(1), "test", true);

    let mut ctx = begin(&outbox, signals, timers, 1000);
    run_ci_pipeline_body(&mut ctx, &job_run(), &runner).expect("green run");
    ctx.commit().expect("co-commit");

    // Find the ci.result row + decode its payload.
    let row = outbox
        .committed_rows()
        .into_iter()
        .find(|r| r.envelope.type_.0 == CI_RESULT)
        .expect("a ci.result rollup was emitted");
    let result: myelin_events::check_seam::CiResult =
        serde_json::from_value(row.envelope.payload.clone()).expect("the rollup decodes");
    assert_eq!(result.commit_oid, "deadbeef");
    assert_eq!(
        result.overall,
        myelin_events::check_seam::CiOverall::Success
    );
    assert_eq!(
        result.idem_token, "merge-attempt-7",
        "the rollup is keyed on the merge-attempt id the merge queue echoes (OQ-F)"
    );
    assert_eq!(
        result.contexts,
        vec!["build".to_string(), "test".to_string()],
        "the rollup's context set (sorted)"
    );
}

/// **A terminal `ci.check.updated` fact carries the X-1 shape Git's gate decodes.** The per-context
/// fact decodes to `myelin_git::check_status::CheckStatus` with `state: success`, the stamped
/// `trust_tier` (read off the run, never recomputed), and the monotonic `run_attempt`.
#[test]
fn a_terminal_check_fact_decodes_to_the_frozen_git_checkstatus_shape() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    deliver_done(&signals, &stage_token(0), "build", true);
    deliver_done(&signals, &stage_token(1), "test", true);

    let mut ctx = begin(&outbox, signals, timers, 1000);
    run_ci_pipeline_body(&mut ctx, &job_run(), &runner).expect("green run");
    ctx.commit().expect("co-commit");

    let row = outbox
        .committed_rows()
        .into_iter()
        .find(|r| r.envelope.type_.0 == CI_CHECK_UPDATED)
        .expect("a ci.check.updated fact was emitted");
    let payload = &row.envelope.payload;
    assert_eq!(payload["state"], "success", "terminal success state");
    assert_eq!(
        payload["trust_tier"], "trusted",
        "the trust_tier stamped at trigger time (never recomputed, X-1)"
    );
    assert_eq!(payload["run_attempt"], 1, "the monotonic supersession key");
    assert_eq!(
        payload["cost_settled"], false,
        "terminal but not settled until CI-P17's reserve/settle bookend closes (X-1 cost gate)"
    );
    // The subject grammar is byte-identical to the frozen check_seam subject (no drift from Git).
    let expected =
        myelin_events::check_seam::check_subject("myelin://acme/git/repo/r1", "deadbeef", "build");
    assert_eq!(
        row.envelope.subject, expected,
        "the X-1 subject grammar (no drift)"
    );
}

/// **A protected-env GATE is APPROVED → the runner stages run + the run succeeds.** A leading gate
/// stage parks on `approval:<stage>`; with the approval buffered it proceeds to the runner stages.
#[test]
fn an_approved_gate_proceeds_to_the_runner_stages() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    let run = PipelineRun {
        stages: vec![
            PipelineStage::gate(CiStage::new(
                "deploy-approval",
                "gate://acme/ci/run-7#deploy",
                MinorUnits(0),
                Some(86_400),
            )),
            PipelineStage::job(CiStage::new(
                "deploy",
                "pipeline://acme/ci/run-7#deploy",
                MinorUnits(0),
                Some(3600),
            )),
        ],
        contexts: vec!["deploy".into()],
        facts: facts(),
    };

    deliver_approval(&signals, "deploy-approval", true);
    // After the gate consumes command position 0, the deploy stage dispatch is at position 1.
    deliver_done(&signals, &stage_token_after_one_gate(0), "deploy", true);

    let mut ctx = begin(&outbox, signals, timers, 1000);
    let verdict = run_ci_pipeline_body(&mut ctx, &run, &runner).expect("approved → runs");
    assert_eq!(
        verdict,
        RunVerdict::Succeeded {
            stages_completed: 1
        }
    );
    assert_eq!(runner.count(), 1, "the deploy stage ran after the approval");

    let types = emitted_types(ctx, &outbox);
    assert!(
        types.contains(&CI_RUN_SUCCEEDED.to_string()),
        "ci.run.succeeded after the gate"
    );
    assert!(
        !types.contains(&CI_DEPLOYMENT_REJECTED.to_string()),
        "an approved gate never emits ci.deployment.rejected"
    );
}

/// **A DENIED protected-env GATE → `ci.deployment.rejected` + the gated stages NEVER run (0 wasted
/// spend on a rejected deploy, §3.1).** The gate's approval decision is a decline; the deploy stage
/// is never dispatched.
#[test]
fn a_denied_gate_rejects_the_deploy_and_never_dispatches_the_gated_stage() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    let run = PipelineRun {
        stages: vec![
            PipelineStage::gate(CiStage::new(
                "deploy-approval",
                "gate://acme/ci/run-7#deploy",
                MinorUnits(0),
                Some(86_400),
            )),
            PipelineStage::job(CiStage::new(
                "deploy",
                "pipeline://acme/ci/run-7#deploy",
                MinorUnits(0),
                Some(3600),
            )),
        ],
        contexts: vec!["deploy".into()],
        facts: facts(),
    };

    deliver_approval(&signals, "deploy-approval", false); // DENIED

    let mut ctx = begin(&outbox, signals, timers, 1000);
    let verdict = run_ci_pipeline_body(&mut ctx, &run, &runner).expect("denied gate");
    assert_eq!(
        verdict,
        RunVerdict::Rejected {
            stage: "deploy-approval".into()
        },
        "the gate was rejected"
    );
    assert_eq!(
        runner.count(),
        0,
        "the deploy stage was NEVER dispatched (0 wasted spend)"
    );

    let types = emitted_types(ctx, &outbox);
    assert!(
        types.contains(&CI_DEPLOYMENT_REJECTED.to_string()),
        "ci.deployment.rejected emitted"
    );
    assert!(
        !types.contains(&CI_RUN_SUCCEEDED.to_string()),
        "a rejected deploy never succeeds"
    );
}

/// **No balance → the first stage is NEVER dispatched (reserve fronts the dispatch, 11.7).** A wallet
/// that cannot afford the build stage refuses the reserve; the runner is NEVER called; the body fails
/// loud (the reserve/settle bookend the body inherits from the engine).
#[test]
fn no_balance_means_the_first_stage_is_never_dispatched() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    // a metered run: the build stage costs 10 minor-units; the wallet holds 5 → refused at reserve.
    let run = PipelineRun {
        stages: vec![PipelineStage::job(CiStage::new(
            "build",
            "pipeline://acme/ci/run-7#build",
            MinorUnits(10),
            Some(3600),
        ))],
        contexts: vec!["build".into()],
        facts: facts(),
    };

    let mut ctx = begin_metered(&outbox, signals, timers, MinorUnits(5), 1000);
    let err = run_ci_pipeline_body(&mut ctx, &run, &runner).expect_err("exhausted wallet");
    assert!(
        matches!(err, myelin_flow::WfError::CoCommit(ref m) if m.contains("never dispatched")),
        "the refused reserve is loud, got {err:?}"
    );
    assert_eq!(
        runner.count(),
        0,
        "the runner was NEVER called (no balance → no dispatch)"
    );
}
