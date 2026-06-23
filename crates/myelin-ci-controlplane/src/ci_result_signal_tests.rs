//! Unit tests for the X-1 `ci.result` rollup-signal producer (CI-P19 → P-362, M4).
//!
//! These prove the producer half of contract 5.9 IN PROCESS over the FROZEN `myelin-flow`
//! `SignalStore` (the `wf_signal` substrate the merge-queue workflow parks on): the rollup is the
//! frozen 5.9 verdict (REUSES `rollup_ci_result`), the signal is references-not-payloads + keyed on
//! the `idem_token`, and a doubly-delivered rollup is buffered EXACTLY ONCE (the merge-queue wakes
//! once — 0 double-merge). The END-TO-END seam GATE (CI's real `run_ci_pipeline_body` → this signal →
//! Git's merge queue, GIT-D10/CI-D8) is `tests/drills_ci_p19_seam_gate.rs`.

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

/// **The rollup is the FROZEN 5.9 verdict: success iff EVERY required context succeeded.** The
/// producer REUSES `rollup_ci_result` — a missing/failing required context closes the gate (never an
/// implicit pass), and the rolled-up context set is the sorted required set (byte-stable).
#[test]
fn rollup_is_the_frozen_5_9_verdict() {
    let signals = SignalStore::new();
    let prod = CiResultSignal::new(&signals, tenant(), region(), RUN);

    // all required green → Success.
    let r = prod.rollup(COMMIT, &all_green(), &required(), "merge-7");
    assert_eq!(r.overall, CiOverall::Success);
    assert_eq!(r.commit_oid, COMMIT);
    assert_eq!(
        r.contexts,
        vec!["build".to_string(), "test".to_string()],
        "the rolled-up set is the sorted required gate set (byte-stable)"
    );
    assert_eq!(r.idem_token, "merge-7");

    // one required failed → Failure.
    let mut mixed = all_green();
    mixed.insert("test".into(), false);
    assert_eq!(
        prod.rollup(COMMIT, &mixed, &required(), "merge-7").overall,
        CiOverall::Failure
    );

    // a required context CI never reported → Failure (the gate stays closed; never an implicit pass).
    let mut partial = BTreeMap::new();
    partial.insert("build".into(), true);
    assert_eq!(
        prod.rollup(COMMIT, &partial, &required(), "merge-7")
            .overall,
        CiOverall::Failure,
        "a missing required context never implicitly passes"
    );
}

/// **The rollup signal is references-not-payloads + keyed on the `idem_token`, under the FROZEN
/// `ci.result` signal name.** The delivered `wf_signal` row carries the rollup as `ArtifactRef`s
/// (never a PII body), under `CI_RESULT_SIGNAL`, idem-keyed on the merge_attempt_id — and decodes
/// back through the merge-queue's OWN codec to the same verdict (no drift).
#[test]
fn signal_is_references_not_payloads_keyed_on_idem_token() {
    let signals = SignalStore::new();
    let prod = CiResultSignal::new(&signals, tenant(), region(), RUN);

    let out = prod.signal_ci_result(COMMIT, &all_green(), &required(), "merge-attempt-7");
    assert_eq!(out, RollupDelivery::Woke, "the first delivery wakes");
    assert_eq!(signals.buffered_depth(), 1, "ONE buffered ci.result row");

    // The signal payload decodes back through the merge-queue's OWN codec (idem_token off the
    // envelope, not the body) — the producer + consumer agree byte-for-byte (no drift).
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

/// **A doubly-delivered `ci.result` is buffered EXACTLY ONCE → the merge-queue wakes once (0
/// double-merge).** The `wf_signal` PK `(tenant, run_id, signal_name, idem_key)` dedups the
/// at-least-once double delivery: the first delivery `Woke`, every re-delivery is a `Duplicate`, and
/// the buffered depth stays 1.
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
        "EXACTLY one buffered ci.result row — the merge-queue wakes ONCE (0 double-merge)"
    );
}

/// **Distinct merge attempts (distinct `idem_token`s) wake independently.** A re-queue of the SAME PR
/// mints a NEW `merge_attempt_id`; its rollup is a distinct `wf_signal` row (one merge attempt's
/// rollup never wakes another's wait).
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

/// **A `failure` rollup still delivers (the merge queue dequeues on it).** CI reports the fact (a
/// failed required context → `overall: failure`); the merge queue consumes it and dequeues with a
/// humanised reason — CI never decides the merge, it emits the verdict.
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

/// **The split `rollup` + `deliver` halves compose: a re-derived-and-re-delivered SAME rollup is one
/// wake.** A re-drive re-derives the byte-identical rollup (deterministic) and re-delivers under the
/// same `idem_key` → the PK dedups it (the consume-exactly-once precondition).
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
