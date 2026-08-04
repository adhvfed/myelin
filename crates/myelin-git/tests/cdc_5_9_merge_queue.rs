use myelin_events::{CiOverall, CiResult};
use myelin_flow::{
    decode_ci_result, encode_ci_result, merge_attempt_id, MockCiResultProducer, SignalStore,
};
use myelin_tenancy::{Region, TenantId};

const HEAD: &str = "c0ffee";
const RUN: &str = "R1";

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn producer_rollup(overall: CiOverall, contexts: Vec<String>, attempt: &str) -> CiResult {
    CiResult {
        commit_oid: HEAD.into(),
        overall,
        contexts,
        idem_token: attempt.into(),
    }
}

#[test]
fn cdc_5_9_ci_result_rollup_round_trips_through_the_signal_codec() {
    let attempt = merge_attempt_id(RUN, "merge.queue:0");
    let rollup = producer_rollup(
        CiOverall::Success,
        vec!["ci/build".into(), "ci/test".into()],
        &attempt,
    );

    let refs = encode_ci_result(&rollup);
    assert!(
        refs.iter().all(|r| r.0.starts_with("ci.result:")),
        "machine tokens only (no inline PII)"
    );

    let decoded = decode_ci_result(&refs, &attempt).expect("the rollup decodes");
    assert_eq!(
        decoded, rollup,
        "the producer rollup round-trips to EXACTLY the consumer's CiResult"
    );
}

#[test]
fn cdc_5_9_doubly_delivered_ci_result_is_one_buffered_row() {
    let signals = SignalStore::new();
    let producer = MockCiResultProducer::new(&signals, tenant(), region(), RUN);
    let attempt = merge_attempt_id(RUN, "merge.queue:0");

    let first = producer.deliver(&attempt, HEAD, CiOverall::Success, vec!["ci/build".into()]);
    let second = producer.deliver(&attempt, HEAD, CiOverall::Success, vec!["ci/build".into()]);

    assert!(first, "the first delivery is new");
    assert!(
        !second,
        "the at-least-once double-delivery deduped (ON CONFLICT DO NOTHING on the wf_signal PK)"
    );
    assert_eq!(
        signals.buffered_depth(),
        1,
        "ONE buffered ci.result row - the workflow wakes ONCE"
    );
}

#[test]
fn cdc_5_9_producer_and_consumer_agree_on_the_merge_attempt_id() {
    let consumer_key = merge_attempt_id(RUN, "merge.queue:0");
    let producer_key = merge_attempt_id(RUN, "merge.queue:0");
    assert_eq!(
        consumer_key, producer_key,
        "producer and consumer derive the same merge_attempt_id"
    );
    assert_eq!(consumer_key, "R1/merge.queue:0/merge");
}

#[test]
fn cdc_5_9_failure_rollup_decodes_to_failure() {
    let attempt = merge_attempt_id(RUN, "merge.queue:0");
    let rollup = producer_rollup(CiOverall::Failure, vec!["ci/build".into()], &attempt);
    let decoded = decode_ci_result(&encode_ci_result(&rollup), &attempt).expect("decodes");
    assert_eq!(
        decoded.overall,
        CiOverall::Failure,
        "a failure rollup decodes to Failure"
    );
}
