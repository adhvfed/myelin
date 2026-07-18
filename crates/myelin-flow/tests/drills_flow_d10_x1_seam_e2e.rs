//! # GIT-D10 / CI-D8 — the X-1 seam END-TO-END (P-FLOW-23 → P-346, M4)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` rows
//! **GIT-D10** + **CI-D8** (the X-1 check seam end-to-end). **Thresholds (exact — NEVER weaken):**
//! - push → `ci.check.updated` per context → green → merge;
//! - out-of-order / re-delivered `ci.check.updated` → `run_attempt`-monotonic supersession holds,
//!   drops the stale lower attempt;
//! - a fork PR self-green is **NEUTRAL for gating** (a required context not actually green never
//!   unblocks the merge);
//! - the merge-queue durable workflow wakes on `ci.result` **idempotently** — a doubly-delivered
//!   rollup wakes EXACTLY once;
//! - **0 double-merge** (merge-count == 1 per attempt), **0 spurious unblocks**, across re-delivery
//!   + restart.
//!
//! ## What this drill proves — the X-1 seam END-TO-END against CI's REAL producer (the floor CLOSED)
//!
//! P-FLOW-19 built + drilled the merge-queue body IN ISOLATION against the MOCK producer (the named
//! FLOOR). **P-FLOW-23 CLOSES that floor**: this drill drives the merge-queue durable workflow body
//! (`run_merge_attempt`) on the REAL durable substrate (a [`FlowDispatcher`] over a `RunStore` +
//! journal + signal buffer + outbox + timer wheel) and wakes it on the rollup CI's REAL producer
//! ([`RealCiResultProducer`]) **DERIVES** — `myelin_events::check_seam::{CheckSeamOrder,
//! rollup_ci_result}` (contract 5.9, CI-owned) — from per-context `ci.check.updated` facts, NOT a
//! fabricated verdict. The whole flow is exercised end-to-end:
//!
//! 1. CI emits per-context `ci.check.updated` facts (interleaved, out-of-`seq`, with a re-run + an
//!    at-least-once duplicate) for the speculative merge commit.
//! 2. The Bus carries them per-aggregate ordered on `(repo, commit_oid)` (the D-11 substrate).
//! 3. Git's `run_attempt` last-writer-wins supersession collapses each context to its CURRENT row.
//! 4. CI DERIVES the `ci.result` rollup over Git's REQUIRED gate set ([`rollup_ci_result`]).
//! 5. The merge-queue workflow — parked across a worker restart — wakes on the rollup (delivered
//!    TWICE, at-least-once) EXACTLY once → merges EXACTLY once → emits `git.pr.merged` once.
//!
//! Green artifact: **0 double-merge, merge-count == 1/attempt, 1 wake/attempt, 0 spurious unblocks**,
//! dated, against the REAL producer. A red drill is information — never weaken it (EI-01 §3).

use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    merge_attempt_id, partition_for_run_id, run_state, CheckFact, CiDispatch, CiDispatcher,
    DriveOutcome, DurableExecutor, FlowDispatcher, FlowExecutor, FlowTelemetry, MergeOutcome,
    MergePerformer, MergeRequest, RealCiResultProducer, RunStore, SignalStore, TimerStore, WfCtx,
    WfJournal, WorkflowBody,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::reserve_settle::MinorUnits;
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

/// A CI dispatcher that counts dispatches (so the drill proves 0 re-dispatch across a restart).
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

/// A merge performer that counts merges (so the drill proves EXACTLY one merge per attempt —
/// merge-count == 1, the 0-double-merge threshold).
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
                MinorUnits(0),
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

/// The required gate set (Git's policy — the merge proceeds only when EVERY one is green).
fn required() -> Vec<String> {
    vec!["build".into(), "test".into()]
}

/// **GIT-D10/CI-D8 (a)+(d): push → out-of-order/dup `ci.check.updated` → supersession holds → CI
/// DERIVES a green `ci.result` → the merge queue wakes EXACTLY ONCE on the doubly-delivered rollup →
/// 0 double-merge, across a worker restart.**
///
/// CI emits the per-context checks SCRAMBLED + out-of-`seq`, with a re-run of `build` (a higher
/// `run_attempt` that supersedes a stale failure) and an at-least-once duplicate of a check.
/// [`RealCiResultProducer`] runs them through CI's REAL ordering + supersession + rollup — NOT a
/// fabricated verdict — and delivers the green rollup TWICE. The parked workflow (worker crashed)
/// resumes and merges EXACTLY once.
#[test]
fn x1_seam_e2e_green_rollup_from_real_producer_one_merge_across_restart() {
    let ci = Arc::new(CountingCi::default());
    let merger = Arc::new(CountingMerger::default());
    let (_ex, run, sub) = start("queue:main:pr-7");
    let part = partition_for_run_id(&run.0);

    // WORKER 1: dispatch CI + PARK on ci.result (state=waiting, holds no runtime).
    let w1 = fresh_worker(&sub, "worker-1", part, ci.clone(), merger.clone());
    assert_eq!(
        w1.tick(1_000, "2026-06-21T00:00:00Z", 7).unwrap(),
        DriveOutcome::Waiting,
        "the run PARKED on the ci.result wait"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING,
        "state=waiting — holds no runtime across the multi-hour CI run"
    );
    assert_eq!(ci.calls.load(Ordering::SeqCst), 1, "CI dispatched once");
    drop(w1); // worker crashes + service redeployed while parked (hours pass).

    // ── CI's REAL producer: per-context ci.check.updated facts, delivered SCRAMBLED + out-of-seq,
    // with a build RE-RUN (run_attempt 2 supersedes the stale failure at attempt 1) + a duplicate.
    let facts = vec![
        CheckFact {
            context: "build".into(),
            run_attempt: 2,
            success: true,
            seq: 3,
        }, // the re-run (current build = success) — ARRIVES FIRST (out of order)
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
        }, // the STALE build failure — arrives AFTER the re-run; supersession drops it
        CheckFact {
            context: "test".into(),
            run_attempt: 1,
            success: true,
            seq: 2,
        }, // an at-least-once DUPLICATE of test#1 (same seq) — absorbed
    ];

    let attempt = merge_attempt_id(&run.0, "merge.queue:0");
    let producer = RealCiResultProducer::new(&sub.signals, tenant(), region(), &run.0, REPO);

    // The producer DERIVES the green rollup (build's current attempt is success#2, not the stale
    // failure#1; test is success) and delivers it TWICE (at-least-once).
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

    // WORKER 2 (redeployed): re-lease + resume + merge.
    let w2 = fresh_worker(&sub, "worker-2", part, ci.clone(), merger.clone());
    match w2.tick(2_000, "2026-06-21T02:00:00Z", 7).expect("resume") {
        DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("outcome:merged:merged-deadbeefcafe".into())],
            "the resumed run MERGED on the REAL green rollup (build supersession honoured)"
        ),
        other => panic!("expected the run to merge + complete, got {other:?}"),
    }

    // THE THRESHOLDS: 1 wake, merge-count == 1 (0 double-merge), 1 git.pr.merged, 0 re-dispatch.
    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "the ci.result was consumed EXACTLY ONCE (1 wake)"
    );
    assert_eq!(
        merger.merges.load(Ordering::SeqCst),
        1,
        "merge-count == 1 (0 double-merge) — the GIT-D10/CI-D8 threshold"
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

/// **GIT-D10/CI-D8 (a): a SUPERSEDING FAILURE → the REAL rollup is `failure` → the merge is DEQUEUED;
/// 0 spurious unblocks.** CI re-runs `test` and the re-run FAILS (a higher `run_attempt` supersedes
/// the earlier success). CI's real rollup derives `failure` over the required set; the merge queue
/// dequeues with a humanised reason — it does NOT merge on the stale green.
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

    // build green; test was green at attempt 1 but the RE-RUN (attempt 2) FAILED → supersedes.
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
        }, // the STALE green
        CheckFact {
            context: "test".into(),
            run_attempt: 2,
            success: false,
            seq: 3,
        }, // the superseding FAILURE (current test = failure)
    ];

    let attempt = merge_attempt_id(&run.0, "merge.queue:0");
    let producer = RealCiResultProducer::new(&sub.signals, tenant(), region(), &run.0, REPO);
    // The REAL rollup must be FAILURE (test's current attempt failed) — never the stale green.
    let rollup = producer.rollup(COMMIT, &facts, &required(), &attempt);
    assert_eq!(
        rollup.overall,
        myelin_events::check_seam::CiOverall::Failure,
        "the REAL rollup derives FAILURE — the superseding failure is the current verdict"
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

/// **GIT-D10/CI-D8 (b): a FORK self-green is NEUTRAL for gating — a required context not actually
/// green never unblocks the merge.** A fork PR reports `build` green but the REQUIRED `test` context
/// is ABSENT (the fork could not self-green a required check it does not run / is held neutral until
/// endorsed). CI's real rollup over the required set derives `failure` (a missing required context
/// never implicitly passes) → the merge queue dequeues; 0 spurious unblock.
#[test]
fn x1_seam_e2e_fork_self_green_is_neutral_for_gating() {
    let producer_signals = SignalStore::new();
    let producer = RealCiResultProducer::new(&producer_signals, tenant(), region(), "R-fork", REPO);

    // The fork self-greens `build` only — the required `test` context is ABSENT (neutral until
    // endorsed; the fork cannot self-green a required check it does not actually run).
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
        "a fork self-green is NEUTRAL — the missing required `test` keeps the gate CLOSED \
         (0 spurious unblock)"
    );
    // The rollup's contexts are the required gate set (sorted, byte-stable) — never the fork's.
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
