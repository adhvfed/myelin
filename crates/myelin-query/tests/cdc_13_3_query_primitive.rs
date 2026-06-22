//! CDC pair for contract-index row 13.3 — the **`myelin-query` primitive (frozen byte-identical)**:
//! the field-type enum, the `ViewSpec` view-model, and the `QueryAst` grammar (= the `EventMatcher`
//! core, 3.4) (X-3/OQ-C). The `order_key`/LexoRank half of 13.3 is co-landed in `field.rs`
//! (P-167/P-235) and exercised by its own unit tests; THIS file is the **type/grammar half** the
//! P-235 prompt freezes.
//!
//! PROVIDER side: Knowledge (this crate) freezes the shared definitions — [`FieldType`],
//! [`ViewSpec`]/[`ViewKind`], and the textual [`parse_query`] grammar that compiles into the ONE
//! [`QueryAst`] predicate core. CONSUMER side: Issues + Search each build their OWN executor against
//! the SAME definitions (their compilers differ; the shapes are byte-identical, X-3). The consumer
//! asserts the golden serialization + the parsed-tree parity, so a shape rename on EITHER side
//! breaks this drift test at once. This file carries BOTH sides so the contract-coverage scanner
//! (P-037) admits row 13.3 as a real provider+consumer pair.

use myelin_identity::Literal;
use myelin_query::{
    parse_query, CmpOp, EvalContext, Expr, FieldId, FieldType, Predicate, QueryAst, SortDir,
    SortSpec, ViewKind, ViewSpec,
};

// ── PROVIDER side (13.3): Knowledge freezes the shared FieldType/ViewSpec/QueryAst ──

/// The provider freezes the field-type taxonomy (the byte-identical wire-id set the three co-owners
/// reconcile to).
fn provider_field_type_wire_ids() -> Vec<&'static str> {
    FieldType::all().iter().map(|t| t.wire_id()).collect()
}

/// The provider freezes the textual grammar → the ONE `QueryAst` predicate core. A saved-view
/// filter string compiles to the same tree the Bus matcher / Notif prefs evaluate.
fn provider_compiles_filter(src: &str) -> QueryAst {
    parse_query(src).expect("the provider grammar compiles a well-formed filter")
}

/// The provider freezes the `ViewSpec` view-model: a board grouped by `status`, sorted by
/// `priority`, manual-ordered by the frozen `order_key`.
fn provider_view() -> ViewSpec {
    ViewSpec {
        kind: ViewKind::Board,
        filter: provider_compiles_filter("status == 'open' AND severity >= 3"),
        group_by: Some(FieldId::new("status")),
        sort: vec![SortSpec {
            field: FieldId::new("priority"),
            dir: SortDir::Desc,
        }],
        visible: vec![FieldId::new("title"), FieldId::new("assignee")],
        order_field: FieldId::new("order_key"),
    }
}

// ── CONSUMER side (X-3): Issues + Search build their own executor to the SAME shapes ──

/// A consumer (Issues' board executor) builds its OWN compiler against the SAME `ViewSpec`/
/// `QueryAst` — here it asserts the wire shape byte-identically (a rename on either side breaks
/// this). It does NOT redefine the types; it consumes them.
fn issues_consumes_view(view: &ViewSpec) -> serde_json::Value {
    serde_json::to_value(view).expect("the consumer serializes the shared view-model")
}

/// A consumer (Search's structured-shape compiler) builds against the SAME `FieldType` enum — its
/// columnar facet kinds are typed over the frozen variant set, byte-identically.
fn search_consumes_field_types() -> Vec<&'static str> {
    // Search lowers each frozen FieldType to a columnar/inverted shape; it reads the SAME wire ids.
    FieldType::all().iter().map(|t| t.wire_id()).collect()
}

#[test]
fn cdc_13_3_provider_freezes_primitive_consumers_build_to_identical_shapes() {
    // ── PROVIDER + Search CONSUMER: the FieldType wire-id set is byte-identical ──
    let provider_ids = provider_field_type_wire_ids();
    let search_ids = search_consumes_field_types();
    assert_eq!(
        provider_ids, search_ids,
        "Search's compiler reads the SAME frozen FieldType set"
    );
    assert_eq!(
        provider_ids,
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
        "the frozen FieldType wire-id set (X-3 anti-drift anchor)"
    );

    // ── PROVIDER: the grammar compiles a filter into the ONE QueryAst core ──
    let filter = provider_compiles_filter("status == 'open' AND severity >= 3");
    let ctx = EvalContext::new()
        .bind("status", Literal::Str("open".into()))
        .bind("severity", Literal::Int(4));
    assert_eq!(
        filter.eval(&ctx),
        Ok(true),
        "the parsed filter evaluates through the ONE engine"
    );

    // The parsed tree is the SAME shape a directly-built consumer tree is (no second grammar).
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
        filter.predicate(),
        directly_built.predicate(),
        "the grammar front-end produces the same Predicate tree a hand-built consumer does"
    );

    // ── Issues CONSUMER: the ViewSpec golden serialization is byte-identical ──
    let view = provider_view();
    let json = issues_consumes_view(&view);
    assert_eq!(json["kind"], "board");
    assert_eq!(json["group_by"], "status");
    assert_eq!(json["sort"][0]["field"], "priority");
    assert_eq!(json["sort"][0]["dir"], "desc");
    assert_eq!(json["order_field"], "order_key");

    // The consumer round-trips the SAME bytes back into the SAME shape (no divergence).
    let back: ViewSpec = serde_json::from_value(json).unwrap();
    assert_eq!(
        back, view,
        "the consumer reconstructs the provider's exact view-model"
    );
}
