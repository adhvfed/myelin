use std::collections::BTreeMap;

use myelin_identity::{Literal, Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_knowledge::{
    db_row_id_colref, execute_view_count, execute_view_query, lower_view_filter, FacetTelemetry,
    FieldDef, FieldSchema, PageBound, PropertyBag, FACET_PROMOTION_THRESHOLD,
};
use myelin_query::{
    CmpOp, Expr, FieldId, FieldType, FieldValue, Predicate, QueryAst, SortDir, SortSpec, ViewKind,
    ViewSpec,
};
use myelin_substrate::Thresholds;
use myelin_tenancy::TenantId;

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:v".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

#[test]
fn cdc_13_3_executor_consumes_the_frozen_shapes() {
    let view = ViewSpec {
        kind: ViewKind::Table,
        filter: QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("status".into()),
            rhs: Expr::Lit(Literal::Str("open".into())),
        })
        .unwrap(),
        group_by: None,
        sort: vec![SortSpec {
            field: FieldId::new("priority"),
            dir: SortDir::Desc,
        }],
        visible: vec![FieldId::new("title")],
        order_field: FieldId::new("order_key"),
    };
    let lowered = lower_view_filter(&view.filter, &[]).expect("the frozen filter lowers");
    assert!(
        lowered.sql_predicate.contains("db_row.props ->> 'status'"),
        "the frozen FieldId facet is read: {}",
        lowered.sql_predicate
    );
    assert_eq!(
        lowered.params.len(),
        1,
        "the frozen Literal is bound, not interpolated"
    );

    let q = execute_view_query(
        &view,
        &SetExpr::All,
        &viewer(),
        &TenantId("acme".into()),
        "db:p",
        &[],
        PageBound::DEFAULT,
    )
    .unwrap();
    assert!(
        q.sql.contains("db_row.props ->> 'priority' DESC"),
        "the frozen ViewSpec sort is applied: {}",
        q.sql
    );
    assert!(
        q.sql.contains("db_row.order_key ASC"),
        "the frozen order_field is the last-resort tiebreak: {}",
        q.sql
    );
}

#[test]
fn cdc_4_3_executor_conjoins_the_setexpr_acl() {
    let view = ViewSpec {
        kind: ViewKind::Table,
        filter: QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("status".into()),
            rhs: Expr::Lit(Literal::Str("open".into())),
        })
        .unwrap(),
        group_by: None,
        sort: vec![],
        visible: vec![],
        order_field: FieldId::new("order_key"),
    };
    let acl = SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: db_row_id_colref(),
    };
    let q = execute_view_query(
        &view,
        &acl,
        &viewer(),
        &TenantId("acme".into()),
        "db:p",
        &[],
        PageBound::DEFAULT,
    )
    .unwrap();
    assert_eq!(
        q.statement_count(),
        1,
        "one query (no N+1, no post-filter): {}",
        q.sql
    );
    let where_part = q.sql.split(" ORDER BY ").next().unwrap();
    assert!(
        where_part.contains("JOIN authz_visible"),
        "the ACL JOIN is conjoined: {}",
        q.sql
    );
    assert!(
        where_part.contains("db_row.props ->> 'status'"),
        "the view filter is conjoined in the SAME WHERE: {}",
        q.sql
    );

    let count = execute_view_count(
        &view,
        &acl,
        &viewer(),
        &TenantId("acme".into()),
        "db:p",
        &[],
    )
    .unwrap();
    assert!(
        count.sql.starts_with("SELECT COUNT(*)") && count.sql.contains("JOIN authz_visible"),
        "the ACL is inside the COUNT: {}",
        count.sql
    );
}

#[test]
fn cdc_6_3_facet_promotion_threshold_matches_file_and_is_strict() {
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    assert_eq!(
        thresholds.flex_db.facet_promotion_ratio, FACET_PROMOTION_THRESHOLD,
        "the executor's facet-promotion threshold == the thresholds-file value (one source of truth)"
    );
    assert_eq!(
        FACET_PROMOTION_THRESHOLD, 0.05,
        "the frozen 6.3 trigger is >5%"
    );

    let schema = FieldSchema::of([
        FieldDef::new("hot", FieldType::Select),
        FieldDef::new("edge", FieldType::Int),
    ])
    .unwrap();
    let tel = FacetTelemetry::new();
    for n in 0..20u32 {
        let mut facets = Vec::new();
        if n < 2 {
            facets.push(FieldId::new("hot"));
        }
        if n < 1 {
            facets.push(FieldId::new("edge"));
        }
        tel.record_execution("db:x", &facets);
    }
    let candidates: Vec<String> = tel
        .promotion_candidates("db:x", &schema)
        .into_iter()
        .map(|h| h.field_id.to_string())
        .collect();
    assert_eq!(
        candidates,
        vec!["hot".to_string()],
        "hot (10%) promotes; edge (exactly 5%) does NOT (strict >5%): {candidates:?}"
    );
}

#[test]
fn cdc_13_3_typed_field_validation_rejects_mismatch() {
    let schema = FieldSchema::of([FieldDef::new("priority", FieldType::Int)]).unwrap();
    let mut bad: PropertyBag = BTreeMap::new();
    bad.insert(FieldId::new("priority"), FieldValue::Text("high".into()));
    assert!(
        schema.validate_props(&bad).is_err(),
        "a Text in an Int field is rejected (no coercion)"
    );
    let mut good: PropertyBag = BTreeMap::new();
    good.insert(FieldId::new("priority"), FieldValue::Int(3));
    assert_eq!(schema.validate_props(&good), Ok(()));
}
