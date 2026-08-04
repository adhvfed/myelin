use myelin_events::check_seam::CiOverall;
use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    encode_ci_result, merge_attempt_id, partition_for_run_id, run_state, CiDispatch, CiDispatcher,
    DriveOutcome, DurableExecutor, FlowDispatcher, FlowExecutor, FlowTelemetry, MergeOutcome,
    MergePerformer, MergeRequest, RunStore, SignalOutcome, SignalSpec, SignalStore, TimerStore,
    WfCtx, WfJournal, WorkflowBody, CI_RESULT_SIGNAL,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::reserve_settle::MicroUsd;
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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
        pr_ref: "myelin://acme/git/repo/core#pr-7".into(),
        target_ref: "refs/heads/main".into(),
        speculative_commit_oid: "deadbeef".into(),
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

fn deliver_ci_result(
    ex: &FlowExecutor,
    run: &myelin_flow::RunId,
    attempt_id: &str,
    commit_oid: &str,
    overall: CiOverall,
    contexts: Vec<String>,
) -> SignalOutcome {
    let result = myelin_events::check_seam::CiResult {
        commit_oid: commit_oid.into(),
        overall,
        contexts,
        idem_token: attempt_id.into(),
    };
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: CI_RESULT_SIGNAL.into(),
        idem_key: attempt_id.into(),
        payload: encode_ci_result(&result),
        payload_key_ref: None,
    })
    .expect("deliver ci.result")
}

#[test]
fn merge_queue_success_double_delivery_one_wake_one_merge_across_restart() {
    let ci = Arc::new(CountingCi::default());
    let merger = Arc::new(CountingMerger::default());
    let (ex, run, sub) = start("queue:main:pr-7");
    let part = partition_for_run_id(&run.0);

    let w1 = fresh_worker(&sub, "worker-1", part, ci.clone(), merger.clone());
    let o1 = w1
        .tick(1_000, "2026-06-21T00:00:00Z", 7)
        .expect("worker-1 drives");
    assert_eq!(
        o1,
        DriveOutcome::Waiting,
        "the run PARKED on the ci.result wait"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING,
        "state=waiting - the merge queue holds no runtime across the multi-hour CI run"
    );
    assert_eq!(
        ci.calls.load(Ordering::SeqCst),
        1,
        "CI dispatched exactly once"
    );
    assert_eq!(
        merger.merges.load(Ordering::SeqCst),
        0,
        "no merge yet - CI still running"
    );
    assert_eq!(sub.outbox.committed_count(), 0, "no git.pr.merged yet");
    drop(w1);

    let attempt = merge_attempt_id(&run.0, "merge.queue:0");
    let first = deliver_ci_result(
        &ex,
        &run,
        &attempt,
        "deadbeef",
        CiOverall::Success,
        vec!["build".into(), "test".into()],
    );
    let second = deliver_ci_result(
        &ex,
        &run,
        &attempt,
        "deadbeef",
        CiOverall::Success,
        vec!["build".into(), "test".into()],
    );
    assert_eq!(
        first,
        SignalOutcome::Buffered,
        "the first ci.result buffered"
    );
    assert_eq!(
        second,
        SignalOutcome::Duplicate,
        "the double-delivery is a no-op (ON CONFLICT DO NOTHING)"
    );
    assert_eq!(
        sub.signals.count_for_run(&tenant(), &run.0),
        1,
        "ONE buffered ci.result (wakes once)"
    );
    sub.runs.wake(&tenant(), &run.0);

    let w2 = fresh_worker(&sub, "worker-2", part, ci.clone(), merger.clone());
    let o2 = w2
        .tick(2_000, "2026-06-21T02:00:00Z", 7)
        .expect("worker-2 resumes");
    match o2 {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("outcome:merged:merged-deadbeef".into())],
            "the resumed run MERGED on the green rollup"
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
        "EXACTLY one merge (0 double-merge)"
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
    assert!(sub.runs.get(&tenant(), &run.0).unwrap().state == run_state::COMPLETED);

    println!(
        "[2026-06-21] PASS  drill=merge-queue-in-isolation  scenario=success  \
         park->state=waiting  double-delivery->buffered=1  wake=1  merge=1  git.pr.merged=1  \
         re-dispatch=0  producer=MOCK(P-FLOW-22 floor)"
    );
}

#[test]
fn merge_queue_failure_one_dequeue_humanised_reason() {
    let ci = Arc::new(CountingCi::default());
    let merger = Arc::new(CountingMerger::default());
    let (ex, run, sub) = start("queue:main:pr-8");
    let part = partition_for_run_id(&run.0);

    let w1 = fresh_worker(&sub, "worker-1", part, ci.clone(), merger.clone());
    assert_eq!(
        w1.tick(1_000, "2026-06-21T00:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "parked"
    );
    drop(w1);

    let attempt = merge_attempt_id(&run.0, "merge.queue:0");
    deliver_ci_result(
        &ex,
        &run,
        &attempt,
        "deadbeef",
        CiOverall::Failure,
        vec!["build".into(), "test".into()],
    );
    sub.runs.wake(&tenant(), &run.0);

    let w2 = fresh_worker(&sub, "worker-2", part, ci.clone(), merger.clone());
    let o2 = w2
        .tick(2_000, "2026-06-21T01:00:00Z", 7)
        .expect("worker-2 resumes");
    match o2 {
        DriveOutcome::Completed(refs) => {
            let r = &refs[0].0;
            assert!(
                r.starts_with("outcome:dequeued:"),
                "the PR was dequeued: {r}"
            );
            assert!(
                r.contains("CI failed"),
                "humanised reason (contract 7.3): {r}"
            );
            assert!(!r.contains("ActivityError"), "no raw error code: {r}");
        }
        other => panic!("expected dequeue, got {other:?}"),
    }
    assert_eq!(
        merger.merges.load(Ordering::SeqCst),
        0,
        "no merge on failure"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        0,
        "no git.pr.merged on failure"
    );

    println!(
        "[2026-06-21] PASS  drill=merge-queue-in-isolation  scenario=failure  \
         dequeue=1(humanised)  merge=0  git.pr.merged=0"
    );
}

#[test]
fn merge_queue_vanished_ci_run_timeout_bounds_the_wait() {
    let ci = Arc::new(CountingCi::default());
    let merger = Arc::new(CountingMerger::default());
    let (_ex, run, sub) = start("queue:main:pr-9");
    let part = partition_for_run_id(&run.0);

    let w1 = fresh_worker(&sub, "worker-1", part, ci.clone(), merger.clone());
    assert_eq!(
        w1.tick(1_000, "2026-06-21T00:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "dispatched, parked with the SLA timeout-timer"
    );
    drop(w1);

    sub.runs.wake(&tenant(), &run.0);
    let w2 = fresh_worker(&sub, "worker-2", part, ci.clone(), merger.clone());
    let o2 = w2
        .tick(10_000, "2026-06-21T03:00:00Z", 7)
        .expect("worker-2 re-drives past the deadline");
    match o2 {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("outcome:timedout".into())],
            "a vanished CI run timed out (the queue continues), NOT parked forever"
        ),
        other => panic!("expected TimedOut, got {other:?}"),
    }
    assert_eq!(
        ci.calls.load(Ordering::SeqCst),
        1,
        "0 re-dispatch - the dispatch short-circuited on replay"
    );
    assert_eq!(
        merger.merges.load(Ordering::SeqCst),
        0,
        "no merge on a vanished CI run"
    );

    println!(
        "[2026-06-21] PASS  drill=merge-queue-in-isolation  scenario=vanished-ci  \
         timeout-bounded=yes  merge=0  re-dispatch=0"
    );
}
