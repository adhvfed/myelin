use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    merge_attempt_id, partition_for_run_id, run_state, CheckFact, CiDispatch, CiDispatcher,
    DriveOutcome, DurableExecutor, FlowDispatcher, FlowExecutor, FlowTelemetry, MergeOutcome,
    MergePerformer, MergeRequest, RealCiResultProducer, RunStore, SignalStore, TimerStore, WfCtx,
    WfJournal, WorkflowBody,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::reserve_settle::MicroUsd;
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "deadbeefcafe";

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
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

#[derive(Default)]
struct CountingCi {
    calls: AtomicUsize,
}
impl CiDispatcher for CountingCi {
    fn dispatch(&self, _ci: &CiDispatch) -> Result<(), myelin_flow::ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct CountingMerger {
    merges: AtomicUsize,
}
impl MergePerformer for CountingMerger {
    fn merge(&self, request: &MergeRequest) -> Result<String, myelin_flow::ActivityError> {
        self.merges.fetch_add(1, Ordering::SeqCst);
        Ok(format!("merged-{}", request.speculative_commit_oid))
    }
}

fn request() -> MergeRequest {
    MergeRequest {
        pr_ref: format!("{REPO}#pr-7"),
        target_ref: "refs/heads/main".into(),
        speculative_commit_oid: COMMIT.into(),
        required_contexts: vec!["build".into(), "test".into()],
    }
}

fn merge_queue_body(ci: Arc<CountingCi>, merger: Arc<CountingMerger>) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let out = ctx
            .run_merge_attempt(
                &request(),
                ci.as_ref(),
                merger.as_ref(),
                Some(3600),
                MicroUsd(0),
                vec![],
            )
            .map_err(|e| format!("{e:?}"))?;
        match out {
            MergeOutcome::Merged {
                merged_commit_oid, ..
            } => Ok(vec![ArtifactRef(format!(
                "outcome:merged:{merged_commit_oid}"
            ))]),
            MergeOutcome::Dequeued { reason } => {
                Ok(vec![ArtifactRef(format!("outcome:dequeued:{reason}"))])
            }
            MergeOutcome::TimedOut => Ok(vec![ArtifactRef("outcome:timedout".into())]),
            MergeOutcome::Parked => Ok(vec![]),
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
    ci: Arc<CountingCi>,
    merger: Arc<CountingMerger>,
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
    disp.register("merge.queue", merge_queue_body(ci, merger));
    disp
}

fn start(idem: &str) -> (FlowExecutor, myelin_flow::RunId, Substrate) {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("merge.queue");
    let run = ex
        .start(myelin_flow::StartSpec {
            wf_type: "merge.queue".into(),
            input: vec![],
            budget: None,
            idem_key: idem.into(),
        })
        .expect("start the merge-queue workflow");
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

fn required() -> Vec<String> {
    vec!["build".into(), "test".into()]
}

#[test]
fn x1_seam_e2e_green_rollup_from_real_producer_one_merge_across_restart() {
    let ci = Arc::new(CountingCi::default());
    let merger = Arc::new(CountingMerger::default());
    let (_ex, run, sub) = start("queue:main:pr-7");
    let part = partition_for_run_id(&run.0);

    let w1 = fresh_worker(&sub, "worker-1", part, ci.clone(), merger.clone());
    assert_eq!(
        w1.tick(1_000, "2026-06-21T00:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "the run PARKED on the ci.result wait"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING,
        "state=waiting - holds no runtime across the multi-hour CI run"
    );
    assert_eq!(ci.calls.load(Ordering::SeqCst), 1, "CI dispatched once");
    drop(w1);

    let facts = vec![
        CheckFact {
            context: "build".into(),
            run_attempt: 2,
            success: true,
            seq: 3,
        },
        CheckFact {
            context: "test".into(),
            run_attempt: 1,
            success: true,
            seq: 2,
        },
        CheckFact {
            context: "build".into(),
            run_attempt: 1,
            success: false,
            seq: 1,
        },
        CheckFact {
            context: "test".into(),
            run_attempt: 1,
            success: true,
            seq: 2,
        },
    ];

    let attempt = merge_attempt_id(&run.0, "merge.queue:0");
    let producer = RealCiResultProducer::new(&sub.signals, tenant(), region(), &run.0, REPO);

    let first = producer.deliver(COMMIT, &facts, &required(), &attempt);
    let second = producer.deliver(COMMIT, &facts, &required(), &attempt);
    assert!(first, "the first ci.result delivery is new");
    assert!(
        !second,
        "the at-least-once double-delivery deduped on merge_attempt_id (ON CONFLICT DO NOTHING)"
    );
    assert_eq!(
        sub.signals.count_for_run(&tenant(), &run.0),
        1,
        "ONE buffered ci.result (wakes once)"
    );
    sub.runs.wake(&tenant(), &run.0);

    let w2 = fresh_worker(&sub, "worker-2", part, ci.clone(), merger.clone());
    match w2.tick(2_000, "2026-06-21T02:00:00Z", 7).expect("resume") {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("outcome:merged:merged-deadbeefcafe".into())],
            "the resumed run MERGED on the REAL green rollup (build supersession honoured)"
        ),
        other => panic!("expected the run to merge + complete, got {other:?}"),
    }

    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "the ci.result was consumed EXACTLY ONCE (1 wake)"
    );
    assert_eq!(
        merger.merges.load(Ordering::SeqCst),
        1,
        "merge-count == 1 (0 double-merge) - the GIT-D10/CI-D8 threshold"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        1,
        "EXACTLY one git.pr.merged emit"
    );
    assert_eq!(
        ci.calls.load(Ordering::SeqCst),
        1,
        "0 re-dispatch across the restart"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::COMPLETED
    );

    println!(
        "[2026-06-23] PASS  drill=GIT-D10/CI-D8  scenario=x1-seam-end-to-end  \
         producer=REAL(RealCiResultProducer)  checks=out-of-order+re-run+dup  \
         supersession=run_attempt-monotonic  rollup=success  double-delivery->buffered=1  \
         wake=1  merge-count=1  double-merge=0  git.pr.merged=1  re-dispatch=0"
    );
}

#[test]
fn x1_seam_e2e_superseding_failure_dequeues_zero_spurious_unblock() {
    let ci = Arc::new(CountingCi::default());
    let merger = Arc::new(CountingMerger::default());
    let (_ex, run, sub) = start("queue:main:pr-8");
    let part = partition_for_run_id(&run.0);

    let w1 = fresh_worker(&sub, "worker-1", part, ci.clone(), merger.clone());
    assert_eq!(
        w1.tick(1_000, "2026-06-21T00:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "parked"
    );
    drop(w1);

    let facts = vec![
        CheckFact {
            context: "build".into(),
            run_attempt: 1,
            success: true,
            seq: 1,
        },
        CheckFact {
            context: "test".into(),
            run_attempt: 1,
            success: true,
            seq: 2,
        },
        CheckFact {
            context: "test".into(),
            run_attempt: 2,
            success: false,
            seq: 3,
        },
    ];

    let attempt = merge_attempt_id(&run.0, "merge.queue:0");
    let producer = RealCiResultProducer::new(&sub.signals, tenant(), region(), &run.0, REPO);
    let rollup = producer.rollup(COMMIT, &facts, &required(), &attempt);
    assert_eq!(
        rollup.overall,
        myelin_events::check_seam::CiOverall::Failure,
        "the REAL rollup derives FAILURE - the superseding failure is the current verdict"
    );
    producer.deliver(COMMIT, &facts, &required(), &attempt);
    sub.runs.wake(&tenant(), &run.0);

    let w2 = fresh_worker(&sub, "worker-2", part, ci.clone(), merger.clone());
    match w2.tick(2_000, "2026-06-21T01:00:00Z", 7).expect("resume") {
        DriveOutcome::Completed(refs) => {
            let r = &refs[0].0;
            assert!(
                r.starts_with("outcome:dequeued:"),
                "the PR was dequeued: {r}"
            );
            assert!(r.contains("CI failed"), "humanised reason: {r}");
        }
        other => panic!("expected dequeue, got {other:?}"),
    }
    assert_eq!(
        merger.merges.load(Ordering::SeqCst),
        0,
        "0 merge on a superseding failure (0 spurious unblock)"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        0,
        "no git.pr.merged on the superseding failure"
    );

    println!(
        "[2026-06-23] PASS  drill=GIT-D10/CI-D8  scenario=superseding-failure  \
         producer=REAL  supersession=run_attempt-monotonic  rollup=failure  dequeue=1(humanised)  \
         merge=0  spurious-unblock=0"
    );
}

#[test]
fn x1_seam_e2e_fork_self_green_is_neutral_for_gating() {
    let producer_signals = SignalStore::new();
    let producer = RealCiResultProducer::new(&producer_signals, tenant(), region(), "R-fork", REPO);

    let facts = vec![CheckFact {
        context: "build".into(),
        run_attempt: 1,
        success: true,
        seq: 1,
    }];
    let attempt = merge_attempt_id("R-fork", "merge.queue:0");
    let rollup = producer.rollup(COMMIT, &facts, &required(), &attempt);
    assert_eq!(
        rollup.overall,
        myelin_events::check_seam::CiOverall::Failure,
        "a fork self-green is NEUTRAL - the missing required `test` keeps the gate CLOSED \
         (0 spurious unblock)"
    );
    assert_eq!(
        rollup.contexts,
        vec!["build".to_string(), "test".to_string()],
        "the rollup is over Git's required gate set, not the fork's self-reported contexts"
    );

    println!(
        "[2026-06-23] PASS  drill=GIT-D10/CI-D8  scenario=fork-self-green-neutral  \
         producer=REAL  required-missing=test  rollup=failure  spurious-unblock=0"
    );
}
