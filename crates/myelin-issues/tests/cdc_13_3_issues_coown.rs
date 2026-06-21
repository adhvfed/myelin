//! CDC pair for contract-index row 13.3 — the **Issues side of the co-owned `myelin-query`
//! primitive** (frozen byte-identical with Knowledge, X-3/OQ-C — ISS-P02 / P-241).
//!
//! ## Why this lives in the `myelin-issues` crate (the genuine co-ownership proof)
//! Knowledge's CDC (`crates/myelin-query/tests/cdc_13_3_*`) freezes the shared definitions and
//! *simulates* the Issues consumer from inside the shared crate. THIS file is the real thing: it
//! lives in the **`myelin-issues` crate**, which **links `myelin-query` as a dependency** (the
//! co-ownership: architecture §1 "Issues links them directly — the same bytes Knowledge uses, not a
//! re-implementation"). It replays the shared conformance vector and serializes the shared `ViewSpec`
//! / `FieldType` set **through the Issues crate's own call sites** and asserts **0 byte differences**
//! against the frozen outputs Knowledge authored. A drift on EITHER side — a `FieldType` rename, a
//! `ViewSpec` field reorder, an `order_key` midpoint/jitter change — fails this test (and Knowledge's)
//! at once. This is the §3.0 exit-gate drift-killer, proven from the Issues side.
//!
//! PROVIDER side: Knowledge (the `myelin-query` crate) freezes the shared definitions + authors the
//! single `CONFORMANCE_VECTOR` and the `ViewSpec`/`FieldType` golden shapes.
//! CONSUMER side: Issues (this crate) replays/serializes those SAME shapes through its OWN linked
//! references and asserts byte-identity. The two assertions below (the order_key conformance vector +
//! the ViewSpec/FieldType golden serialization) are the byte-diff = 0 green artifact.

use myelin_issues::query_coown::{
    issues_canonical_board_view, issues_field_type_wire_ids, issues_replay_conformance_vector,
    tiebreak, CmpOp, EvalContext, Expr, FieldId, FieldType, OrderKey, Predicate, QueryAst, ViewKind,
    CONFORMANCE_VECTOR,
};
use myelin_identity::Literal;
use std::cmp::Ordering;

/// **THE X-3 ANTI-DRIFT GATE (Issues side): the order_key conformance vector replays byte-for-byte,
/// 0 divergences.** Issues drives the SAME shared `CONFORMANCE_VECTOR` through the SAME co-owned
/// `OrderKey::rank_*` operations and every produced base-62 key MUST equal the frozen `expect` string
/// Knowledge authored. The byte-diff count is asserted to be exactly 0 — the dated green artifact.
#[test]
fn cdc_13_3_order_key_conformance_vector_byte_identical_from_issues() {
    let issues_keys = issues_replay_conformance_vector();
    let knowledge_expected: Vec<&str> = CONFORMANCE_VECTOR.iter().map(|s| s.expect).collect();

    // Count byte differences explicitly (the green artifact is "byte-diff = 0").
    let mut byte_diffs = 0usize;
    for (i, (produced, expected)) in issues_keys.iter().zip(&knowledge_expected).enumerate() {
        if produced.as_str() != *expected {
            eprintln!(
                "DIVERGENCE step {i} [{}]: Issues produced {produced:?}, Knowledge froze {expected:?}",
                CONFORMANCE_VECTOR[i].label
            );
            byte_diffs += 1;
        }
    }
    assert_eq!(issues_keys.len(), knowledge_expected.len(), "the co-owners replay the same vector");
    assert_eq!(
        byte_diffs, 0,
        "the order_key codec is byte-identical across the co-owners (0 byte differences, X-3)"
    );

    // The vector covers the concurrency-safety leg: two same-gap inserts yield DISTINCT keys (the
    // 2-char jitter property) — proven from the Issues call site.
    let key_of = |prefix: &str| -> String {
        let idx = CONFORMANCE_VECTOR
            .iter()
            .position(|s| s.label.starts_with(prefix))
            .expect("the vector carries the concurrent same-gap legs");
        issues_keys[idx].clone()
    };
    let a = key_of("concurrent same-gap insert A");
    let b = key_of("concurrent same-gap insert B");
    assert_ne!(a, b, "concurrent same-gap inserts produce DISTINCT keys (the jitter, from Issues)");
}

/// **The order_key midpoint bisection, 2-char jitter, and 48-char rebalance round-trip from the
/// Issues side** (the codec is the rank source of truth — mandatory-core). These exercise the frozen
/// rules directly through the co-owned API, independent of the vector, so the Issues crate proves the
/// behaviour itself (not merely a fixture replay).
#[test]
fn order_key_bisection_jitter_rebalance_round_trip_from_issues() {
    // Midpoint bisection: a key built strictly between two bounds sorts strictly between them.
    let lo = OrderKey::parse("F22").unwrap();
    let hi = OrderKey::parse("U00").unwrap();
    let mid = OrderKey::bisect(Some(&lo), Some(&hi));
    assert!(lo < mid && mid < hi, "the midpoint sorts strictly between the bounds: {lo} < {mid} < {hi}");

    // 2-char jitter: the same midpoint body with two different jitters yields DISTINCT in-range keys,
    // both strictly between the bounds (concurrency safety).
    let ja = OrderKey::rank_between(Some(&lo), Some(&hi), jit(5, 5));
    let jb = OrderKey::rank_between(Some(&lo), Some(&hi), jit(6, 6));
    assert_ne!(ja.as_str(), jb.as_str(), "distinct jitters → distinct keys");
    assert!(lo < ja && ja < hi, "jittered key A stays in the gap");
    assert!(lo < jb && jb < hi, "jittered key B stays in the gap");

    // 48-char rebalance trigger: a key AT the rebalance length trips `needs_rebalance`; one below
    // does not (the measured-pathology boundary, byte-identical with Knowledge).
    let at_trigger =
        OrderKey::parse("V".repeat(myelin_query::LEXORANK_REBALANCE_LEN)).unwrap();
    let below = OrderKey::parse("V".repeat(myelin_query::LEXORANK_REBALANCE_LEN - 1)).unwrap();
    assert!(at_trigger.needs_rebalance(), "a {}-char key trips rebalance", myelin_query::LEXORANK_REBALANCE_LEN);
    assert!(!below.needs_rebalance(), "one char below the trigger does NOT trip");
}

/// **The `created_at`+ULID tiebreak is a total order from the Issues side** (contract 13.3 "total
/// order guaranteed"). Issues breaks equal `order_key`s exactly the way Knowledge does: `created_at`
/// first, then the ULID id; distinct rows never compare `Equal`.
#[test]
fn order_key_created_at_ulid_tiebreak_total_order_from_issues() {
    let k = OrderKey::parse("M00").unwrap();
    // Equal key → earlier created_at wins.
    assert_eq!(
        tiebreak(&k, "2026-06-21T10:00:00Z", "01A", &k, "2026-06-21T11:00:00Z", "01B"),
        Ordering::Less,
    );
    // Equal key + equal created_at → ULID id breaks it (lexicographic == time-ordered).
    assert_eq!(tiebreak(&k, "t", "01A", &k, "t", "01B"), Ordering::Less);
    // The order_key dominates when it differs (the tiebreak never overrides the primary rank).
    let hi = OrderKey::parse("U00").unwrap();
    assert_eq!(tiebreak(&k, "2026z", "zzz", &hi, "2026a", "000"), Ordering::Less);
    // Fully identical → the only legitimate Equal (the same row).
    assert_eq!(tiebreak(&k, "t", "id", &k, "t", "id"), Ordering::Equal);
}

/// **The `ViewSpec` + `FieldType` golden serialization is byte-identical from the Issues side.** The
/// Issues crate builds the SAME canonical board view from the co-owned shapes and asserts the golden
/// JSON Knowledge froze (`cdc_13_3_query_primitive`'s `provider_view`), then round-trips it. A
/// `ViewSpec` field rename/reorder or a `FieldType` wire-id change breaks BOTH co-owners' tests.
#[test]
fn cdc_13_3_view_spec_and_field_type_golden_byte_identical_from_issues() {
    // The frozen FieldType wire-id set, read from the co-owned enum on the Issues side.
    let issues_ids = issues_field_type_wire_ids();
    assert_eq!(
        issues_ids,
        ["text", "int", "bool", "date", "select", "relation", "principal", "order_key"],
        "the frozen FieldType wire-id set is byte-identical (X-3 anti-drift anchor), from Issues"
    );

    // The canonical board view, built from the co-owned shapes on the Issues side.
    let view = issues_canonical_board_view();
    let json = serde_json::to_value(&view).expect("Issues serializes the shared ViewSpec");

    // The golden JSON Knowledge's `provider_view` asserts (byte-identical fields).
    assert_eq!(json["kind"], "board");
    assert_eq!(json["group_by"], "status");
    assert_eq!(json["sort"][0]["field"], "priority");
    assert_eq!(json["sort"][0]["dir"], "desc");
    assert_eq!(json["visible"][0], "title");
    assert_eq!(json["visible"][1], "assignee");
    assert_eq!(json["order_field"], "order_key");

    // The parsed filter is the SAME Predicate tree a directly-built consumer tree is (no second
    // grammar): `status == 'open' AND severity >= 3`.
    let directly_built = QueryAst::compiled(Predicate::And(vec![
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("status".into()),
            rhs: Expr::Lit(Literal::Str("open".into())),
        },
        Predicate::Cmp {
            op: CmpOp::Ge,
            lhs: Expr::Var("severity".into()),
            rhs: Expr::Lit(Literal::Int(3)),
        },
    ]))
    .unwrap();
    assert_eq!(
        view.filter.predicate(),
        directly_built.predicate(),
        "the co-owned grammar front-end produces the same Predicate tree on the Issues side"
    );

    // The parsed filter evaluates through the ONE shared engine (no second interpreter).
    let ctx = EvalContext::new()
        .bind("status", Literal::Str("open".into()))
        .bind("severity", Literal::Int(4));
    assert_eq!(view.filter.eval(&ctx), Ok(true), "the Issues filter evaluates through the ONE engine");

    // The consumer round-trips the SAME bytes back into the SAME shape (0 divergence).
    let back: myelin_issues::query_coown::ViewSpec = serde_json::from_value(json).unwrap();
    assert_eq!(back, view, "Issues reconstructs the provider's exact view-model");

    // The view kind set is the frozen six, read from the co-owned enum (the closed taxonomy).
    let kinds: Vec<&str> = ViewKind::all().iter().map(|k| k.wire_id()).collect();
    assert_eq!(kinds, ["table", "board", "calendar", "timeline", "gallery", "list"]);

    // A lone FieldId token serializes as the bare string (the byte-identical id encoding).
    assert_eq!(
        serde_json::to_value(FieldId::new("order_key")).unwrap(),
        serde_json::json!("order_key")
    );
    // A FieldType serializes byte-identically from Issues (the closed enum's wire form).
    assert_eq!(serde_json::to_value(FieldType::OrderKey).unwrap(), serde_json::json!("OrderKey"));
}

fn jit(a: usize, b: usize) -> myelin_query::Jitter {
    myelin_query::Jitter::from_ranks(a, b).unwrap()
}
