//! # CDC — contract 13.3 (the flexible-DB EXECUTOR over the frozen shapes) + 6.3 (the >5%
//! facet-promotion threshold, measured) — KNOWLEDGE side (KN-P17 / P-307, M3)
//!
//! **Contract 13.3** (the `FieldType` enum + the `ViewSpec` view-model + the `QueryAst` grammar —
//! the frozen SHARED definitions live in `myelin-query`; **Knowledge owns its EXECUTOR**: the JSONB
//! query lowering over `db_row.props` + the read-time view/COUNT path). This CDC pins the AGREEMENT:
//! Knowledge's executor consumes the frozen [`ViewSpec`]/[`QueryAst`]/[`FieldType`] BYTE-IDENTICALLY
//! (it never re-defines them) and lowers a view into a permission-correct JSONB query.
//!
//! **Contract 4.3** (the `list_objects` `SetExpr` push-down) is CONSUMED here — the view executor
//! ALWAYS conjoins the ACL `SetExpr` into the query (the leak-free view-permission gate). The
//! producer/consumer agreement on 4.3 is pinned by `cdc_4_3_knowledge_list_pushdown.rs`; this CDC
//! asserts the EXECUTOR conjoins it (0 post-filter).
//!
//! **Contract 6.3** (the `> 5%` facet-promotion threshold — the Search-owned tunable): MEASURED here
//! ([`FacetTelemetry`]) and ACTED on in KN-P31 (M5). This CDC pins that (a) the threshold the
//! executor reads matches the value in the thresholds file (one source of truth), and (b) the
//! measured trigger fires strictly above the ratio (a facet at exactly the ratio does NOT promote —
//! the frozen wording, never weakened).
//!
//! ## The CDC pair (provider + consumer)
//! - **PROVIDER** — `myelin-query` (co-owned with Issues + Search) PROVIDES the frozen 13.3 shapes
//!   (`FieldType`/`ViewSpec`/`QueryAst`) + `myelin-substrate` PROVIDES the frozen 6.3
//!   `flex_db.facet_promotion_ratio` threshold value. These CDC tests assert the provider's shapes +
//!   threshold value are consumed UNCHANGED (a provider-side rename / threshold drift fails here).
//! - **CONSUMER** — Knowledge CONSUMES them: its flexible-DB executor (`myelin_knowledge::database`)
//!   lowers the provider's `ViewSpec`/`QueryAst` into a JSONB query and reads the provider's
//!   threshold value (one source of truth). The consumer never re-defines the provider's shapes.
//!
//! Deterministic + DB-free (the contract-agreement). The live p99-at-scale + the one-query/0-leak
//! proof against the dev-stack Postgres is the `--features integration` test
//! (`integration_kn_d9_flex_db.rs`) + the KN-D9 SCHED drill (`drill_kn_d9_flex_db.rs`).

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

/// **13.3 — the executor consumes the frozen `ViewSpec`/`QueryAst`/`FieldType` byte-identically.** A
/// view built from the frozen shared shapes lowers into a JSONB query over `db_row.props`; the
/// executor NEVER re-defines a second view-model / field-type / predicate grammar (the one-primitive
/// invariant, EI-01 §7). The lowered query references the frozen `FieldType`-typed facets by their
/// `FieldId` and the `ViewSpec`'s sort/order_field.
#[test]
fn cdc_13_3_executor_consumes_the_frozen_shapes() {
    // The view is the FROZEN ViewSpec over the FROZEN QueryAst — the SAME types Issues co-owns.
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
    // The executor lowers it (the JSONB ops over props) — proving Knowledge owns the executor over
    // the shared definition.
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

    // And the whole VIEW_QUERY composes with the view's frozen sort + order_field tiebreak.
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

/// **4.3 conjoin (the leak-free view-permission gate): the executor conjoins the `SetExpr` ACL
/// INSIDE the query — 0 post-filter.** The view filter AND the ACL are ONE conjunction in the WHERE,
/// before pagination. A `None` ACL is a deny conjunct (the view returns nothing); a restrictive
/// `Ids` ACL is an IN-set conjoined with the filter.
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
    // The reverse-index JOIN ACL (the InRelation form) is conjoined INTO the view query + the COUNT.
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

    // The permission-correct COUNT conjoins the SAME ACL INSIDE the aggregate (KN-D5 count-leak-closed).
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

/// **6.3 — the measured >5% facet-promotion trigger; the threshold matches the file (one source of
/// truth) and the trigger is strictly above the ratio (never weakened).**
#[test]
fn cdc_6_3_facet_promotion_threshold_matches_file_and_is_strict() {
    // The executor's constant == the value in the thresholds file (one source of truth, no drift).
    let thresholds = Thresholds::load_canonical().expect("the canonical thresholds file loads");
    assert_eq!(
        thresholds.flex_db.facet_promotion_ratio, FACET_PROMOTION_THRESHOLD,
        "the executor's facet-promotion threshold == the thresholds-file value (one source of truth)"
    );
    assert_eq!(
        FACET_PROMOTION_THRESHOLD, 0.05,
        "the frozen 6.3 trigger is >5%"
    );

    // The measured trigger fires strictly ABOVE the ratio. 20 executions: `hot` in 2 (10% > 5% →
    // promote), `edge` in 1 (5% == ratio → does NOT promote, strict >).
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

/// **The typed FieldType validation gate is part of the executor contract: a wrong-typed value is
/// rejected, never coerced into a query.**
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
