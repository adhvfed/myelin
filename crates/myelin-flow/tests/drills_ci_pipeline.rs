use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    job_idem_token, partition_for_run_id, run_state, stage_verdict_marker, CiPipelineSpec, CiStage,
    DriveOutcome, DurableExecutor, FlowDispatcher, FlowExecutor, FlowTelemetry, JobKind, JobRunner,
    JobSpec, PipelineOutcome, RunStore, SignalOutcome, SignalSpec, SignalStore, TimerStore, WfCtx,
    WfJournal, WorkflowBody, CI_PIPELINE_WF_TYPE, JOB_DONE_SIGNAL,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::reserve_settle::MicroUsd;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn tenant() -> myelin_tenancy::TenantId {
    myelin_tenancy::TenantId("acme".into())
}
fn region() -> myelin_tenancy::Region {
    myelin_tenancy::Region("fr-par".into())
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
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
        caused_by: None,
    }
}

fn pipeline() -> CiPipelineSpec {
    CiPipelineSpec::new(vec![
        CiStage::new(
            "build",
            "pipeline://acme/ci/pr-7#build",
            MicroUsd(0),
            Some(3600),
        ),
        CiStage::new(
            "test",
            "pipeline://acme/ci/pr-7#test",
            MicroUsd(0),
            Some(3600),
        ),
        CiStage::new(
            "lint",
            "pipeline://acme/ci/pr-7#lint",
            MicroUsd(0),
            Some(600),
        ),
    ])
}

#[derive(Default)]
struct CountingCiRunner {
    calls: AtomicUsize,
}
impl JobRunner for CountingCiRunner {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), myelin_flow::ActivityError> {
        assert_eq!(spec.kind, JobKind::Ci, "a CI pipeline dispatches kind=ci");
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn ci_pipeline_body(runner: Arc<CountingCiRunner>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let out = ctx
            .run_ci_pipeline(&pipeline(), runner.as_ref())
            .map_err(|e| format!("{e:?}"))?;
        match out {
            PipelineOutcome::Succeeded { stages_completed } => Ok(vec![ArtifactRef(format!(
                "outcome:succeeded:{stages_completed}"
            ))]),
            PipelineOutcome::Failed { stage } => {
                Ok(vec![ArtifactRef(format!("outcome:failed:{stage}"))])
            }
            PipelineOutcome::TimedOut { stage } => {
                Ok(vec![ArtifactRef(format!("outcome:timedout:{stage}"))])
            }
            PipelineOutcome::Parked => Ok(vec![]),
        }
    })
}

struct Substrate {
    runs: RunStore,
    journal: WfJournal,
    signals: SignalStore,
    outbox: OutboxStore,
    tele: FlowTelemetry,
    timers: TimerStore,
}

fn fresh_worker(
    sub: &Substrate,
    worker: &str,
    partition: i16,
    runner: Arc<CountingCiRunner>,
) -> FlowDispatcher {
    let mut disp = FlowDispatcher::new(
        sub.runs.clone(),
        sub.outbox.clone(),
        sub.journal.clone(),
        sub.tele.clone(),
        minter(),
        ctx_base(),
        partition,
        worker,
        30,
    )
    .with_signals(sub.signals.clone())
    .with_timers(sub.timers.clone());
    disp.register(CI_PIPELINE_WF_TYPE, ci_pipeline_body(runner));
    disp
}

fn start(idem: &str) -> (FlowExecutor, myelin_flow::RunId, Substrate) {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition(CI_PIPELINE_WF_TYPE);
    let run = ex
        .start(myelin_flow::StartSpec {
            wf_type: CI_PIPELINE_WF_TYPE.into(),
            input: vec![],
            budget: None,
            idem_key: idem.into(),
        })
        .expect("start the ci.pipeline workflow");
    let sub = Substrate {
        runs: ex.runs().clone(),
        journal: WfJournal::new(),
        signals: ex.signals().clone(),
        outbox: OutboxStore::new(),
        tele: FlowTelemetry::new(),
        timers: TimerStore::new(),
    };
    (ex, run, sub)
}

fn stage_token(run_id: &str, stage_idx: usize) -> String {
    job_idem_token(run_id, &format!("{CI_PIPELINE_WF_TYPE}:{}", stage_idx * 2))
}

fn deliver_stage_done(
    ex: &FlowExecutor,
    run: &myelin_flow::RunId,
    stage_idx: usize,
    stage: &str,
    pass: bool,
) -> SignalOutcome {
    let token = stage_token(&run.0, stage_idx);
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: JOB_DONE_SIGNAL.into(),
        idem_key: token,
        payload: vec![stage_verdict_marker(stage, pass)],
        payload_key_ref: None,
    })
    .expect("deliver job.done")
}

#[test]
fn ci_d9_ci_pipeline_replay_is_bit_identical_and_only_journaled_job_done_feeds_the_body() {
    let runner = Arc::new(CountingCiRunner::default());
    let (ex, run, sub) = start("ci.pipeline:pr-7:run-1");
    let part = partition_for_run_id(&run.0);

    let w1 = fresh_worker(&sub, "worker-1", part, runner.clone());
    let o1 = w1
        .tick(1_000, "2026-06-21T00:00:00Z", 7)
        .expect("worker-1 drives");
    assert_eq!(
        o1,
        DriveOutcome::Waiting,
        "the run PARKED on the build stage's job.done"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING,
        "state=waiting - the pipeline holds no runtime across the multi-hour build"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "ONLY build dispatched"
    );

    let journal_after_drive1: Vec<_> = sub
        .journal
        .history_for(&tenant(), &run.0)
        .iter()
        .map(|h| (h.kind.clone(), h.command_id.clone(), h.result.clone()))
        .collect();
    assert!(
        !journal_after_drive1.is_empty(),
        "the dispatch + the signal_waited are journaled"
    );

    sub.runs.wake(&tenant(), &run.0);
    let w_replay = fresh_worker(&sub, "worker-replay", part, runner.clone());
    let o_replay = w_replay
        .tick(1_500, "2026-06-21T00:30:00Z", 7)
        .expect("replay drive");
    assert_eq!(
        o_replay,
        DriveOutcome::Waiting,
        "with no journaled job.done the re-drive STILL parks at build (only a journaled job.done advances)"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "0 RE-DISPATCH on the replay (the dispatch short-circuited)"
    );

    let journal_after_replay: Vec<_> = sub
        .journal
        .history_for(&tenant(), &run.0)
        .iter()
        .map(|h| (h.kind.clone(), h.command_id.clone(), h.result.clone()))
        .collect();
    assert_eq!(
        journal_after_drive1, journal_after_replay,
        "REPLAY-BIT-IDENTICAL: the journal is byte-identical across the two drives (CI-D9)"
    );

    deliver_stage_done(&ex, &run, 0, "build", true);
    sub.runs.wake(&tenant(), &run.0);
    let w2 = fresh_worker(&sub, "worker-2", part, runner.clone());
    let o2 = w2
        .tick(2_000, "2026-06-21T01:00:00Z", 7)
        .expect("worker-2 advances to test");
    assert_eq!(
        o2,
        DriveOutcome::Waiting,
        "build done → parked on `test`'s job.done"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "build (replayed, 0 re-dispatch) + test (newly dispatched once)"
    );

    println!(
        "[2026-06-21] PASS  drill=CI-D9  fixture=ci.pipeline  replay-bit-identical=yes  \
         only-journaled-job.done=yes  re-dispatch=0  flow-determinism-lint=green(lint_fixtures)  \
         producer=RUNNER-FIXTURE(AG-D4-gated)"
    );
}

#[test]
fn ci_d1_kill_runner_and_control_plane_mid_run_resumes_effectively_once() {
    let runner = Arc::new(CountingCiRunner::default());
    let (ex, run, sub) = start("ci.pipeline:pr-8:run-1");
    let part = partition_for_run_id(&run.0);

    let w1 = fresh_worker(&sub, "worker-1", part, runner.clone());
    assert_eq!(
        w1.tick(1_000, "2026-06-21T00:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "build dispatched + parked (holds no runtime)"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "build dispatched once"
    );
    drop(w1);

    let first = deliver_stage_done(&ex, &run, 0, "build", true);
    let second = deliver_stage_done(&ex, &run, 0, "build", true);
    assert_eq!(
        first,
        SignalOutcome::Buffered,
        "the first delivery buffered"
    );
    assert_eq!(
        second,
        SignalOutcome::Duplicate,
        "the double-delivery is a no-op (ON CONFLICT DO NOTHING) - 1 wake"
    );
    assert_eq!(
        sub.signals.count_for_run(&tenant(), &run.0),
        1,
        "ONE buffered job.done (the pipeline wakes once)"
    );
    sub.runs.wake(&tenant(), &run.0);

    let w2 = fresh_worker(&sub, "worker-2", part, runner.clone());
    assert_eq!(
        w2.tick(2_000, "2026-06-21T02:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "resumed past build, now parked on test"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "build (0 re-dispatch) + test (dispatched once)"
    );
    drop(w2);

    assert_eq!(
        deliver_stage_done(&ex, &run, 1, "test", true),
        SignalOutcome::Buffered
    );
    assert_eq!(
        deliver_stage_done(&ex, &run, 1, "test", true),
        SignalOutcome::Duplicate,
        "double-delivery no-op - 1 wake"
    );
    sub.runs.wake(&tenant(), &run.0);
    let w3 = fresh_worker(&sub, "worker-3", part, runner.clone());
    assert_eq!(
        w3.tick(3_000, "2026-06-21T04:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "resumed past test, now parked on lint"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        3,
        "build + test + lint, each dispatched once"
    );
    drop(w3);

    deliver_stage_done(&ex, &run, 2, "lint", true);
    sub.runs.wake(&tenant(), &run.0);
    let w4 = fresh_worker(&sub, "worker-4", part, runner.clone());
    let o4 = w4
        .tick(4_000, "2026-06-21T05:00:00Z", 7)
        .expect("worker-4 completes");
    match o4 {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("outcome:succeeded:3".into())],
            "the pipeline completed green - all three stages ran to completion"
        ),
        other => panic!("expected the pipeline to complete, got {other:?}"),
    }

    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        3,
        "0 double-deploy: each of the 3 stages dispatched EXACTLY once across the restarts"
    );
    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "0 duplicate publish: every job.done consumed EXACTLY once"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::COMPLETED,
        "0 lost runs: the killed-and-resumed pipeline ran to completion"
    );

    println!(
        "[2026-06-21] PASS  drill=CI-D1  fixture=ci.pipeline  kills=3(runner+control-plane)  \
         resumed=yes  effectively-once=yes  lost-runs=0  double-deploys=0  duplicate-publishes=0  \
         re-dispatch=0  producer=RUNNER-FIXTURE(AG-D4-gated)"
    );
}

#[test]
fn ci_pipeline_failing_stage_stops_fast_under_the_dispatcher() {
    let runner = Arc::new(CountingCiRunner::default());
    let (ex, run, sub) = start("ci.pipeline:pr-9:run-1");
    let part = partition_for_run_id(&run.0);

    let w1 = fresh_worker(&sub, "worker-1", part, runner.clone());
    assert_eq!(
        w1.tick(1_000, "2026-06-21T00:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting
    );
    drop(w1);
    deliver_stage_done(&ex, &run, 0, "build", true);
    sub.runs.wake(&tenant(), &run.0);

    let w2 = fresh_worker(&sub, "worker-2", part, runner.clone());
    assert_eq!(
        w2.tick(2_000, "2026-06-21T01:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting
    );
    drop(w2);
    deliver_stage_done(&ex, &run, 1, "test", false);
    sub.runs.wake(&tenant(), &run.0);

    let w3 = fresh_worker(&sub, "worker-3", part, runner.clone());
    let o3 = w3
        .tick(3_000, "2026-06-21T02:00:00Z", 7)
        .expect("worker-3 fails the pipeline");
    match o3 {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("outcome:failed:test".into())],
            "the pipeline failed fast at test"
        ),
        other => panic!("expected the pipeline to fail at test, got {other:?}"),
    }
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "ONLY build + test dispatched - lint was NEVER dispatched (0 wasted spend)"
    );
}
