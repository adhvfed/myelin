//! # CDC — contract 13.3 (the read-time formula/rollup engine, OWNED) + 4.3 (the rollup
//! `list_objects` conjoin, CONSUMED) — KNOWLEDGE side (KN-P18 / P-308, M3)
//!
//! **Contract 13.3** (the `rollup`/`formula` "computed at READ TIME, never stored") — Knowledge OWNS
//! the bounded evaluator over the frozen [`myelin_query`] expression core. This CDC pins the
//! AGREEMENT: the formula tree reuses the frozen [`myelin_query::Literal`] value space + the SAME
//! static node/depth/step cost discipline the predicate core enforces (it never ships a second
//! expression engine, EI-01 §7); the value is computed at read time and never written back.
//!
//! **Contract 4.3** (the `list_objects` `SetExpr` push-down) is CONSUMED here — a rollup over a
//! relation aggregates ONLY the related rows the viewer may `read`, conjoining the SAME
//! [`lower_over_db_row_id`] lowering the view executor uses (the leak-free rollup-permission gate).
//! The producer/consumer agreement on 4.3 is pinned by `cdc_4_3_knowledge_list_pushdown.rs`; this
//! CDC asserts the ROLLUP conjoins it (0 rollup leak, 0 post-filter).
//!
//! Two CI gates the prompt names live here as deterministic asserts:
//! - **the #CYCLE gate** — a cyclic formula surfaces as `#CYCLE`, never an infinite loop (the
//!   bounded-evaluation counter = the green artifact);
//! - **the rollup-permission gate** — a restricted related row is never counted/summed for an
//!   unauthorized viewer (0 rollup leak, composing with KN-D5).
//!
//! ## The CDC pair (provider + consumer)
//! - **PROVIDER** — Knowledge PROVIDES the read-time formula/rollup half of 13.3 (the bounded
//!   `FormulaAst` evaluator, `myelin_knowledge::rollup`); it is the OWNER of the rollup engine the
//!   rest of the platform reads computed cells through. This CDC asserts the provider's engine
//!   computes the rollups at read time, surfaces `#CYCLE` on a cycle, and is bounded.
//! - **CONSUMER** — the engine CONSUMES the frozen `myelin-query` expression core (the `Literal`
//!   value space + the static node/depth cost ceilings, pinned byte-for-byte — never a re-definition)
//!   AND the `list_objects` `SetExpr` push-down (4.3) it conjoins into every rollup. A provider-side
//!   rename of the frozen ceilings / `Literal` shape fails here.
//!
//! Deterministic + DB-free (the contract-agreement). The live rollup-p99-at-scale + the
//! permission-filtered conjoin against the dev-stack Postgres is the `--features integration` test
//! (`integration_kn_d10_rollup.rs`) + the KN-D10 SCHED drill (`drill_kn_d10_rollup.rs`).

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

/// **13.3 — the formula engine reuses the frozen `myelin-query` cost discipline (no second engine).**
/// The formula static bounds are the SAME ceilings the predicate core enforces — a drift here would
/// be a second, divergent expression engine (the EI-01 §7 anti-pattern). Pinned byte-for-byte.
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

/// **13.3 — the formula value space IS the frozen `myelin_query::Literal` (no parallel value type).**
/// A formula literal is the SAME `Bool|Int|Str` the predicate `Expr::Lit` carries — the rollup
/// engine consumes the frozen value space, never a re-definition.
#[test]
fn cdc_13_3_formula_value_space_is_the_frozen_literal() {
    let relations = RelationStore::new();
    let authz = AuthzVisibleIndex::new();
    let tenant = TenantId("acme".into());
    let region = Region::new("fr-par");
    let target_props: BTreeMap<String, PropertyBag> = BTreeMap::new();
    let r = RollupResolver::new(&tenant, &region, &relations, &authz, &target_props);
    // A formula whose literals are the frozen Literal variants.
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

/// **The #CYCLE gate (CI):** a cyclic formula surfaces as `#CYCLE`, never an infinite loop.
#[test]
fn cdc_13_3_cycle_gate_surfaces_cycle_never_loops() {
    let relations = RelationStore::new();
    let authz = AuthzVisibleIndex::new();
    let tenant = TenantId("acme".into());
    let region = Region::new("fr-par");
    let target_props: BTreeMap<String, PropertyBag> = BTreeMap::new();
    let r = RollupResolver::new(&tenant, &region, &relations, &authz, &target_props);
    // a = b; b = a → a cycle. The evaluation TERMINATES (the test itself completing is the
    // bounded-evaluation green artifact) and yields #CYCLE.
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
        "a cyclic formula is #CYCLE (bounded — the eval terminated)"
    );
    assert_eq!(
        out.display(),
        "#CYCLE",
        "the diagnostic cell renders #CYCLE"
    );
}

/// **The rollup-permission gate (CI):** a restricted related row is never counted/summed for an
/// unauthorized viewer — the rollup conjoins `list_objects` (4.3), 0 rollup leak (composes KN-D5).
#[test]
fn cdc_4_3_rollup_permission_gate_zero_leak() {
    let tenant = TenantId("acme".into());
    let region = Region::new("fr-par");
    let relations = RelationStore::new();
    let authz = AuthzVisibleIndex::new();
    let v = viewer("p:viewer");

    // Three related targets; the viewer is granted read of two, the third (a big value) is restricted.
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
    // t:secret is NOT granted to the viewer.

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
        "0 rollup leak: SUM = 30 (10+20), NOT 100030 — the restricted target is unsummed"
    );
    assert_eq!(
        count,
        CellValue::Int(2),
        "0 rollup leak: COUNT = 2, NOT 3 — the restricted target is uncounted"
    );

    // An AUTHORIZED viewer (granted all three) sees the full aggregate — proving the rollup is not
    // a blanket hide but a per-viewer permission conjoin.
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
