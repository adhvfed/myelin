use myelin_ci_controlplane::ci_pipeline::{
    run_ci_pipeline_body, CheckFacts, PipelineRun, PipelineStage,
};
use myelin_ci_controlplane::scheduler::{ClaimRequest, JobState, Lane, SchedulerState, TrustTier};
use myelin_ci_controlplane::{
    complete_job, JobScheduleTerms, RunVerdict, SchedulerJobRunner, CI_PIPELINE_WF_TYPE,
};
use myelin_ci_sandbox::events::{CI_CHECK_UPDATED, CI_RESULT, CI_RUN_SUCCEEDED};
use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    job_idem_token, partition_for_run_id, run_state, stage_verdict_marker, CiStage, DriveOutcome,
    DurableExecutor, FlowDispatcher, FlowExecutor, FlowTelemetry, JobKind, JobRunner, JobSpec,
    MicroUsd, RunStore, SignalOutcome, SignalSpec, SignalStore, TimerStore, WfCtx, WfJournal,
    WorkflowBody, JOB_DONE_SIGNAL,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
            PrincipalId("ci".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
        caused_by: None,
    }
}

fn facts() -> CheckFacts {
    CheckFacts {
        repo: "myelin://acme/git/repo/r1".into(),
        commit_oid: "deadbeef".into(),
        run_ref: "myelin://acme/ci/run/pr-7".into(),
        run_attempt: 1,
        trust_tier: "trusted".into(),
        started_at: "2026-06-23T00:00:00Z".into(),
        merge_idem_token: "merge-attempt-pr-7".into(),
    }
}

fn run_spec() -> PipelineRun {
    PipelineRun {
        stages: vec![
            PipelineStage::job(CiStage::new(
                "build",
                "pipeline://acme/ci/pr-7#build",
                MicroUsd(0),
                Some(3600),
            )),
            PipelineStage::job(CiStage::new(
                "test",
                "pipeline://acme/ci/pr-7#test",
                MicroUsd(0),
                Some(3600),
            )),
        ],
        contexts: vec!["build".into(), "test".into()],
        facts: facts(),
    }
}

fn terms(run_id: &str) -> JobScheduleTerms {
    JobScheduleTerms::new(
        "acme",
        "fr-par",
        run_id,
        Lane::Interactive,
        TrustTier::Trusted,
        "acme",
    )
}

#[derive(Clone)]
struct CountingSchedulerRunner {
    inner: SchedulerJobRunner,
    accepts: Arc<AtomicUsize>,
}
impl CountingSchedulerRunner {
    fn new(scheduler: Arc<Mutex<SchedulerState>>, run_id: &str) -> Self {
        CountingSchedulerRunner {
            inner: SchedulerJobRunner::new(scheduler, terms(run_id)),
            accepts: Arc::new(AtomicUsize::new(0)),
        }
    }
}
impl JobRunner for CountingSchedulerRunner {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), myelin_flow::ActivityError> {
        assert_eq!(spec.kind, JobKind::Ci, "a CI pipeline dispatches kind=ci");
        self.accepts.fetch_add(1, Ordering::SeqCst);
        self.inner.dispatch(spec)
    }
}

fn ci_pipeline_body(runner: CountingSchedulerRunner) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let verdict =
            run_ci_pipeline_body(ctx, &run_spec(), &runner).map_err(|e| format!("{e:?}"))?;
        match verdict {
            RunVerdict::Succeeded { stages_completed } => Ok(vec![ArtifactRef(format!(
                "verdict:succeeded:{stages_completed}"
            ))]),
            RunVerdict::Failed { stage } => {
                Ok(vec![ArtifactRef(format!("verdict:failed:{stage}"))])
            }
            RunVerdict::Rejected { stage } => {
                Ok(vec![ArtifactRef(format!("verdict:rejected:{stage}"))])
            }
            RunVerdict::Parked => Ok(vec![]),
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
    runner: CountingSchedulerRunner,
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
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: JOB_DONE_SIGNAL.into(),
        idem_key: stage_token(&run.0, stage_idx),
        payload: vec![stage_verdict_marker(stage, pass)],
        payload_key_ref: None,
    })
    .expect("deliver job.done")
}

#[test]
fn ci_d1_kill_runner_and_control_plane_mid_run_is_effectively_once() {
    let (ex, run, sub) = start("ci.pipeline:pr-7:run-1");
    let part = partition_for_run_id(&run.0);
    let scheduler = Arc::new(Mutex::new(SchedulerState::new()));
    let runner = CountingSchedulerRunner::new(scheduler.clone(), &run.0);

    let w1 = fresh_worker(&sub, "worker-1", part, runner.clone());
    assert_eq!(
        w1.tick(1_000, "2026-06-23T00:00:00Z", 7).expect("drive 1"),
        DriveOutcome::Waiting,
        "the run PARKED on build's job.done (holds no runtime)"
    );
    let build_token = stage_token(&run.0, 0);
    {
        let s = scheduler.lock().unwrap();
        assert_eq!(s.jobs().len(), 1, "build enqueued ONE job_queue row");
        assert_eq!(
            s.state_of("acme", &build_token),
            Some(JobState::Queued),
            "build is claimable"
        );
    }

    {
        let mut s = scheduler.lock().unwrap();
        let claim = ClaimRequest {
            cell_region: "fr-par".into(),
            runner_labels: vec![],
            runner_allowed_tiers: vec![TrustTier::Trusted],
            lease_owner: "runner-dead".into(),
            lease_ttl: 10,
        };
        let claimed = s.claim(&claim).expect("the runner claims build");
        assert_eq!(claimed.job_id, build_token, "build leased");
        s.advance(50);
        let reaped = s.reap();
        assert_eq!(reaped.len(), 1, "the dead runner's build lease was reaped");
        assert_eq!(
            s.state_of("acme", &build_token),
            Some(JobState::Queued),
            "build re-queued IN PLACE (one row) - claimable by a fresh runner"
        );
    }

    sub.runs.wake(&tenant(), &run.0);
    let accepts_before_replay = runner.accepts.load(Ordering::SeqCst);
    let w_replay = fresh_worker(&sub, "worker-replay", part, runner.clone());
    assert_eq!(
        w_replay
            .tick(2_000, "2026-06-23T00:30:00Z", 7)
            .expect("the control-plane-killed re-drive"),
        DriveOutcome::Waiting,
        "the re-driven run STILL parks on build (no journaled job.done yet)"
    );
    assert_eq!(
        runner.accepts.load(Ordering::SeqCst),
        accepts_before_replay,
        "0 RE-DISPATCH on the control-plane replay (the dispatch activity short-circuited)"
    );
    {
        let s = scheduler.lock().unwrap();
        assert_eq!(
            s.jobs().len(),
            1,
            "STILL one build row after runner-kill + control-plane-kill (0 double-deploy - CI-D1)"
        );
    }

    {
        let mut s = scheduler.lock().unwrap();
        let claim = ClaimRequest {
            cell_region: "fr-par".into(),
            runner_labels: vec![],
            runner_allowed_tiers: vec![TrustTier::Trusted],
            lease_owner: "runner-live".into(),
            lease_ttl: 100,
        };
        s.claim(&claim)
            .expect("a fresh runner claims the re-queued build");
    }
    let first = complete_job(&scheduler, "acme", &build_token).expect("complete build");
    let second =
        complete_job(&scheduler, "acme", &build_token).expect("re-complete (double job.done)");
    assert!(
        first && !second,
        "build terminates ONCE (a double job.done is a no-op)"
    );

    let s1 = deliver_stage_done(&ex, &run, 0, "build", true);
    let s2 = deliver_stage_done(&ex, &run, 0, "build", true);
    assert_eq!(
        s1,
        SignalOutcome::Buffered,
        "build's job.done buffered (the first delivery)"
    );
    assert_eq!(
        s2,
        SignalOutcome::Duplicate,
        "the second job.done is a DUPLICATE no-op (the wf_signal PK dedup - one wake)"
    );

    sub.runs.wake(&tenant(), &run.0);
    let w2 = fresh_worker(&sub, "worker-2", part, runner.clone());
    assert_eq!(
        w2.tick(3_000, "2026-06-23T01:00:00Z", 7)
            .expect("advance to test"),
        DriveOutcome::Waiting,
        "build done → parked on test's job.done"
    );
    let test_token = stage_token(&run.0, 1);
    {
        let s = scheduler.lock().unwrap();
        assert_eq!(
            s.jobs().len(),
            2,
            "build + test = two distinct job_queue rows"
        );
        assert_eq!(
            s.state_of("acme", &build_token),
            Some(JobState::Terminal),
            "build is terminal (the reaper never re-queues it again)"
        );
        assert_eq!(
            s.state_of("acme", &test_token),
            Some(JobState::Queued),
            "test enqueued ONE row"
        );
    }

    let _ = complete_job(&scheduler, "acme", &test_token).expect("complete test");
    deliver_stage_done(&ex, &run, 1, "test", true);
    sub.runs.wake(&tenant(), &run.0);
    let w3 = fresh_worker(&sub, "worker-3", part, runner.clone());
    let terminal = w3
        .tick(4_000, "2026-06-23T02:00:00Z", 7)
        .expect("the terminal drive");
    match &terminal {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            &vec![ArtifactRef("verdict:succeeded:2".into())],
            "the run COMPLETED - both stages, effectively-once across every kill"
        ),
        other => panic!("the run should COMPLETE, got {other:?}"),
    }

    {
        let s = scheduler.lock().unwrap();
        assert_eq!(
            s.jobs().len(),
            2,
            "exactly TWO job_queue rows across the WHOLE run (build + test) - 0 double-deploy"
        );
        let distinct: std::collections::BTreeSet<_> =
            s.jobs().iter().map(|j| j.idem_token.clone()).collect();
        assert_eq!(
            distinct.len(),
            2,
            "two DISTINCT idem_tokens - no duplicate job (0 duplicate publish)"
        );
        assert!(
            s.jobs().iter().all(|j| j.state == JobState::Terminal),
            "both stages terminated ONCE"
        );
    }
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::COMPLETED,
        "the run reached a terminal COMPLETED state (0 lost runs)"
    );

    let types: Vec<String> = sub
        .outbox
        .committed_rows()
        .into_iter()
        .map(|r| r.envelope.type_.0)
        .collect();
    assert_eq!(
        types.iter().filter(|t| *t == CI_CHECK_UPDATED).count(),
        2,
        "one terminal ci.check.updated PER context (build + test), emitted ONCE: {types:?}"
    );
    assert_eq!(
        types.iter().filter(|t| *t == CI_RUN_SUCCEEDED).count(),
        1,
        "ci.run.succeeded emitted EXACTLY once (0 duplicate publish)"
    );
    assert_eq!(
        types.iter().filter(|t| *t == CI_RESULT).count(),
        1,
        "the ci.result rollup emitted EXACTLY once (wakes Git's merge queue ONCE - 0 double-merge)"
    );

    println!(
        "[2026-06-23] PASS  drill=CI-D1  handshake=SCHEDULE_AND_RUN_JOB(CI-controlplane)  \
         kill=runner-mid-job+control-plane-mid-run  resume=yes  lost-runs=0  double-deploys=0  \
         duplicate-publishes=0  double-effect-count=0  job_queue-rows=2(distinct)  \
         job.done-dedup=wf_signal-PK  enqueue-dedup=jq_idem  re-dispatch-on-replay=0  \
         metering=CI-P17(floor)  sandbox-exec=AG-D4(gated)"
    );
}
