use myelin_identity::Literal;
use myelin_issues::query_coown::{
    issues_canonical_board_view, issues_field_type_wire_ids, issues_replay_conformance_vector,
    tiebreak, CmpOp, EvalContext, Expr, FieldId, FieldType, OrderKey, Predicate, QueryAst,
    ViewKind, CONFORMANCE_VECTOR,
};
use std::cmp::Ordering;

#[test]
fn cdc_13_3_order_key_conformance_vector_byte_identical_from_issues() {
    let issues_keys = issues_replay_conformance_vector();
    let knowledge_expected: Vec<&str> = CONFORMANCE_VECTOR.iter().map(|s| s.expect).collect();

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
    assert_eq!(
        issues_keys.len(),
        knowledge_expected.len(),
        "the co-owners replay the same vector"
    );
    assert_eq!(
        byte_diffs, 0,
        "the order_key codec is byte-identical across the co-owners (0 byte differences, X-3)"
    );

    let key_of = |prefix: &str| -> String {
        let idx = CONFORMANCE_VECTOR
            .iter()
            .position(|s| s.label.starts_with(prefix))
            .expect("the vector carries the concurrent same-gap legs");
        issues_keys[idx].clone()
    };
    let a = key_of("concurrent same-gap insert A");
    let b = key_of("concurrent same-gap insert B");
    assert_ne!(
        a, b,
        "concurrent same-gap inserts produce DISTINCT keys (the jitter, from Issues)"
    );
}

#[test]
fn order_key_bisection_jitter_rebalance_round_trip_from_issues() {
    let lo = OrderKey::parse("F22").unwrap();
    let hi = OrderKey::parse("U00").unwrap();
    let mid = OrderKey::bisect(Some(&lo), Some(&hi));
    assert!(
        lo < mid && mid < hi,
        "the midpoint sorts strictly between the bounds: {lo} < {mid} < {hi}"
    );

    let ja = OrderKey::rank_between(Some(&lo), Some(&hi), jit(5, 5));
    let jb = OrderKey::rank_between(Some(&lo), Some(&hi), jit(6, 6));
    assert_ne!(ja.as_str(), jb.as_str(), "distinct jitters → distinct keys");
    assert!(lo < ja && ja < hi, "jittered key A stays in the gap");
    assert!(lo < jb && jb < hi, "jittered key B stays in the gap");

    let at_trigger = OrderKey::parse("V".repeat(myelin_query::LEXORANK_REBALANCE_LEN)).unwrap();
    let below = OrderKey::parse("V".repeat(myelin_query::LEXORANK_REBALANCE_LEN - 1)).unwrap();
    assert!(
        at_trigger.needs_rebalance(),
        "a {}-char key trips rebalance",
        myelin_query::LEXORANK_REBALANCE_LEN
    );
    assert!(
        !below.needs_rebalance(),
        "one char below the trigger does NOT trip"
    );
}

#[test]
fn order_key_created_at_ulid_tiebreak_total_order_from_issues() {
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
    );
    assert_eq!(tiebreak(&k, "t", "01A", &k, "t", "01B"), Ordering::Less);
    let hi = OrderKey::parse("U00").unwrap();
    assert_eq!(
        tiebreak(&k, "2026z", "zzz", &hi, "2026a", "000"),
        Ordering::Less
    );
    assert_eq!(tiebreak(&k, "t", "id", &k, "t", "id"), Ordering::Equal);
}

#[test]
fn cdc_13_3_view_spec_and_field_type_golden_byte_identical_from_issues() {
    let issues_ids = issues_field_type_wire_ids();
    assert_eq!(
        issues_ids,
        [
            "text",
            "int",
            "bool",
            "date",
            "select",
            "relation",
            "principal",
            "order_key"
        ],
        "the frozen FieldType wire-id set is byte-identical (X-3 anti-drift anchor), from Issues"
    );

    let view = issues_canonical_board_view();
    let json = serde_json::to_value(&view).expect("Issues serializes the shared ViewSpec");

    assert_eq!(json["kind"], "board");
    assert_eq!(json["group_by"], "status");
    assert_eq!(json["sort"][0]["field"], "priority");
    assert_eq!(json["sort"][0]["dir"], "desc");
    assert_eq!(json["visible"][0], "title");
    assert_eq!(json["visible"][1], "assignee");
    assert_eq!(json["order_field"], "order_key");

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

    let ctx = EvalContext::new()
        .bind("status", Literal::Str("open".into()))
        .bind("severity", Literal::Int(4));
    assert_eq!(
        view.filter.eval(&ctx),
        Ok(true),
        "the Issues filter evaluates through the ONE engine"
    );

    let back: myelin_issues::query_coown::ViewSpec = serde_json::from_value(json).unwrap();
    assert_eq!(
        back, view,
        "Issues reconstructs the provider's exact view-model"
    );

    let kinds: Vec<&str> = ViewKind::all().iter().map(|k| k.wire_id()).collect();
    assert_eq!(
        kinds,
        ["table", "board", "calendar", "timeline", "gallery", "list"]
    );

    assert_eq!(
        serde_json::to_value(FieldId::new("order_key")).unwrap(),
        serde_json::json!("order_key")
    );
    assert_eq!(
        serde_json::to_value(FieldType::OrderKey).unwrap(),
        serde_json::json!("OrderKey")
    );
}

fn jit(a: usize, b: usize) -> myelin_query::Jitter {
    myelin_query::Jitter::from_ranks(a, b).unwrap()
}
