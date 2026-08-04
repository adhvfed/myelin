use super::*;
use myelin_ci_sandbox::events::{
    CI_CHECK_UPDATED, CI_DEPLOYMENT_REJECTED, CI_RESULT, CI_RUN_FAILED, CI_RUN_SUCCEEDED,
};
use myelin_events::{Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore};
use myelin_flow::engine::SignalRow;
use myelin_flow::MicroUsd;
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

fn job_run() -> PipelineRun {
    PipelineRun {
        stages: vec![
            PipelineStage::job(CiStage::new(
                "build",
                "pipeline://acme/ci/run-7#build",
                MicroUsd(0),
                Some(3600),
            )),
            PipelineStage::job(CiStage::new(
                "test",
                "pipeline://acme/ci/run-7#test",
                MicroUsd(0),
                Some(3600),
            )),
        ],
        contexts: vec!["build".into(), "test".into()],
        facts: facts(),
    }
}

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

fn begin_metered(
    outbox: &OutboxStore,
    signals: SignalStore,
    timers: TimerStore,
    balance: MicroUsd,
    now_secs: i64,
) -> WfCtx {
    begin(outbox, signals, timers, now_secs).with_budget(BudgetGate::new(Wallet::new(balance)))
}

fn stage_token(stage_idx: usize) -> String {
    job_idem_token("run-7", &format!("{CI_PIPELINE_WF_TYPE}:{}", stage_idx * 2))
}

fn stage_token_after_one_gate(stage_idx: usize) -> String {
    job_idem_token(
        "run-7",
        &format!("{CI_PIPELINE_WF_TYPE}:{}", 1 + stage_idx * 2),
    )
}

fn deliver_done(signals: &SignalStore, token: &str, stage: &str, pass: bool) {
    signals.deliver(SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: "run-7".into(),
        signal_name: myelin_flow::JOB_DONE_SIGNAL.into(),
        idem_key: token.into(),
        payload: vec![stage_verdict_marker(stage, pass)],
        payload_key_ref: None,
        received_unix_ms: 0,
        consumed_seq: None,
    });
}

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
        received_unix_ms: 0,
        consumed_seq: None,
    });
}

fn emitted_types(ctx: WfCtx, outbox: &OutboxStore) -> Vec<String> {
    ctx.commit().expect("co-commit the body's emits");
    outbox
        .committed_rows()
        .into_iter()
        .map(|r| r.envelope.type_.0)
        .collect()
}

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
    let types = emitted_types(ctx, &outbox);
    assert!(
        !types
            .iter()
            .any(|t| t == CI_RUN_SUCCEEDED || t == CI_RUN_FAILED || t == CI_RESULT),
        "a parked run emits no terminal run/result fact, got {types:?}"
    );
}

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

#[test]
fn a_failing_stage_emits_failure_checks_run_failed_and_stops_fast() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    deliver_done(&signals, &stage_token(0), "build", false);

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
        "ONLY build dispatched - test was never dispatched"
    );

    let types = emitted_types(ctx, &outbox);
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
    let expected =
        myelin_events::check_seam::check_subject("myelin://acme/git/repo/r1", "deadbeef", "build");
    assert_eq!(
        row.envelope.subject, expected,
        "the X-1 subject grammar (no drift)"
    );
}

#[test]
fn terminal_run_event_uses_the_canonical_run_ref_as_its_subject() {
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
        .find(|row| row.envelope.type_.0 == CI_RUN_SUCCEEDED)
        .expect("a ci.run.succeeded fact was emitted");
    assert_eq!(
        row.envelope.subject,
        ArtifactRef("myelin://acme/ci/run/run-7".into())
    );
}

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
                MicroUsd(0),
                Some(86_400),
            )),
            PipelineStage::job(CiStage::new(
                "deploy",
                "pipeline://acme/ci/run-7#deploy",
                MicroUsd(0),
                Some(3600),
            )),
        ],
        contexts: vec!["deploy".into()],
        facts: facts(),
    };

    deliver_approval(&signals, "deploy-approval", true);
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
                MicroUsd(0),
                Some(86_400),
            )),
            PipelineStage::job(CiStage::new(
                "deploy",
                "pipeline://acme/ci/run-7#deploy",
                MicroUsd(0),
                Some(3600),
            )),
        ],
        contexts: vec!["deploy".into()],
        facts: facts(),
    };

    deliver_approval(&signals, "deploy-approval", false);

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

#[test]
fn no_balance_means_the_first_stage_is_never_dispatched() {
    let outbox = OutboxStore::new();
    let signals = SignalStore::new();
    let timers = TimerStore::new();
    let runner = RecordingRunner::default();

    let run = PipelineRun {
        stages: vec![PipelineStage::job(CiStage::new(
            "build",
            "pipeline://acme/ci/run-7#build",
            MicroUsd(10),
            Some(3600),
        ))],
        contexts: vec!["build".into()],
        facts: facts(),
    };

    let mut ctx = begin_metered(&outbox, signals, timers, MicroUsd(5), 1000);
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
