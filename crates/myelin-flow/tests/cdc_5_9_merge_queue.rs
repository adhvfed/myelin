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

#[test]
fn provider_ci_result_shape_is_the_ci_owned_shape() {
    let rollup = CiResult {
        commit_oid: "deadbeef".into(),
        overall: CiOverall::Success,
        contexts: vec!["build".into(), "test".into()],
        idem_token: "R1/merge.queue:0/merge".into(),
    };
    let refs = encode_ci_result(&rollup);
    let back = decode_ci_result(&refs, &rollup.idem_token).expect("decodable");
    assert_eq!(
        back, rollup,
        "the engine consumes CI's CiResult shape, never a redefinition"
    );
}

#[test]
fn consumer_and_provider_agree_on_the_merge_attempt_id_without_coordination() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();

    let producer = MockCiResultProducer::new(&signals, tenant(), region(), "R1");
    let attempt = merge_attempt_id("R1", "merge.queue:0");
    producer.deliver(
        &attempt,
        "deadbeef",
        CiOverall::Success,
        vec!["build".into(), "test".into()],
    );

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

#[test]
fn consumer_reconciles_with_the_real_ci_producer_end_to_end() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();

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
        },
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
    let derived = producer.rollup("deadbeef", &facts, &request().required_contexts, &attempt);
    assert_eq!(
        derived.overall,
        CiOverall::Success,
        "the REAL producer derives success (the stale failure was superseded)"
    );
    producer.deliver("deadbeef", &facts, &request().required_contexts, &attempt);

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

#[test]
fn both_ends_agree_on_the_ci_result_signal_name() {
    assert_eq!(CI_RESULT_SIGNAL, "ci.result");
    assert_eq!(
        CI_RESULT_SIGNAL,
        myelin_events::check_seam::CiResultWaitSubstrate::SIGNAL_NAME,
        "the merge-queue signal name == the seam-owning crate's ci.result token"
    );
}
