//! # The `ci.pipeline` body drill — CI-D9 (determinism guard) (CI-P15 → P-358, M4)
//!
//! The CI-P15 GATE drill: the `ci.pipeline` DURABLE WORKFLOW BODY
//! ([`myelin_ci_controlplane::ci_pipeline::run_ci_pipeline_body`]) run UNDER THE REAL DURABLE
//! DISPATCHER ([`myelin_flow::FlowDispatcher`] over a `RunStore` + journal + signal buffer + outbox +
//! timer wheel), proving CI-D9:
//!
//! - **flow-determinism lint GREEN on the real body** — the actual `ci_pipeline.rs` body file passes
//!   the `flow-determinism` lint (no clock/RNG/IO outside `WfCtx`); a RED fixture (a body with a raw
//!   `SystemTime::now()`) FAILS the lint (the red+green fixture pair, the prompt's required test).
//! - **REPLAY-BIT-IDENTICAL + ONLY-journaled-`job.done`** — a parked run re-driven with NO buffered
//!   `job.done` STILL parks (the body never advances on an un-journaled stage), and the journal is
//!   BYTE-IDENTICAL across the two drives (0 re-dispatch, 0 re-emit).
//! - **The X-1 PRODUCER side lands on the terminal drive** — once every stage's `job.done` is
//!   journaled, the run reaches SUCCESS and the body emits the terminal per-context
//!   `ci.check.updated` + `ci.run.succeeded` + the `ci.result` rollup (the producer facts the merge
//!   queue + Git's gate consume).
//!
//! The `job.done` PRODUCER is a recording runner fixture standing in for CI's real runner pool — the
//! `SCHEDULE_AND_RUN_JOB` dispatch into the live scheduler/runner is CI-P16 (P-359), GATED by AG-D4.
//! The bit-identical replay + effectively-once crash-recovery of the ENGINE the body composes is
//! additionally drilled in `myelin-flow/tests/drills_ci_pipeline.rs` (CI-D9/CI-D1 on the substrate).

use myelin_ci_controlplane::ci_pipeline::{
    run_ci_pipeline_body, CheckFacts, PipelineRun, PipelineStage, RunVerdict,
};
use myelin_ci_controlplane::CI_PIPELINE_WF_TYPE;
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

/// The reference run: two ordered runner stages (`build` → `test`), un-metered, each a
/// `SCHEDULE_AND_RUN_JOB` long-park; two reported contexts (`build`, `test`).
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

/// The registered `ci.pipeline` body closure: run the CI body + encode the terminal verdict into the
/// body's result refs so the drill reads it off the [`DriveOutcome`].
fn ci_pipeline_body(runner: Arc<CountingCiRunner>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let verdict = run_ci_pipeline_body(ctx, &run_spec(), runner.as_ref())
            .map_err(|e| format!("{e:?}"))?;
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

/// The deterministic dispatch `idem_token` for the Nth runner stage (each = 2 command positions).
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

/// **CI-D9 — the `ci.pipeline` BODY is replay-bit-identical + only a journaled `job.done` feeds it +
/// the X-1 producer facts land on the terminal drive.** The body runs under the real dispatcher: a
/// parked re-drive with no buffered `job.done` STILL parks (byte-identical journal, 0 re-dispatch);
/// once every stage's `job.done` is journaled, the run reaches SUCCESS and emits the terminal X-1
/// producer facts.
#[test]
fn ci_d9_ci_pipeline_body_replay_bit_identical_and_emits_x1_producer_facts() {
    let runner = Arc::new(CountingCiRunner::default());
    let (ex, run, sub) = start("ci.pipeline:pr-7:run-1");
    let part = partition_for(&run.0);

    // DRIVE 1: dispatch `build` + park on its job.done.
    let w1 = fresh_worker(&sub, "worker-1", part, runner.clone());
    assert_eq!(
        w1.tick(1_000, "2026-06-23T00:00:00Z", 7).expect("drive 1"),
        DriveOutcome::Waiting,
        "the run PARKED on build's job.done"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING,
        "state=waiting — the pipeline holds no runtime across the build"
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
        "the dispatch + wait journaled"
    );

    // ONLY-JOURNALED-JOB.DONE: a re-drive with NO buffered job.done STILL parks at build.
    sub.runs.wake(&tenant(), &run.0);
    let w_replay = fresh_worker(&sub, "worker-replay", part, runner.clone());
    assert_eq!(
        w_replay
            .tick(1_500, "2026-06-23T00:30:00Z", 7)
            .expect("replay drive"),
        DriveOutcome::Waiting,
        "with no journaled job.done the re-drive STILL parks (only a journaled job.done advances)"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        1,
        "0 RE-DISPATCH on the replay (the dispatch short-circuited)"
    );

    // BIT-IDENTICAL: the journal after the replay is BYTE-IDENTICAL to drive 1's.
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
    // No terminal X-1 facts while parked (a parked run is not terminal).
    assert!(
        sub.outbox
            .committed_rows()
            .iter()
            .all(|r| r.envelope.type_.0 != CI_RUN_SUCCEEDED),
        "no terminal ci.run.succeeded while the run is parked"
    );

    // Feed build's job.done → advance to test (parks on test's job.done).
    deliver_stage_done(&ex, &run, 0, "build", true);
    sub.runs.wake(&tenant(), &run.0);
    let w2 = fresh_worker(&sub, "worker-2", part, runner.clone());
    assert_eq!(
        w2.tick(2_000, "2026-06-23T01:00:00Z", 7)
            .expect("advance to test"),
        DriveOutcome::Waiting,
        "build done → parked on test's job.done"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "build (replayed, 0 re-dispatch) + test (dispatched once)"
    );

    // Feed test's job.done → the run reaches SUCCESS and emits the X-1 producer facts.
    deliver_stage_done(&ex, &run, 1, "test", true);
    sub.runs.wake(&tenant(), &run.0);
    let w3 = fresh_worker(&sub, "worker-3", part, runner.clone());
    let terminal = w3
        .tick(3_000, "2026-06-23T02:00:00Z", 7)
        .expect("the terminal drive");
    match &terminal {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            &vec![ArtifactRef("verdict:succeeded:2".into())],
            "the terminal verdict is SUCCEEDED with both stages completed"
        ),
        other => panic!("every stage passed → the run COMPLETED, got {other:?}"),
    }
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "exactly two dispatches across the whole run (0 re-dispatch on any replay)"
    );

    // THE X-1 PRODUCER FACTS: two per-context terminal ci.check.updated + ci.run.succeeded + ci.result.
    let types: Vec<String> = sub
        .outbox
        .committed_rows()
        .into_iter()
        .map(|r| r.envelope.type_.0)
        .collect();
    let checks = types.iter().filter(|t| *t == CI_CHECK_UPDATED).count();
    assert_eq!(
        checks, 2,
        "one terminal ci.check.updated PER context (build + test), got {types:?}"
    );
    assert!(
        types.contains(&CI_RUN_SUCCEEDED.to_string()),
        "ci.run.succeeded emitted on the terminal drive"
    );
    assert!(
        types.contains(&CI_RESULT.to_string()),
        "the ci.result rollup emitted (wakes Git's merge queue, X-1)"
    );

    println!(
        "[2026-06-23] PASS  drill=CI-D9  body=ci.pipeline(CI-controlplane)  \
         replay-bit-identical=yes  only-journaled-job.done=yes  re-dispatch=0  \
         x1-producer=ci.check.updated(per-context)+ci.run.succeeded+ci.result  \
         flow-determinism-lint=green(real-body)  producer=RUNNER-FIXTURE(AG-D4-gated)"
    );
}

/// **CI-D9 (the flow-determinism red+green fixture) — the REAL `ci.pipeline` body is lint-green; a
/// body with a raw clock is lint-RED.** The prompt's required red+green fixture: the actual
/// `ci_pipeline.rs` body file passes `flow-determinism` (no clock/RNG/IO outside `WfCtx`); a fixture
/// body with a raw `SystemTime::now()` inside a `@workflow-body` FAILS the lint.
#[test]
fn ci_d9_flow_determinism_lint_green_on_the_real_body_red_on_a_raw_clock() {
    let lint = myelin_lints::flow_determinism();

    // GREEN: the ACTUAL ci.pipeline body file (the deliverable) reads no clock/RNG/IO outside WfCtx.
    let real_body = include_str!("../src/ci_pipeline.rs");
    let green = lint.run(real_body);
    assert!(
        green.is_empty(),
        "the real ci.pipeline body must be flow-determinism clean, got {green:?}"
    );

    // RED: a workflow body that reads a raw clock bypasses WfCtx → the lint FIRES (the determinism
    // guard the body must never trip). The `@workflow-body` marker scopes the scan.
    let red = "// @workflow-body\n\
        fn ci_pipeline(ctx: &mut WfCtx) {\n\
        \x20\x20\x20\x20let now = std::time::SystemTime::now(); // BYPASSES WfCtx — replay diverges\n\
        \x20\x20\x20\x20let _ = (ctx, now);\n\
        }\n";
    assert!(
        !lint.run(red).is_empty(),
        "a workflow body with a raw SystemTime::now() must FAIL the flow-determinism lint (the red fixture)"
    );

    println!(
        "[2026-06-23] PASS  drill=CI-D9  fixture=flow-determinism(red+green)  \
         green=real-ci.pipeline-body  red=raw-SystemTime::now()"
    );
}
