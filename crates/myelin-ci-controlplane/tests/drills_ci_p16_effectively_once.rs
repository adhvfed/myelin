//! # The `SCHEDULE_AND_RUN_JOB` effectively-once drill — CI-D1 (CI-P16 → P-359, M4)
//!
//! **The CI-P16 GATE (the drill catalogue row CI-D1, EI-01 §4 — chain mutations end-to-end):** kill
//! the runner mid-job; kill the control plane mid-run → the run RESUMES (the durable-workflow replay +
//! the `SCHEDULE_AND_RUN_JOB` idempotent re-dispatch on the engine-minted `idem_token`) → **0 lost
//! runs, 0 double-deploys, 0 duplicate artifact publishes** (effectively-once) — double-effect count =
//! 0.
//!
//! The drill runs the REAL `ci.pipeline` body ([`run_ci_pipeline_body`]) under the REAL durable
//! dispatcher ([`myelin_flow::FlowDispatcher`]), with the REAL CI dispatch handshake
//! ([`myelin_ci_controlplane::SchedulerJobRunner`]) enqueuing each stage's job into a REAL CI
//! scheduler ([`myelin_ci_controlplane::SchedulerState`] `job_queue`). The two kills are injected:
//!
//! - **Kill the runner mid-job** — a runner claims+leases the dispatched job, then DIES; the
//!   dead-runner reaper sweeps the expired lease back to `queued` (CI-P12). The dispatch's
//!   deterministic `idem_token` makes the re-claim a re-attempt of the SAME job (the `jq_idem` unique
//!   keeps it ONE `job_queue` row).
//! - **Kill the control plane mid-run** — a FRESH dispatcher worker re-drives the run off the journal
//!   (a new process). The dispatch activity SHORT-CIRCUITS (0 re-dispatch); the re-driven body
//!   redundantly re-dispatches the un-journaled stage idempotently (still ONE row).
//!
//! The double-effect probe is the **`job_queue` row count per stage** (the dispatch enqueue is the
//! observable side effect that, doubled, would double-deploy / double-publish) PLUS the **runner's
//! observed dispatch-accept count** vs the **distinct jobs that actually reached the queue**. The
//! invariant: across every kill + replay, each stage is ONE `job_queue` row, completed ONCE.
//!
//! The in-sandbox EXECUTION of the dispatched job (`ToolHands::exec`) is GATED by AG-D4; this drill
//! exercises the dispatch handshake + the effectively-once invariant over the scheduler model + the
//! engine, with the runner's terminal `job.done` standing in for the sandboxed completion. The
//! reserve/settle metering into `cost_event` is CI-P17.

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
    job_idem_token, run_state, stage_verdict_marker, CiStage, DriveOutcome, DurableExecutor,
    FlowDispatcher, FlowExecutor, FlowTelemetry, JobKind, JobRunner, JobSpec, MinorUnits, RunStore,
    SignalOutcome, SignalSpec, SignalStore, TimerStore, WfCtx, WfJournal, WorkflowBody,
    JOB_DONE_SIGNAL,
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
        merge_idem_token: "merge-attempt-pr-7".into(),
    }
}

/// The reference run: two ordered runner stages (`build` → `test`), each a `SCHEDULE_AND_RUN_JOB`.
fn run_spec() -> PipelineRun {
    PipelineRun {
        stages: vec![
            PipelineStage::job(CiStage::new(
                "build",
                "pipeline://acme/ci/pr-7#build",
                MinorUnits(0),
                Some(3600),
            )),
            PipelineStage::job(CiStage::new(
                "test",
                "pipeline://acme/ci/pr-7#test",
                MinorUnits(0),
                Some(3600),
            )),
        ],
        contexts: vec!["build".into(), "test".into()],
        facts: facts(),
    }
}

/// The CI run's scheduling terms (a PURE function of the snapshot).
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

/// A `JobRunner` that wraps the REAL [`SchedulerJobRunner`] and ALSO counts every dispatch ACCEPT (so
/// the drill can assert the engine's replay short-circuit = 0 re-dispatch, while the underlying
/// `jq_idem` keeps it ONE row even when a re-dispatch DOES reach the runner).
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

/// The registered `ci.pipeline` body closure.
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

fn partition_for(run_id: &str) -> i16 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    run_id.hash(&mut h);
    (h.finish() % myelin_flow::PARTITION_COUNT as u64) as i16
}

/// A FRESH dispatcher worker (a NEW control-plane process — the "kill the control plane" injection: a
/// new worker re-drives the run off the shared journal/run-store).
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

/// The deterministic dispatch `idem_token` for the Nth runner stage (= the `job_queue` job_id).
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

/// **CI-D1 — kill the runner mid-job + kill the control plane mid-run → effectively-once (0 lost runs,
/// 0 double-deploys, 0 duplicate publishes; double-effect count = 0).** The drill chains the two kills
/// end-to-end across a two-stage run and asserts each stage is ONE `job_queue` row, completed ONCE.
#[test]
fn ci_d1_kill_runner_and_control_plane_mid_run_is_effectively_once() {
    let (ex, run, sub) = start("ci.pipeline:pr-7:run-1");
    let part = partition_for(&run.0);
    let scheduler = Arc::new(Mutex::new(SchedulerState::new()));
    let runner = CountingSchedulerRunner::new(scheduler.clone(), &run.0);

    // ── DRIVE 1: dispatch `build` (enqueue ONE job_queue row) + park on its job.done.
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

    // ── KILL THE RUNNER MID-JOB: a runner claims+leases build, then DIES; the reaper re-queues it.
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
        // the runner DIES — advance past the lease + reap (the dead-runner reaper, CI-P12).
        s.advance(50);
        let reaped = s.reap();
        assert_eq!(reaped.len(), 1, "the dead runner's build lease was reaped");
        assert_eq!(
            s.state_of("acme", &build_token),
            Some(JobState::Queued),
            "build re-queued IN PLACE (one row) — claimable by a fresh runner"
        );
    }

    // ── KILL THE CONTROL PLANE MID-RUN: a FRESH worker re-drives off the journal. The dispatch
    // SHORT-CIRCUITS (0 re-dispatch into the engine); the body still parks on build (only a journaled
    // job.done advances it). The reaper re-queue + the engine replay leave build as ONE row.
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
            "STILL one build row after runner-kill + control-plane-kill (0 double-deploy — CI-D1)"
        );
    }

    // ── The fresh runner claims the re-queued build + completes it. Deliver build's job.done TWICE
    // (at-least-once under the bus) — the engine wakes ONCE; complete_job terminates the row ONCE.
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
    let s2 = deliver_stage_done(&ex, &run, 0, "build", true); // DOUBLE delivery (at-least-once).
    assert_eq!(
        s1,
        SignalOutcome::Buffered,
        "build's job.done buffered (the first delivery)"
    );
    assert_eq!(
        s2,
        SignalOutcome::Duplicate,
        "the second job.done is a DUPLICATE no-op (the wf_signal PK dedup — one wake)"
    );

    // ── Advance to `test` (build's journaled job.done → the body dispatches test).
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
        // build (terminal) + test (queued) = 2 rows, each ONE.
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

    // ── test completes → the run reaches SUCCESS and emits the X-1 producer facts ONCE.
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
            "the run COMPLETED — both stages, effectively-once across every kill"
        ),
        other => panic!("the run should COMPLETE, got {other:?}"),
    }

    // ── THE EFFECTIVELY-ONCE INVARIANT (the double-effect probe = 0):
    {
        let s = scheduler.lock().unwrap();
        assert_eq!(
            s.jobs().len(),
            2,
            "exactly TWO job_queue rows across the WHOLE run (build + test) — 0 double-deploy"
        );
        // distinct idem_tokens (= job_ids) = 2: no stage was ever enqueued twice.
        let distinct: std::collections::BTreeSet<_> =
            s.jobs().iter().map(|j| j.idem_token.clone()).collect();
        assert_eq!(
            distinct.len(),
            2,
            "two DISTINCT idem_tokens — no duplicate job (0 duplicate publish)"
        );
        assert!(
            s.jobs().iter().all(|j| j.state == JobState::Terminal),
            "both stages terminated ONCE"
        );
    }
    // run state = completed → 0 lost runs.
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::COMPLETED,
        "the run reached a terminal COMPLETED state (0 lost runs)"
    );

    // The X-1 producer facts land EXACTLY ONCE (the body emitted them on the single terminal drive).
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
        "the ci.result rollup emitted EXACTLY once (wakes Git's merge queue ONCE — 0 double-merge)"
    );

    println!(
        "[2026-06-23] PASS  drill=CI-D1  handshake=SCHEDULE_AND_RUN_JOB(CI-controlplane)  \
         kill=runner-mid-job+control-plane-mid-run  resume=yes  lost-runs=0  double-deploys=0  \
         duplicate-publishes=0  double-effect-count=0  job_queue-rows=2(distinct)  \
         job.done-dedup=wf_signal-PK  enqueue-dedup=jq_idem  re-dispatch-on-replay=0  \
         metering=CI-P17(floor)  sandbox-exec=AG-D4(gated)"
    );
}
