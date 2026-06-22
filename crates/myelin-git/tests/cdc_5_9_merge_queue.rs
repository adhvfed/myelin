//! # The CDC pair for contract 5.9 — the **merge-queue `ci.result` wait** half (GIT-P23 / P-285)
//!
//! **Contract:** `contract-index.md` row 5.9 (the Git↔CI seam — the merge-queue waking on the rollup
//! `ci.result`; idempotent on `merge_attempt_id`). Owning architecture:
//! `git-hosting/architecture/02-internals-and-algorithms.md` §6.4 (the merge queue parks on `ci.result`;
//! a doubly-delivered rollup wakes the workflow exactly once). **Reconciliation:** X-1.
//!
//! ## The seam this pair pins (CI produces the `ci.result` rollup; Git's merge queue consumes it)
//! `cdc_5_9_merge_gate_required_set.rs` (GIT-P21) pinned the required-set-POLICY half;
//! `cdc_5_9_fork_endorsement.rs` (GIT-P22) the fork-endorsement half. THIS pair pins the **merge-queue
//! `ci.result`-wait half**:
//!
//! - **PRODUCER (CI):** emits a `ci.result` rollup `{commit_oid, overall, contexts, idem_token}` keyed
//!   on the `merge_attempt_id` the workflow minted at dispatch (the no-coordination dedup agreement).
//!   The wire shape is the FROZEN `myelin_events::check_seam::CiResult` (never redefined). The transport
//!   is at-least-once — a rollup may be DOUBLY delivered.
//! - **CONSUMER (Git's merge queue):** parks on `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)`
//!   and decodes EXACTLY the producer's `CiResult` via the references-not-payloads codec. A
//!   doubly-delivered rollup wakes the workflow EXACTLY ONCE (the `wf_signal` PK dedup) → 0 double-merge.
//!
//! The load-bearing CDC assertion: **the producer's `ci.result` rollup round-trips through the signal
//! codec to EXACTLY the consumer's `CiResult`, and a double delivery under the same `merge_attempt_id`
//! is one buffered row (one wake).**

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

/// **PRODUCER side of 5.9 (the merge-queue half)** — CI builds the `ci.result` rollup over the FROZEN
/// `CiResult` shape. The `idem_token` is the `merge_attempt_id` (carried by the signal envelope, not the
/// payload).
fn producer_rollup(overall: CiOverall, contexts: Vec<String>, attempt: &str) -> CiResult {
    CiResult {
        commit_oid: HEAD.into(),
        overall,
        contexts,
        idem_token: attempt.into(),
    }
}

/// **THE CDC: the producer's `ci.result` rollup round-trips to EXACTLY the consumer's `CiResult` through
/// the references-not-payloads signal codec.** No second struct: encode → decode reconstructs the
/// verdict the consumer reads. The rollup carries only PII-free machine tokens.
#[test]
fn cdc_5_9_ci_result_rollup_round_trips_through_the_signal_codec() {
    let attempt = merge_attempt_id(RUN, "merge.queue:0");
    let rollup = producer_rollup(CiOverall::Success, vec!["ci/build".into(), "ci/test".into()], &attempt);

    // PRODUCER encodes the rollup into the signal's references-not-payloads body.
    let refs = encode_ci_result(&rollup);
    assert!(refs.iter().all(|r| r.0.starts_with("ci.result:")), "machine tokens only (no inline PII)");

    // CONSUMER decodes it — `idem_token` is the signal envelope's idem_key (the merge_attempt_id).
    let decoded = decode_ci_result(&refs, &attempt).expect("the rollup decodes");
    assert_eq!(decoded, rollup, "the producer rollup round-trips to EXACTLY the consumer's CiResult");
}

/// **THE CDC: a doubly-delivered `ci.result` under the same `merge_attempt_id` is ONE buffered row (one
/// wake) — 0 double-merge.** The at-least-once transport delivers the rollup twice; the `wf_signal` PK
/// (keyed on `(run, signal_name, idem_key)`) dedups it; the merge queue wakes ONCE.
#[test]
fn cdc_5_9_doubly_delivered_ci_result_is_one_buffered_row() {
    let signals = SignalStore::new();
    let producer = MockCiResultProducer::new(&signals, tenant(), region(), RUN);
    let attempt = merge_attempt_id(RUN, "merge.queue:0");

    let first = producer.deliver(&attempt, HEAD, CiOverall::Success, vec!["ci/build".into()]);
    let second = producer.deliver(&attempt, HEAD, CiOverall::Success, vec!["ci/build".into()]);

    assert!(first, "the first delivery is new");
    assert!(!second, "the at-least-once double-delivery deduped (ON CONFLICT DO NOTHING on the wf_signal PK)");
    assert_eq!(signals.buffered_depth(), 1, "ONE buffered ci.result row — the workflow wakes ONCE");
}

/// **THE CDC: the producer + consumer agree on the `merge_attempt_id` WITHOUT coordination.** The
/// producer derives the dedup key the same way the workflow mints it (`merge_attempt_id(run, cmd)`), so
/// the `ci.result` signal lands on the exact key the merge queue is waiting on — no round-trip.
#[test]
fn cdc_5_9_producer_and_consumer_agree_on_the_merge_attempt_id() {
    // The workflow mints this id at dispatch position 0 of run R1.
    let consumer_key = merge_attempt_id(RUN, "merge.queue:0");
    // The producer (CI) derives the SAME id independently — the no-coordination agreement.
    let producer_key = merge_attempt_id(RUN, "merge.queue:0");
    assert_eq!(consumer_key, producer_key, "producer and consumer derive the same merge_attempt_id");
    assert_eq!(consumer_key, "R1/merge.queue:0/merge");
}

/// **THE CDC: a `failure` rollup decodes to `overall: Failure` (the consumer dequeues, not merges).** A
/// failed CI rollup round-trips to a `Failure` verdict the merge queue reads to dequeue the PR.
#[test]
fn cdc_5_9_failure_rollup_decodes_to_failure() {
    let attempt = merge_attempt_id(RUN, "merge.queue:0");
    let rollup = producer_rollup(CiOverall::Failure, vec!["ci/build".into()], &attempt);
    let decoded = decode_ci_result(&encode_ci_result(&rollup), &attempt).expect("decodes");
    assert_eq!(decoded.overall, CiOverall::Failure, "a failure rollup decodes to Failure");
}
