use myelin_query::order_key::{tiebreak, ConformanceStep, RankOp};
use myelin_query::{OrderKey, CONFORMANCE_VECTOR};
use std::cmp::Ordering;

fn provider_replays_vector() -> Vec<String> {
    CONFORMANCE_VECTOR
        .iter()
        .map(|step| step.run().as_str().to_owned())
        .collect()
}

fn issues_consumes_vector() -> Vec<String> {
    CONFORMANCE_VECTOR.iter().map(consumer_run_step).collect()
}

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

    assert_eq!(
        provider, consumer,
        "the order_key encoding is byte-identical across the co-owners (0 divergences)"
    );
    let provider_refs: Vec<&str> = provider.iter().map(String::as_str).collect();
    assert_eq!(
        provider_refs, expected,
        "every replayed key equals its frozen expected base-62 output (the dated green artifact)"
    );

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
    assert_ne!(
        a, b,
        "concurrent same-gap inserts produce DISTINCT keys (the jitter property)"
    );
}

#[test]
fn cdc_13_3_order_key_tiebreak_total_order_is_shared() {
    let k = OrderKey::parse("M00").unwrap();
    assert_eq!(
        tiebreak(
            &k,
            "2026-06-21T10:00:00Z",
            "01A",
            &k,
            "2026-06-21T11:00:00Z",
            "01B"
        ),
        Ordering::Less,
        "equal key → earlier created_at wins (byte-identical across co-owners)"
    );
    assert_eq!(
        tiebreak(&k, "t", "01A", &k, "t", "01B"),
        Ordering::Less,
        "equal key + equal created_at → ULID id breaks it"
    );
}
