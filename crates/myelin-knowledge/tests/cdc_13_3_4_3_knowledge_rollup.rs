use std::collections::BTreeMap;

use myelin_identity::{Literal, Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::{
    compute_row, AuthzVisibleIndex, CellValue, DbRelation, FormulaExpr, FormulaField,
    FormulaSchema, PropertyBag, RelationKind, RelationStore, RollupFn, RollupResolver,
    MAX_FORMULA_DEPTH, MAX_FORMULA_NODES,
};
use myelin_query::{FieldId, FieldValue, MAX_PREDICATE_DEPTH, MAX_PREDICATE_NODES};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

#[test]
fn cdc_13_3_formula_reuses_the_frozen_predicate_cost_bounds() {
    assert_eq!(
        MAX_FORMULA_NODES, MAX_PREDICATE_NODES,
        "the formula node ceiling IS the frozen predicate node ceiling (one cost model, EI-01 §7)"
    );
    assert_eq!(
        MAX_FORMULA_DEPTH, MAX_PREDICATE_DEPTH,
        "the formula depth ceiling IS the frozen predicate depth ceiling (one cost model)"
    );
}

#[test]
fn cdc_13_3_formula_value_space_is_the_frozen_literal() {
    let relations = RelationStore::new();
    let authz = AuthzVisibleIndex::new();
    let tenant = TenantId("acme".into());
    let region = Region::new("fr-par");
    let target_props: BTreeMap<String, PropertyBag> = BTreeMap::new();
    let r = RollupResolver::new(&tenant, &region, &relations, &authz, &target_props);
    let schema = FormulaSchema::of([FormulaField {
        field: FieldId::new("f"),
        expr: FormulaExpr::Add(
            Box::new(FormulaExpr::Lit(Literal::Int(40))),
            Box::new(FormulaExpr::Lit(Literal::Int(2))),
        ),
    }])
    .unwrap();
    let out = compute_row(
        &viewer("p:v"),
        "src",
        &FieldId::new("f"),
        &BTreeMap::new(),
        &schema,
        &r,
    );
    assert_eq!(
        out,
        CellValue::Int(42),
        "the formula evaluates over the frozen Literal value space"
    );
}

#[test]
fn cdc_13_3_cycle_gate_surfaces_cycle_never_loops() {
    let relations = RelationStore::new();
    let authz = AuthzVisibleIndex::new();
    let tenant = TenantId("acme".into());
    let region = Region::new("fr-par");
    let target_props: BTreeMap<String, PropertyBag> = BTreeMap::new();
    let r = RollupResolver::new(&tenant, &region, &relations, &authz, &target_props);
    let schema = FormulaSchema::of([
        FormulaField {
            field: FieldId::new("a"),
            expr: FormulaExpr::FormulaRef(FieldId::new("b")),
        },
        FormulaField {
            field: FieldId::new("b"),
            expr: FormulaExpr::FormulaRef(FieldId::new("a")),
        },
    ])
    .unwrap();
    let out = compute_row(
        &viewer("p:v"),
        "src",
        &FieldId::new("a"),
        &BTreeMap::new(),
        &schema,
        &r,
    );
    assert_eq!(
        out,
        CellValue::Cycle,
        "a cyclic formula is #CYCLE (bounded - the eval terminated)"
    );
    assert_eq!(
        out.display(),
        "#CYCLE",
        "the diagnostic cell renders #CYCLE"
    );
}

#[test]
fn cdc_4_3_rollup_permission_gate_zero_leak() {
    let tenant = TenantId("acme".into());
    let region = Region::new("fr-par");
    let relations = RelationStore::new();
    let authz = AuthzVisibleIndex::new();
    let v = viewer("p:viewer");

    let targets: &[(&str, i64)] = &[("t:1", 10), ("t:2", 20), ("t:secret", 100_000)];
    let mut target_props: BTreeMap<String, PropertyBag> = BTreeMap::new();
    for (i, (id, amount)) in targets.iter().enumerate() {
        relations.relate(
            &tenant,
            DbRelation {
                relation_id: format!("rel:{i}"),
                src_row: "src".to_string(),
                dst_ref: ArtifactRef((*id).to_string()),
                rel: RelationKind::RollupSource,
            },
        );
        let mut props: PropertyBag = BTreeMap::new();
        props.insert(FieldId::new("amount"), FieldValue::Int(*amount));
        target_props.insert((*id).to_string(), props);
    }
    authz.grant(
        &tenant,
        &region,
        &v.principal_id.0,
        "read",
        "t:1",
        "zk-0000000001",
    );
    authz.grant(
        &tenant,
        &region,
        &v.principal_id.0,
        "read",
        "t:2",
        "zk-0000000001",
    );

    let r = RollupResolver::new(&tenant, &region, &relations, &authz, &target_props);
    let sum_schema = FormulaSchema::of([FormulaField {
        field: FieldId::new("total"),
        expr: FormulaExpr::Rollup {
            func: RollupFn::Sum,
            target: FieldId::new("amount"),
        },
    }])
    .unwrap();
    let count_schema = FormulaSchema::of([FormulaField {
        field: FieldId::new("n"),
        expr: FormulaExpr::Rollup {
            func: RollupFn::Count,
            target: FieldId::new("amount"),
        },
    }])
    .unwrap();

    let sum = compute_row(
        &v,
        "src",
        &FieldId::new("total"),
        &BTreeMap::new(),
        &sum_schema,
        &r,
    );
    let count = compute_row(
        &v,
        "src",
        &FieldId::new("n"),
        &BTreeMap::new(),
        &count_schema,
        &r,
    );
    assert_eq!(
        sum,
        CellValue::Int(30),
        "0 rollup leak: SUM = 30 (10+20), NOT 100030 - the restricted target is unsummed"
    );
    assert_eq!(
        count,
        CellValue::Int(2),
        "0 rollup leak: COUNT = 2, NOT 3 - the restricted target is uncounted"
    );

    let v2 = viewer("p:admin");
    authz.grant(
        &tenant,
        &region,
        &v2.principal_id.0,
        "read",
        "t:1",
        "zk-0000000002",
    );
    authz.grant(
        &tenant,
        &region,
        &v2.principal_id.0,
        "read",
        "t:2",
        "zk-0000000002",
    );
    authz.grant(
        &tenant,
        &region,
        &v2.principal_id.0,
        "read",
        "t:secret",
        "zk-0000000002",
    );
    let sum2 = compute_row(
        &v2,
        "src",
        &FieldId::new("total"),
        &BTreeMap::new(),
        &sum_schema,
        &r,
    );
    assert_eq!(
        sum2,
        CellValue::Int(100_030),
        "the authorized viewer sees the full SUM (per-viewer conjoin, not a blanket hide)"
    );
}
