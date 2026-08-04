use super::*;
use myelin_flow::{decode_ci_result, CI_RESULT_SIGNAL};

const COMMIT: &str = "deadbeefcafe";
const RUN: &str = "merge-queue-run-7";

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn required() -> Vec<String> {
    vec!["build".into(), "test".into()]
}

fn all_green() -> BTreeMap<String, bool> {
    let mut m = BTreeMap::new();
    m.insert("build".into(), true);
    m.insert("test".into(), true);
    m
}

#[test]
fn rollup_is_the_frozen_5_9_verdict() {
    let signals = SignalStore::new();
    let prod = CiResultSignal::new(&signals, tenant(), region(), RUN);

    let r = prod.rollup(COMMIT, &all_green(), &required(), "merge-7");
    assert_eq!(r.overall, CiOverall::Success);
    assert_eq!(r.commit_oid, COMMIT);
    assert_eq!(
        r.contexts,
        vec!["build".to_string(), "test".to_string()],
        "the rolled-up set is the sorted required gate set (byte-stable)"
    );
    assert_eq!(r.idem_token, "merge-7");

    let mut mixed = all_green();
    mixed.insert("test".into(), false);
    assert_eq!(
        prod.rollup(COMMIT, &mixed, &required(), "merge-7").overall,
        CiOverall::Failure
    );

    let mut partial = BTreeMap::new();
    partial.insert("build".into(), true);
    assert_eq!(
        prod.rollup(COMMIT, &partial, &required(), "merge-7")
            .overall,
        CiOverall::Failure,
        "a missing required context never implicitly passes"
    );
}

#[test]
fn signal_is_references_not_payloads_keyed_on_idem_token() {
    let signals = SignalStore::new();
    let prod = CiResultSignal::new(&signals, tenant(), region(), RUN);

    let out = prod.signal_ci_result(COMMIT, &all_green(), &required(), "merge-attempt-7");
    assert_eq!(out, RollupDelivery::Woke, "the first delivery wakes");
    assert_eq!(signals.buffered_depth(), 1, "ONE buffered ci.result row");

    let result = prod.rollup(COMMIT, &all_green(), &required(), "merge-attempt-7");
    let encoded = myelin_flow::encode_ci_result(&result);
    assert!(
        encoded.iter().all(|r| r.0.starts_with("ci.result:")),
        "references-not-payloads: PII-free machine tokens only"
    );
    let back = decode_ci_result(&encoded, "merge-attempt-7").expect("the rollup decodes");
    assert_eq!(back, result, "round-trips through the merge-queue codec");
    assert_eq!(CI_RESULT_SIGNAL, "ci.result", "the FROZEN signal name");
}

#[test]
fn double_delivered_ci_result_buffers_once_zero_double_merge() {
    let signals = SignalStore::new();
    let prod = CiResultSignal::new(&signals, tenant(), region(), RUN);

    let first = prod.signal_ci_result(COMMIT, &all_green(), &required(), "merge-attempt-7");
    let second = prod.signal_ci_result(COMMIT, &all_green(), &required(), "merge-attempt-7");
    let third = prod.signal_ci_result(COMMIT, &all_green(), &required(), "merge-attempt-7");

    assert_eq!(
        first,
        RollupDelivery::Woke,
        "first delivery wakes the queue"
    );
    assert_eq!(
        second,
        RollupDelivery::Duplicate,
        "the at-least-once double-delivery is absorbed (one wake)"
    );
    assert_eq!(third, RollupDelivery::Duplicate, "and a third is absorbed");
    assert_eq!(
        signals.buffered_depth(),
        1,
        "EXACTLY one buffered ci.result row - the merge-queue wakes ONCE (0 double-merge)"
    );
}

#[test]
fn distinct_merge_attempts_wake_independently() {
    let signals = SignalStore::new();
    let prod = CiResultSignal::new(&signals, tenant(), region(), RUN);

    assert_eq!(
        prod.signal_ci_result(COMMIT, &all_green(), &required(), "attempt-1"),
        RollupDelivery::Woke
    );
    assert_eq!(
        prod.signal_ci_result(COMMIT, &all_green(), &required(), "attempt-2"),
        RollupDelivery::Woke,
        "a re-queue mints a new merge_attempt_id → a distinct wf_signal row"
    );
    assert_eq!(
        signals.buffered_depth(),
        2,
        "two distinct merge attempts → two buffered rows"
    );
}

#[test]
fn a_failure_rollup_still_delivers_the_signal() {
    let signals = SignalStore::new();
    let prod = CiResultSignal::new(&signals, tenant(), region(), RUN);

    let mut failed = all_green();
    failed.insert("test".into(), false);
    let out = prod.signal_ci_result(COMMIT, &failed, &required(), "merge-attempt-7");
    assert_eq!(
        out,
        RollupDelivery::Woke,
        "a failure rollup wakes the queue (which dequeues on it)"
    );

    let result = prod.rollup(COMMIT, &failed, &required(), "merge-attempt-7");
    assert!(
        !CiResultSignal::is_success(&result),
        "the overall verdict is failure"
    );
}

#[test]
fn re_derived_and_re_delivered_rollup_is_one_wake() {
    let signals = SignalStore::new();
    let prod = CiResultSignal::new(&signals, tenant(), region(), RUN);

    let r1 = prod.rollup(COMMIT, &all_green(), &required(), "merge-attempt-7");
    let r2 = prod.rollup(COMMIT, &all_green(), &required(), "merge-attempt-7");
    assert_eq!(
        r1, r2,
        "the rollup derivation is deterministic (byte-identical)"
    );

    assert_eq!(prod.deliver(&r1), RollupDelivery::Woke);
    assert_eq!(
        prod.deliver(&r2),
        RollupDelivery::Duplicate,
        "the re-delivered identical rollup is absorbed (one wake)"
    );
    assert_eq!(signals.buffered_depth(), 1);
}
