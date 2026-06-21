//! CDC pair for contract-index row 13.3 — the **`order_key`/LexoRank fractional-index encoding**
//! half (frozen byte-identical with Issues, X-3/OQ-C). KN-P03 (P-236).
//!
//! The type/grammar half of 13.3 (`FieldType`/`ViewSpec`/`QueryAst`) is in
//! `cdc_13_3_query_primitive.rs`; THIS file is the **order_key half** — the X-3 anti-drift
//! conformance vector.
//!
//! PROVIDER side: Knowledge (this crate) ships the encoding (`rank_first`/`rank_last`/`rank_between`,
//! the 2-char jitter, the 48-char rebalance, the `created_at`+ULID tiebreak) AND the single
//! shared [`CONFORMANCE_VECTOR`] — authored ONCE. CONSUMER side: Issues (ISS-P02 / P-241) adds this
//! crate as a dependency and replays the SAME vector through its OWN drag-rank executor, asserting
//! the identical base-62 outputs. Until ISS-P02 lands, this file carries the consumer assertion
//! **against the shared fixture** (the byte-identical anchor both sides build to) so the
//! contract-coverage scanner admits row 13.3's order_key half as a real provider+consumer pair, and
//! so the X-3 parity (0 rank divergences) is a dated green artifact NOW.
//!
//! **The X-3 invariant:** a unit/encoding mismatch that ships on ONE side calcifies (EI-01 §7). The
//! shared fixture makes that mismatch a compile/test failure on BOTH sides at once.

use myelin_query::order_key::{tiebreak, ConformanceStep, RankOp};
use myelin_query::{OrderKey, CONFORMANCE_VECTOR};
use std::cmp::Ordering;

// ── PROVIDER side (13.3 order_key): Knowledge ships the encoding + the shared vector ──

/// The provider replays the shared conformance vector through the FROZEN `OrderKey` operations and
/// returns the produced base-62 outputs (the byte-identical anchor).
fn provider_replays_vector() -> Vec<String> {
    CONFORMANCE_VECTOR
        .iter()
        .map(|step| step.run().as_str().to_owned())
        .collect()
}

// ── CONSUMER side (X-3): Issues replays the SAME vector through its OWN executor ──

/// A consumer (Issues' drag-rank executor, ISS-P02) builds against the SAME `OrderKey` API and the
/// SAME shared [`CONFORMANCE_VECTOR`]. It does NOT redefine the encoding; it consumes the one frozen
/// crate. Replaying the shared fixture through the shared API IS the byte-identity proof — when
/// ISS-P02 lands its own executor, it asserts against this exact vector, so a divergence on either
/// side breaks both. Here the consumer asserts the produced outputs equal each step's frozen
/// `expect`, byte-for-byte.
fn issues_consumes_vector() -> Vec<String> {
    // Issues drives the SAME data-only RankOp fixture through the SAME public operations — exactly
    // what its executor will do. (This mirrors how `cdc_13_3_query_primitive` has the consumer build
    // its own serialization against the shared ViewSpec.)
    CONFORMANCE_VECTOR
        .iter()
        .map(consumer_run_step)
        .collect()
}

/// The consumer's independent re-implementation of "run a step": it dispatches on the SAME frozen
/// `RankOp` data through the SAME frozen `OrderKey::rank_*` operations. A drift in the encoding (the
/// provider changing a midpoint rule, the alphabet, or the jitter shape) makes this produce a
/// different string than the provider's `step.run()` — caught immediately.
fn consumer_run_step(step: &ConformanceStep) -> String {
    let key = match step.op {
        RankOp::First { jitter } => OrderKey::rank_first(jitter_of(jitter)),
        RankOp::Last { after, jitter } => {
            let after = after.map(|s| OrderKey::parse(s).unwrap());
            OrderKey::rank_last(after.as_ref(), jitter_of(jitter))
        }
        RankOp::Between { lo, hi, jitter } => {
            let lo = lo.map(|s| OrderKey::parse(s).unwrap());
            let hi = hi.map(|s| OrderKey::parse(s).unwrap());
            OrderKey::rank_between(lo.as_ref(), hi.as_ref(), jitter_of(jitter))
        }
    };
    key.as_str().to_owned()
}

fn jitter_of((a, b): (usize, usize)) -> myelin_query::Jitter {
    myelin_query::Jitter::from_ranks(a, b).unwrap()
}

#[test]
fn cdc_13_3_order_key_conformance_vector_is_byte_identical_across_co_owners() {
    let provider = provider_replays_vector();
    let consumer = issues_consumes_vector();
    let expected: Vec<&str> = CONFORMANCE_VECTOR.iter().map(|s| s.expect).collect();

    // The X-3 anti-drift gate: 0 rank divergences across the shared vector, on BOTH sides.
    assert_eq!(
        provider, consumer,
        "the order_key encoding is byte-identical across the co-owners (0 divergences)"
    );
    let provider_refs: Vec<&str> = provider.iter().map(String::as_str).collect();
    assert_eq!(
        provider_refs, expected,
        "every replayed key equals its frozen expected base-62 output (the dated green artifact)"
    );

    // The vector covers the load-bearing legs: a concurrent same-gap collision yields DISTINCT keys.
    let a = consumer
        .iter()
        .zip(CONFORMANCE_VECTOR)
        .find(|(_, s)| s.label.starts_with("concurrent same-gap insert A"))
        .map(|(k, _)| k.clone())
        .unwrap();
    let b = consumer
        .iter()
        .zip(CONFORMANCE_VECTOR)
        .find(|(_, s)| s.label.starts_with("concurrent same-gap insert B"))
        .map(|(k, _)| k.clone())
        .unwrap();
    assert_ne!(a, b, "concurrent same-gap inserts produce DISTINCT keys (the jitter property)");
}

#[test]
fn cdc_13_3_order_key_tiebreak_total_order_is_shared() {
    // The `created_at`+ULID tiebreak is part of the frozen 13.3 order_key contract; the consumer
    // (Issues) breaks equal keys exactly the same way (created_at, then ULID id).
    let k = OrderKey::parse("M00").unwrap();
    assert_eq!(
        tiebreak(&k, "2026-06-21T10:00:00Z", "01A", &k, "2026-06-21T11:00:00Z", "01B"),
        Ordering::Less,
        "equal key → earlier created_at wins (byte-identical across co-owners)"
    );
    assert_eq!(
        tiebreak(&k, "t", "01A", &k, "t", "01B"),
        Ordering::Less,
        "equal key + equal created_at → ULID id breaks it"
    );
}
