use myelin_identity::Literal;
use myelin_query::{
    parse_query, CmpOp, EvalContext, Expr, FieldId, FieldType, Predicate, QueryAst, SortDir,
    SortSpec, ViewKind, ViewSpec,
};

fn provider_field_type_wire_ids() -> Vec<&'static str> {
    FieldType::all().iter().map(|t| t.wire_id()).collect()
}

fn provider_compiles_filter(src: &str) -> QueryAst {
    parse_query(src).expect("the provider grammar compiles a well-formed filter")
}

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

fn issues_consumes_view(view: &ViewSpec) -> serde_json::Value {
    serde_json::to_value(view).expect("the consumer serializes the shared view-model")
}

fn search_consumes_field_types() -> Vec<&'static str> {
    FieldType::all().iter().map(|t| t.wire_id()).collect()
}

#[test]
fn cdc_13_3_provider_freezes_primitive_consumers_build_to_identical_shapes() {
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

    let filter = provider_compiles_filter("status == 'open' AND severity >= 3");
    let ctx = EvalContext::new()
        .bind("status", Literal::Str("open".into()))
        .bind("severity", Literal::Int(4));
    assert_eq!(
        filter.eval(&ctx),
        Ok(true),
        "the parsed filter evaluates through the ONE engine"
    );

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

    let view = provider_view();
    let json = issues_consumes_view(&view);
    assert_eq!(json["kind"], "board");
    assert_eq!(json["group_by"], "status");
    assert_eq!(json["sort"][0]["field"], "priority");
    assert_eq!(json["sort"][0]["dir"], "desc");
    assert_eq!(json["order_field"], "order_key");

    let back: ViewSpec = serde_json::from_value(json).unwrap();
    assert_eq!(
        back, view,
        "the consumer reconstructs the provider's exact view-model"
    );
}
