use super::*;
use crate::db_row_id_colref;
use myelin_identity::{Literal, ObjectId, PrincipalId, PrincipalKind, RelName};
use myelin_query::{CmpOp, Expr, FieldId, SortDir, SortSpec, ViewKind};
use std::collections::BTreeMap;

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}
fn order_key() -> OrderKey {
    OrderKey::parse("U").unwrap()
}
fn var(name: &str) -> Expr {
    Expr::Var(name.into())
}
fn s(v: &str) -> Expr {
    Expr::Lit(Literal::Str(v.into()))
}
fn i(n: i64) -> Expr {
    Expr::Lit(Literal::Int(n))
}

fn status_open_view() -> ViewSpec {
    ViewSpec {
        kind: ViewKind::Table,
        filter: QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("status"),
            rhs: s("open"),
        })
        .unwrap(),
        group_by: None,
        sort: vec![SortSpec {
            field: FieldId::new("priority"),
            dir: SortDir::Desc,
        }],
        visible: vec![FieldId::new("title")],
        order_field: FieldId::new("order_key"),
    }
}

#[test]
fn typed_field_validation_rejects_type_mismatch() {
    let schema = FieldSchema::of([
        FieldDef::new("status", FieldType::Select),
        FieldDef::new("priority", FieldType::Int),
        FieldDef::personal("assignee", FieldType::Principal),
    ])
    .unwrap();

    let mut props: PropertyBag = BTreeMap::new();
    props.insert(FieldId::new("status"), FieldValue::Select("open".into()));
    props.insert(FieldId::new("priority"), FieldValue::Int(3));
    assert_eq!(
        schema.validate_props(&props),
        Ok(()),
        "a well-typed sparse row validates"
    );

    let mut bad: PropertyBag = BTreeMap::new();
    bad.insert(FieldId::new("status"), FieldValue::Int(1));
    let err = schema.validate_props(&bad).unwrap_err();
    assert_eq!(
        err,
        SchemaError::TypeMismatch {
            field: "status".into(),
            declared: FieldType::Select,
            supplied: FieldType::Int,
        },
        "an Int in a Select field is a TypeMismatch, never coerced"
    );

    let mut unknown: PropertyBag = BTreeMap::new();
    unknown.insert(FieldId::new("ghost"), FieldValue::Text("x".into()));
    assert_eq!(
        schema.validate_props(&unknown),
        Err(SchemaError::UnknownField("ghost".into()))
    );
}

#[test]
fn duplicate_field_id_is_a_schema_error() {
    let err = FieldSchema::of([
        FieldDef::new("status", FieldType::Select),
        FieldDef::new("status", FieldType::Text),
    ])
    .unwrap_err();
    assert_eq!(err, SchemaError::DuplicateField("status".into()));
}

#[test]
fn personal_data_flag_rides_on_the_field_def() {
    let schema = FieldSchema::of([
        FieldDef::personal("ssn", FieldType::Text),
        FieldDef::new("title", FieldType::Text),
    ])
    .unwrap();
    assert!(
        schema
            .fields()
            .iter()
            .find(|d| d.field_id.as_str() == "ssn")
            .unwrap()
            .personal_data
    );
    assert!(
        !schema
            .fields()
            .iter()
            .find(|d| d.field_id.as_str() == "title")
            .unwrap()
            .personal_data
    );
}

#[test]
fn props_json_is_byte_stable_and_covers_every_field_type() {
    let mut props: PropertyBag = BTreeMap::new();
    props.insert(FieldId::new("title"), FieldValue::Text("Spec".into()));
    props.insert(FieldId::new("count"), FieldValue::Int(7));
    props.insert(FieldId::new("done"), FieldValue::Bool(true));
    props.insert(FieldId::new("due"), FieldValue::Date("2026-06-22".into()));
    props.insert(FieldId::new("status"), FieldValue::Select("open".into()));
    let row = DbRow::new("row:1", props, order_key());
    let json = row.props_json();
    assert_eq!(json["title"], serde_json::json!("Spec"));
    assert_eq!(json["count"], serde_json::json!(7));
    assert_eq!(json["done"], serde_json::json!(true));
    assert_eq!(json["due"], serde_json::json!("2026-06-22"));
    assert_eq!(json["status"], serde_json::json!("open"));
    assert_eq!(
        serde_json::to_string(&row.props_json()).unwrap(),
        serde_json::to_string(&row.props_json()).unwrap()
    );
}

#[test]
fn cold_facet_gin_scan_hot_facet_generated_column() {
    let view = status_open_view();
    let cold = lower_view_filter(&view.filter, &[]).unwrap();
    assert!(
        cold.sql_predicate.contains("db_row.props ->> 'status'"),
        "cold facet → GIN props scan: {}",
        cold.sql_predicate
    );
    assert_eq!(
        cold.facet_paths.get(&FieldId::new("status")),
        Some(&FacetPath::GinScan)
    );
    assert!(
        !cold.sql_predicate.contains("'open'"),
        "the filter literal is bound, never interpolated: {}",
        cold.sql_predicate
    );
    assert_eq!(cold.params.len(), 1);
    assert_eq!(cold.params[0].value, "open");

    let hot = lower_view_filter(&view.filter, &[FieldId::new("status")]).unwrap();
    assert!(
        hot.sql_predicate.contains("db_row.status__col"),
        "hot facet → generated column: {}",
        hot.sql_predicate
    );
    assert_eq!(
        hot.facet_paths.get(&FieldId::new("status")),
        Some(&FacetPath::GeneratedColumn)
    );
}

#[test]
fn boolean_connectives_and_orderings_lower() {
    let filter = QueryAst::compiled(Predicate::And(vec![
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("status"),
            rhs: s("open"),
        },
        Predicate::Or(vec![
            Predicate::Cmp {
                op: CmpOp::Ge,
                lhs: var("priority"),
                rhs: i(3),
            },
            Predicate::Not(Box::new(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("archived"),
                rhs: s("true"),
            })),
        ]),
    ]))
    .unwrap();
    let lowered = lower_view_filter(&filter, &[]).unwrap();
    assert!(lowered.sql_predicate.contains(" AND "));
    assert!(lowered.sql_predicate.contains(" OR "));
    assert!(lowered.sql_predicate.contains("NOT "));
    assert!(lowered.sql_predicate.contains(">="));
    assert_eq!(
        lowered.params.len(),
        3,
        "three bound literals (open, 3, true)"
    );
}

#[test]
fn unparsed_filter_is_refused_not_match_all() {
    assert!(lower_view_filter(&QueryAst::raw("status == 'open'"), &[]).is_none());
    let view = ViewSpec {
        kind: ViewKind::Table,
        filter: QueryAst::raw("status == 'open'"),
        group_by: None,
        sort: vec![],
        visible: vec![],
        order_field: FieldId::new("order_key"),
    };
    let err = execute_view_query(
        &view,
        &SetExpr::All,
        &viewer(),
        &TenantId("acme".into()),
        "db:projects",
        &[],
        PageBound::DEFAULT,
    )
    .unwrap_err();
    assert_eq!(err, ViewError::FilterNotCompiled);
}

#[test]
fn view_query_conjoins_filter_and_acl_one_query_pre_filter() {
    let view = status_open_view();
    let acl = SetExpr::Ids(vec![ObjectId("row:1".into()), ObjectId("row:2".into())]);
    let q = execute_view_query(
        &view,
        &acl,
        &viewer(),
        &TenantId("acme".into()),
        "db:projects",
        &[],
        PageBound::DEFAULT,
    )
    .unwrap();

    assert_eq!(
        q.statement_count(),
        1,
        "the VIEW_QUERY is one statement: {}",
        q.sql
    );
    assert!(q.sql.contains("db_row.tenant = :tenant"));
    assert!(q.sql.contains("db_row.db_id = :db_id"));
    assert!(
        q.sql.contains("db_row.id IN ("),
        "the ACL Ids set is conjoined: {}",
        q.sql
    );
    assert!(
        q.sql.contains("db_row.props ->> 'status'"),
        "the view filter is conjoined: {}",
        q.sql
    );
    let where_part = q.sql.split(" ORDER BY ").next().unwrap();
    assert!(
        where_part.contains("db_row.props ->> 'status'") && where_part.contains("db_row.id IN ("),
        "both predicates sit in the WHERE, before ORDER BY: {}",
        q.sql
    );
    assert!(
        q.sql.contains("db_row.order_key ASC"),
        "the LexoRank tiebreak is the last-resort sort: {}",
        q.sql
    );
    assert!(q.sql.ends_with("LIMIT 50"), "row-capped: {}", q.sql);
    assert!(
        q.sql.contains("db_row.props ->> 'priority' DESC"),
        "the view sort is applied: {}",
        q.sql
    );
}

#[test]
fn none_acl_is_deny_false() {
    let view = status_open_view();
    let q = execute_view_query(
        &view,
        &SetExpr::None,
        &viewer(),
        &TenantId("acme".into()),
        "db:projects",
        &[],
        PageBound::DEFAULT,
    )
    .unwrap();
    assert!(
        q.sql.contains("(FALSE)"),
        "None lowers to a deny conjunct: {}",
        q.sql
    );
}

#[test]
fn view_count_conjoins_acl_inside_the_aggregate() {
    let view = status_open_view();
    let acl = SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: db_row_id_colref(),
    };
    let q = execute_view_count(
        &view,
        &acl,
        &viewer(),
        &TenantId("acme".into()),
        "db:projects",
        &[],
    )
    .unwrap();
    assert!(q.is_count);
    assert_eq!(q.statement_count(), 1);
    assert!(
        q.sql.starts_with("SELECT COUNT(*)"),
        "a COUNT aggregate: {}",
        q.sql
    );
    assert!(
        q.sql.contains("JOIN authz_visible"),
        "the ACL reverse-index JOIN is inside the COUNT: {}",
        q.sql
    );
    assert!(
        q.sql.contains("db_row.props ->> 'status'"),
        "the view filter is inside the COUNT: {}",
        q.sql
    );
    assert!(
        !q.sql.contains("ORDER BY"),
        "a COUNT has no ORDER BY: {}",
        q.sql
    );
}

#[test]
fn page_bound_is_always_row_capped() {
    assert_eq!(
        PageBound::new(0, 1000).limit,
        1,
        "a 0-row request is clamped to 1"
    );
    assert_eq!(
        PageBound::new(10_000, 1000).limit,
        PageBound::MAX,
        "an over-large request is clamped to MAX"
    );
    assert_eq!(PageBound::DEFAULT.limit, 50);
    assert_eq!(PageBound::default().statement_timeout_ms, 5_000);
}

fn relation(id: &str, src: &str, dst: &str, rel: RelationKind) -> DbRelation {
    DbRelation {
        relation_id: id.into(),
        src_row: src.into(),
        dst_ref: ArtifactRef(dst.into()),
        rel,
    }
}

#[test]
fn two_way_relation_maintenance_is_idempotent_and_emits() {
    let store = RelationStore::new();
    let acme = TenantId("acme".into());
    let r = relation("rel:1", "row:1", "knw:row:9", RelationKind::Relates);

    assert!(store.relate(&acme, r.clone()), "a new edge is created");
    assert!(
        !store.relate(&acme, r.clone()),
        "relating the same pair again is a no-op"
    );
    assert_eq!(
        store
            .relations_from(&acme, "row:1", RelationKind::Relates)
            .len(),
        1,
        "exactly one edge"
    );

    let events = store.drain_edge_events();
    assert_eq!(events.len(), 1);
    assert!(events[0].created);
    assert_eq!(events[0].relation.src_row, "row:1");
}

#[test]
fn unrelate_removes_the_edge_and_emits_removed() {
    let store = RelationStore::new();
    let acme = TenantId("acme".into());
    let dst = ArtifactRef("knw:row:9".into());
    store.relate(
        &acme,
        relation("rel:1", "row:1", "knw:row:9", RelationKind::Relates),
    );
    store.drain_edge_events();

    assert!(
        store.unrelate(&acme, "row:1", &dst, RelationKind::Relates),
        "the edge is removed"
    );
    assert!(store
        .relations_from(&acme, "row:1", RelationKind::Relates)
        .is_empty());
    assert!(
        !store.unrelate(&acme, "row:1", &dst, RelationKind::Relates),
        "no edge left to remove"
    );

    let events = store.drain_edge_events();
    assert_eq!(events.len(), 1);
    assert!(!events[0].created, "the recorded event is an edge-removed");
}

#[test]
fn relations_are_tenant_scoped() {
    let store = RelationStore::new();
    let acme = TenantId("acme".into());
    let evil = TenantId("evilcorp".into());
    store.relate(
        &acme,
        relation("rel:1", "row:1", "knw:row:9", RelationKind::Relates),
    );
    assert!(store
        .relations_from(&evil, "row:1", RelationKind::Relates)
        .is_empty());
    assert_eq!(
        store
            .relations_from(&acme, "row:1", RelationKind::Relates)
            .len(),
        1
    );
}

#[test]
fn rollup_source_relation_is_distinct_from_relates() {
    let store = RelationStore::new();
    let acme = TenantId("acme".into());
    store.relate(
        &acme,
        relation("rel:1", "row:1", "knw:row:9", RelationKind::Relates),
    );
    store.relate(
        &acme,
        relation("rel:2", "row:1", "knw:row:9", RelationKind::RollupSource),
    );
    assert_eq!(
        store
            .relations_from(&acme, "row:1", RelationKind::Relates)
            .len(),
        1
    );
    assert_eq!(
        store
            .relations_from(&acme, "row:1", RelationKind::RollupSource)
            .len(),
        1
    );
    assert_eq!(RelationKind::RollupSource.wire_id(), "rollup_source");
    assert_eq!(RelationKind::Relates.wire_id(), "relates");
}

#[test]
fn facet_promotion_trigger_is_strictly_above_5_percent() {
    let schema = FieldSchema::of([
        FieldDef::new("status", FieldType::Select),
        FieldDef::personal("assignee", FieldType::Principal),
        FieldDef::new("cold", FieldType::Text),
    ])
    .unwrap();
    let tel = FacetTelemetry::new();
    let db = "db:projects";

    for n in 0..100u32 {
        let mut facets = Vec::new();
        if n < 20 {
            facets.push(FieldId::new("status"));
        }
        if n < 6 {
            facets.push(FieldId::new("assignee"));
        }
        if n < 5 {
            facets.push(FieldId::new("cold"));
        }
        tel.record_execution(db, &facets);
    }

    assert!((tel.facet_frequency(db, &FieldId::new("status")) - 0.20).abs() < 1e-9);
    assert!((tel.facet_frequency(db, &FieldId::new("cold")) - 0.05).abs() < 1e-9);

    let mut candidates: Vec<String> = tel
        .promotion_candidates(db, &schema)
        .into_iter()
        .map(|h| h.field_id.to_string())
        .collect();
    candidates.sort();
    assert_eq!(
        candidates,
        vec!["assignee".to_string(), "status".to_string()],
        "status (20%) + assignee (6%) promote; cold (exactly 5%) does NOT (strict >5%)"
    );

    let assignee_hint = tel
        .promotion_candidates(db, &schema)
        .into_iter()
        .find(|h| h.field_id.as_str() == "assignee")
        .unwrap();
    assert_eq!(assignee_hint.field_type, FieldType::Principal);
    assert!(
        assignee_hint.personal_data,
        "the PII flag rides into the promotion hint (KN-P31 gates it)"
    );

    assert_eq!(FACET_PROMOTION_THRESHOLD, 0.05);
}

#[test]
fn empty_telemetry_has_no_candidates() {
    let schema = FieldSchema::of([FieldDef::new("status", FieldType::Select)]).unwrap();
    let tel = FacetTelemetry::new();
    assert_eq!(
        tel.facet_frequency("db:empty", &FieldId::new("status")),
        0.0
    );
    assert!(tel.promotion_candidates("db:empty", &schema).is_empty());
}

#[test]
fn a_facet_counts_once_per_execution() {
    let tel = FacetTelemetry::new();
    let db = "db:x";
    tel.record_execution(db, &[FieldId::new("status")]);
    tel.record_execution(db, &[FieldId::new("status"), FieldId::new("status")]);
    assert!(
        (tel.facet_frequency(db, &FieldId::new("status")) - 1.0).abs() < 1e-9,
        "status touched in 2/2 executions"
    );
}

#[test]
fn view_query_reports_facet_paths_for_telemetry() {
    let view = status_open_view();
    let tel = FacetTelemetry::new();
    let q = execute_view_query(
        &view,
        &SetExpr::All,
        &viewer(),
        &TenantId("acme".into()),
        "db:projects",
        &[],
        PageBound::DEFAULT,
    )
    .unwrap();
    let facets: Vec<FieldId> = q.facet_paths.keys().cloned().collect();
    assert_eq!(facets, vec![FieldId::new("status")]);
    tel.record_execution("db:projects", &facets);
    assert!((tel.facet_frequency("db:projects", &FieldId::new("status")) - 1.0).abs() < 1e-9);
}
