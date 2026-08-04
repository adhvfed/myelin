//! # The CDC pair for the Git↔CI merge-queue seam — contract 5.9 (the `ci.result` rollup, the
//! merge-queue CONSUMER half, P-FLOW-19)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.9 (the
//! Git↔CI CheckStatus / `ci.result` seam — CI/Git own the DATA shape; this engine owns ONLY the
//! durable-workflow mechanics) + row 9.4 (the `ci.result` wait — owned, the durable half) + row 7.3
//! (humanise — the dequeue reason). Owning architecture: `durable-workflow.md` §6.5 (the merge-queue
//! durable workflow + the `ci.result` rollup wait — the single most load-bearing cross-subsystem
//! seam, X-1).
//!
//! ## What this pair pins (the PROVIDER ↔ CONSUMER agreement of the 5.9 seam's CONSUMING half)
//!
//! The `ci.result` data SHAPE (`{ commit_oid, overall, contexts, idem_token }`) is OWNED by CI
//! ([`myelin_events::check_seam::CiResult`], contract 5.9) — this engine IMPORTS it, never redefines
//! it. THIS pair pins the merge-queue's CONSUMING half of the seam:
//!
//! **5.9 CONSUMER (the `myelin-flow` merge-queue body, [`WfCtx::run_merge_attempt`]) — what the
//! engine relies on:**
//! - it dispatches the required CI under a DETERMINISTIC `merge_attempt_id`
//!   ([`merge_attempt_id`]) and parks on `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)`;
//! - a `success` rollup for ALL required contexts → exactly one merge + one `git.pr.merged`;
//! - a `failure` rollup → one dequeue with a humanised reason (contract 7.3); the queue continues.
//!
//! **5.9 PROVIDER (CI's `ci.result` producer — the NAMED FLOOR, lands in M4) — what it must do:**
//! - emit the rollup as a [`CiResult`] keyed on the `merge_attempt_id` the workflow minted (the
//!   no-coordination agreement — CI echoes the dispatch id WITHOUT a coordination round-trip);
//! - deliver it (possibly DOUBLY, at-least-once) → the wait consumes it ONCE (one wake).
//!
//! This pair proves the two ends RECONCILE — the merge-queue consumer (this engine) paired with the
//! CI provider's shape. The M2 prompt pinned the agreement against a MOCK CI producer
//! ([`MockCiResultProducer`]); **P-FLOW-23 (M4) CLOSES the floor** by pairing the consumer with CI's
//! REAL producer ([`RealCiResultProducer`]) — the rollup is now DERIVED from per-context
//! `ci.check.updated` facts through CI's REAL `myelin_events::check_seam::{CheckSeamOrder,
//! rollup_ci_result}` (the X-1 seam END-TO-END, GIT-D10 / CI-D8), not fabricated. Both halves of the
//! 5.9 pair are now green against the real producer.

use myelin_events::check_seam::{CiOverall, CiResult};
use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    decode_ci_result, encode_ci_result, merge_attempt_id, CheckFact, CiDispatch, CiDispatcher,
    MergeOutcome, MergePerformer, MergeRequest, MockCiResultProducer, RealCiResultProducer,
    SignalStore, WfCtx, WfJournal, CI_RESULT_SIGNAL,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::reserve_settle::MicroUsd;
use myelin_tenancy::{Region, TenantId};
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
struct OkCi;
impl CiDispatcher for OkCi {
    fn dispatch(&self, _ci: &CiDispatch) -> Result<(), myelin_flow::ActivityError> {
        Ok(())
    }
}
#[derive(Default)]
struct OkMerger;
impl MergePerformer for OkMerger {
    fn merge(&self, request: &MergeRequest) -> Result<String, myelin_flow::ActivityError> {
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

/// **PROVIDER shape (5.9): CI emits the rollup as the OWNED [`CiResult`] shape, keyed on the
/// merge_attempt_id the workflow minted.** This pins that the CI provider (M4) and the merge-queue
/// consumer (this engine) agree on the `{ commit_oid, overall, contexts, idem_token }` shape — the
/// engine consumes CI's shape, never a redefinition. The mock producer encodes the SAME [`CiResult`]
/// the real CI producer will.
#[test]
fn provider_ci_result_shape_is_the_ci_owned_shape() {
    // The CI-owned shape (imported from myelin-events::check_seam — NOT redefined here).
    let rollup = CiResult {
        commit_oid: "deadbeef".into(),
        overall: CiOverall::Success,
        contexts: vec!["build".into(), "test".into()],
        idem_token: "R1/merge.queue:0/merge".into(),
    };
    // The merge-queue's references-not-payloads codec round-trips the OWNED shape (the engine carries
    // CI's shape opaquely; it does not own its fields).
    let refs = encode_ci_result(&rollup);
    let back = decode_ci_result(&refs, &rollup.idem_token).expect("decodable");
    assert_eq!(
        back, rollup,
        "the engine consumes CI's CiResult shape, never a redefinition"
    );
}

/// **CONSUMER side (5.9 / 9.4): the merge queue dispatches under a deterministic merge_attempt_id +
/// parks on `ci.result`; CI's mock producer echoes the SAME id → the consumer merges.** The
/// no-coordination agreement: the producer derives the SAME `merge_attempt_id` the consumer minted,
/// via [`merge_attempt_id`], WITHOUT a coordination round-trip.
#[test]
fn consumer_and_provider_agree_on_the_merge_attempt_id_without_coordination() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();

    // PRODUCER side (CI, mocked): derive the merge_attempt_id the dispatch will mint (command :0) and
    // deliver the rollup keyed on it — exactly what CI does in M4, WITHOUT coordinating with the run.
    let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
    let attempt = merge_attempt_id("R1", "merge.queue:0");
    producer.deliver(
        &attempt,
        "deadbeef",
        CiOverall::Success,
        vec!["build".into(), "test".into()],
    );

    // CONSUMER side (the merge queue): dispatch + consume + merge.
    let mut ctx = begin(&outbox, journal, signals);
    let out = ctx
        .run_merge_attempt(&request(), &OkCi, &OkMerger, None, MicroUsd(0), vec![])
        .expect("merge");
    match out {
        MergeOutcome::Merged {
            merge_attempt_id: id,
            ..
        } => assert_eq!(
            id, attempt,
            "RECONCILE: the consumer keyed on the SAME id the producer echoed (no coordination)"
        ),
        other => panic!("expected Merged, got {other:?}"),
    }
    assert_eq!(
        ctx.staged_emit_len(),
        1,
        "the consumer emitted git.pr.merged once"
    );
}

/// **CONSUMER reliance (9.4 / X-1): a DOUBLE-delivered `ci.result` wakes the merge queue ONCE.** The
/// CI provider may deliver the rollup twice (at-least-once); the consumer's wait consumes it once →
/// one merge, never two. This is the X-1 idempotency the producer relies on.
#[test]
fn a_double_delivered_ci_result_wakes_the_merge_queue_once() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();

    let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
    let attempt = merge_attempt_id("R1", "merge.queue:0");
    let first = producer.deliver(
        &attempt,
        "deadbeef",
        CiOverall::Success,
        vec!["build".into(), "test".into()],
    );
    let second = producer.deliver(
        &attempt,
        "deadbeef",
        CiOverall::Success,
        vec!["build".into(), "test".into()],
    );
    assert!(first, "first delivery is new");
    assert!(
        !second,
        "the at-least-once double-delivery deduped on the merge_attempt_id"
    );
    assert_eq!(signals.buffered_depth(), 1, "ONE buffered ci.result");

    let mut ctx = begin(&outbox, journal, signals.clone());
    let out = ctx
        .run_merge_attempt(&request(), &OkCi, &OkMerger, None, MicroUsd(0), vec![])
        .expect("merge");
    assert!(matches!(out, MergeOutcome::Merged { .. }));
    assert_eq!(
        ctx.consumed_signals().len(),
        1,
        "the consumer woke ONCE on the double-delivered rollup"
    );
    assert_eq!(
        ctx.staged_emit_len(),
        1,
        "ONE git.pr.merged (0 double-merge)"
    );
}

/// **RECONCILE on FAILURE (5.9 / 7.3): a `failure` rollup → the consumer dequeues with a humanised
/// reason; no merge.** The CI provider reports `overall: failure`; the merge-queue consumer dequeues
/// the PR with a contract-7.3 humanised reason (no raw error code), and the queue continues.
#[test]
fn a_failure_rollup_reconciles_to_a_humanised_dequeue() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();

    let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
    let attempt = merge_attempt_id("R1", "merge.queue:0");
    producer.deliver(
        &attempt,
        "deadbeef",
        CiOverall::Failure,
        vec!["build".into(), "test".into()],
    );

    let mut ctx = begin(&outbox, journal, signals);
    let out = ctx
        .run_merge_attempt(&request(), &OkCi, &OkMerger, None, MicroUsd(0), vec![])
        .expect("dequeue");
    match out {
        MergeOutcome::Dequeued { reason } => {
            assert!(
                reason.contains("CI failed"),
                "humanised (contract 7.3): {reason}"
            );
            assert!(
                !reason.contains("ActivityError"),
                "no raw error code: {reason}"
            );
        }
        other => panic!("expected Dequeued, got {other:?}"),
    }
    assert_eq!(
        ctx.staged_emit_len(),
        0,
        "no git.pr.merged on a failure rollup"
    );
}

/// **5.9 PROVIDER (REAL) ↔ CONSUMER reconcile END-TO-END (P-FLOW-23, GIT-D10/CI-D8).** The CI
/// provider half is now the REAL [`RealCiResultProducer`]: it DERIVES the rollup from per-context
/// `ci.check.updated` facts through CI's REAL ordering + `run_attempt` supersession + rollup (NOT a
/// fabricated verdict), keyed on the merge_attempt_id the workflow minted. The merge-queue consumer
/// wakes on that derived rollup and merges — the two ends reconcile against the real producer, the
/// floor CLOSED. A build re-run (attempt 2 success) supersedes a stale failure (attempt 1) that
/// arrives out of order.
#[test]
fn consumer_reconciles_with_the_real_ci_producer_end_to_end() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();

    // PROVIDER (CI, REAL): per-context facts — build re-run supersedes a stale failure; test green.
    let facts = vec![
        CheckFact {
            context: "build".into(),
            run_attempt: 2,
            success: true,
            seq: 3,
        },
        CheckFact {
            context: "build".into(),
            run_attempt: 1,
            success: false,
            seq: 1,
        }, // stale failure (out of order) — superseded
        CheckFact {
            context: "test".into(),
            run_attempt: 1,
            success: true,
            seq: 2,
        },
    ];
    let attempt = merge_attempt_id("R1", "merge.queue:0");
    let producer = RealCiResultProducer::new(
        &signals,
        tenant(),
        region(),
        "R1",
        "myelin://acme/git/repo/core",
    );
    // The REAL rollup is success (supersession honoured); deliver it on the minted attempt id.
    let derived = producer.rollup("deadbeef", &facts, &request().required_contexts, &attempt);
    assert_eq!(
        derived.overall,
        CiOverall::Success,
        "the REAL producer derives success (the stale failure was superseded)"
    );
    producer.deliver("deadbeef", &facts, &request().required_contexts, &attempt);

    // CONSUMER (the merge queue): dispatch + consume the DERIVED rollup + merge.
    let mut ctx = begin(&outbox, journal, signals);
    let out = ctx
        .run_merge_attempt(&request(), &OkCi, &OkMerger, None, MicroUsd(0), vec![])
        .expect("merge");
    match out {
        MergeOutcome::Merged {
            merge_attempt_id: id,
            ..
        } => assert_eq!(
            id, attempt,
            "RECONCILE: the consumer merged on the REAL derived rollup keyed on the minted id"
        ),
        other => panic!("expected Merged on the real rollup, got {other:?}"),
    }
    assert_eq!(
        ctx.staged_emit_len(),
        1,
        "one git.pr.merged on the real-producer rollup"
    );
}

/// **The signal NAME both ends agree on is the NAMED `ci.result` token.** The consumer parks on
/// [`CI_RESULT_SIGNAL`] = `ci.result`; the producer keys on the same — agreement by construction (the
/// token comes from `myelin-events`, the seam-owning crate).
#[test]
fn both_ends_agree_on_the_ci_result_signal_name() {
    assert_eq!(CI_RESULT_SIGNAL, "ci.result");
    assert_eq!(
        CI_RESULT_SIGNAL,
        myelin_events::check_seam::CiResultWaitSubstrate::SIGNAL_NAME,
        "the merge-queue signal name == the seam-owning crate's ci.result token"
    );
}
