use super::*;
use myelin_identity::{ObjectId, PrincipalId, PrincipalKind, RelName};
use myelin_query::{CmpOp, Expr, Predicate};

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}
fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn zk() -> Zookie {
    Zookie("zk-0000000010".into())
}

fn cmp_field(field: &str) -> Predicate {
    Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(field.into()),
        rhs: Expr::Lit(myelin_identity::Literal::Str("x".into())),
    }
}

fn ast_over(field: &str) -> QueryAst {
    QueryAst::compiled(cmp_field(field)).expect("a one-field predicate is within the cost bound")
}

fn acl() -> SetExpr {
    SetExpr::Ids(vec![ObjectId("ENG-1".into()), ObjectId("ENG-2".into())])
}

#[test]
fn typed_core_fields_classify_tier_1() {
    let cat = FacetCatalog::new();
    for f in [
        "state",
        "state_category",
        "priority",
        "assignee",
        "project",
        "cycle",
        "rank",
    ] {
        assert_eq!(
            classify_field(f, &cat),
            Tier::TypedCore,
            "{f} is a typed-core column → Tier 1"
        );
    }
}

#[test]
fn cold_custom_facet_classifies_tier_2b_gin() {
    let cat = FacetCatalog::new();
    assert_eq!(classify_field("severity", &cat), Tier::GinProbe);
    assert_eq!(classify_field("story_points", &cat), Tier::GinProbe);
    assert_eq!(
        classify_field("totally_unknown_field", &cat),
        Tier::GinProbe
    );
}

#[test]
fn promoted_custom_facet_classifies_tier_2_generated() {
    let mut cat = FacetCatalog::new();
    assert_eq!(classify_field("severity", &cat), Tier::GinProbe);
    cat.promote("severity");
    assert_eq!(
        classify_field("severity", &cat),
        Tier::GeneratedFacet,
        "once the feeder promotes it (a generated index), severity is Tier 2"
    );
    assert_eq!(classify_field("story_points", &cat), Tier::GinProbe);
}

#[test]
fn fulltext_and_semantic_fields_classify_tier_3() {
    let cat = FacetCatalog::new();
    for f in ["text", "body", "fulltext", "semantic", "any_artifact"] {
        assert_eq!(
            classify_field(f, &cat),
            Tier::Search,
            "{f} is inherently Tier 3"
        );
    }
}

#[test]
fn small_typed_core_query_serves_oltp_bounded() {
    let outcome = plan_board_query(
        &ast_over("state"),
        &acl(),
        &viewer("u"),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        100,
    );
    assert!(
        outcome.is_serve_oltp(),
        "a small typed-core query serves on OLTP"
    );
    assert!(
        outcome.assert_no_unbounded_scan(),
        "the served query is bounded"
    );
    if let PlanOutcome::ServeOltp(q) = outcome {
        assert_eq!(q.tier, Tier::TypedCore);
        assert!(q.is_bounded(), "paginated + statement-timeout'd");
        assert!(q.composed.sql.contains("LIMIT :page"), "ALWAYS paginated");
        assert_eq!(
            q.statement_timeout_ms,
            CostBudget::DEFAULT.statement_timeout_ms
        );
        assert_eq!(q.page_limit, CostBudget::DEFAULT.page_limit);
        let params = q.params();
        assert!(params.iter().any(|p| p.placeholder == ":page"));
        assert!(params
            .iter()
            .any(|p| p.placeholder == ":statement_timeout_ms"));
    } else {
        unreachable!();
    }
}

#[test]
fn over_budget_cold_facet_escalates_never_scans() {
    let outcome = plan_board_query(
        &ast_over("severity"),
        &acl(),
        &viewer("u"),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        100_000,
    );
    assert!(
        outcome.is_escalate(),
        "an over-budget cold facet escalates to Search - NEVER an unbounded JSONB scan"
    );
    assert!(outcome.assert_no_unbounded_scan());
    if let PlanOutcome::EscalateToSearch(e) = outcome {
        assert_eq!(
            e.set_expr,
            acl(),
            "the SAME Filter the OLTP board would have conjoined"
        );
        assert_eq!(e.zookie, zk(), "the SAME consistency snapshot");
        assert!(e.page_limit > 0, "Search is paginated too");
    } else {
        unreachable!();
    }
}

#[test]
fn fulltext_leg_escalates_regardless_of_fanout() {
    let outcome = plan_board_query(
        &ast_over("text"),
        &acl(),
        &viewer("u"),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        10,
    );
    assert!(
        outcome.is_escalate(),
        "a full-text leg always escalates to Search"
    );
}

#[test]
fn cost_beyond_search_bound_returns_refine() {
    let outcome = plan_board_query(
        &ast_over("severity"),
        &acl(),
        &viewer("u"),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        9_000_000,
    );
    assert!(
        outcome.is_refine(),
        "a cost beyond Search's bound returns Refine, not a scan"
    );
    assert!(outcome.assert_no_unbounded_scan());
    if let PlanOutcome::Refine(r) = outcome {
        assert!(
            r.hint.contains("narrow"),
            "the hint asks the operator to narrow: {}",
            r.hint
        );
        assert!(r.estimated_cost >= 5_000_000);
    } else {
        unreachable!();
    }
}

#[test]
fn promotion_keeps_a_hot_facet_on_oltp() {
    let mut cat = FacetCatalog::new();
    cat.promote("severity");
    let fanout = 20_000;
    let hot = plan_board_query(
        &ast_over("severity"),
        &acl(),
        &viewer("u"),
        &tenant(),
        &region(),
        &zk(),
        &cat,
        &CostBudget::DEFAULT,
        fanout,
    );
    assert!(
        hot.is_serve_oltp(),
        "the promoted generated-index facet fits the budget → OLTP"
    );
    if let PlanOutcome::ServeOltp(q) = hot {
        assert_eq!(q.tier, Tier::GeneratedFacet);
    }
    let cold = plan_board_query(
        &ast_over("severity"),
        &acl(),
        &viewer("u"),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        fanout,
    );
    assert!(
        cold.is_escalate(),
        "the SAME fan-out on a cold GIN facet escalates (weight 8)"
    );
}

#[test]
fn escalation_carries_same_filter_into_search_valve() {
    let esc = SearchEscalation::new(ast_over("text"), acl(), zk(), 50);
    let bq = esc.to_board_query();
    assert_eq!(
        bq.set_expr,
        acl(),
        "the board's OWN set_expr reaches Search verbatim (4.3)"
    );
    assert_eq!(
        bq.zookie,
        zk(),
        "the SAME consistency snapshot threads through (4.10)"
    );
    let view = SetExpr::Difference(
        Box::new(SetExpr::Ids(vec![
            ObjectId("A".into()),
            ObjectId("B".into()),
        ])),
        Box::new(SetExpr::Ids(vec![ObjectId("B".into())])),
    );
    let visible = myelin_search::oltp_board_admits(
        &view,
        &["A".into(), "B".into()],
        &viewer("u"),
        &zk(),
        None,
    )
    .expect("the board's ACL lowers through the SAME Search lowering");
    assert_eq!(
        visible,
        vec!["A".to_string()],
        "the set-difference excludes B (leak-equivalent)"
    );
}

#[test]
fn no_escalation_without_acl_filter_structurally() {
    let esc = SearchEscalation::new(ast_over("text"), acl(), zk(), 50);
    assert_eq!(esc.set_expr, acl());
}

#[test]
fn cost_estimate_is_the_bottleneck_oltp_weight() {
    assert_eq!(
        estimate_cost(&[Tier::TypedCore], 1000),
        1000,
        "Tier 1 weight 1"
    );
    assert_eq!(
        estimate_cost(&[Tier::GinProbe], 1000),
        8000,
        "Tier 2b weight 8"
    );
    assert_eq!(
        estimate_cost(&[Tier::GeneratedFacet], 1000),
        2000,
        "Tier 2 weight 2"
    );
    assert_eq!(
        estimate_cost(&[Tier::Search], 1000),
        0,
        "a Tier-3 leg pays 0 OLTP cost"
    );
    assert_eq!(
        estimate_cost(&[Tier::TypedCore; 50], 100),
        100,
        "50 typed-core conjuncts = one index range (max weight 1), NOT 50× the cost"
    );
    assert_eq!(
        estimate_cost(&[Tier::TypedCore, Tier::GinProbe], 100),
        800,
        "100 × max(1, 8) - the GIN probe is the bottleneck"
    );
}

#[test]
fn default_budget_is_under_one_second_and_paginated() {
    let b = CostBudget::DEFAULT;
    assert!(
        b.statement_timeout_ms < 1000,
        "the hard timeout is under the <1s keyboard budget"
    );
    assert!(b.page_limit > 0, "ALWAYS paginated");
    assert!(
        b.max_scanned_cost > 0,
        "an OLTP→Search escalation threshold exists"
    );
    assert!(
        b.refine_cost_ceiling > b.max_scanned_cost,
        "Refine is beyond the OLTP escalation point"
    );
}

#[test]
fn served_query_conjoins_acl_prefilter_before_pagination() {
    let view = SetExpr::Union(vec![
        SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: crate::planner::issue_id_colref(),
        },
        SetExpr::Ids(vec![ObjectId("ENG-9".into())]),
    ]);
    let outcome = plan_board_query(
        &ast_over("state"),
        &view,
        &viewer("u"),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        100,
    );
    if let PlanOutcome::ServeOltp(q) = outcome {
        let sql = &q.composed.sql;
        let where_pos = sql.find("WHERE").unwrap();
        let order_pos = sql.find("ORDER BY").unwrap();
        assert!(
            where_pos < order_pos,
            "the ACL pre-filter is conjoined BEFORE the ORDER BY"
        );
        assert!(
            sql.contains("authz_visible"),
            "the reverse-index JOIN is present (the read relation)"
        );
        assert!(
            sql.contains("tenant_id = :tenant"),
            "the tenant predicate isolates cross-tenant rows"
        );
        assert!(sql.ends_with("LIMIT :page"), "paginated last");
    } else {
        unreachable!("a small typed-core query with a relational ACL serves on OLTP");
    }
}

#[test]
fn floors_are_named() {
    assert_eq!(CostBounderFloors::TIER2_FEEDER, "ISS-P15");
    assert_eq!(CostBounderFloors::DISTRIBUTED_SQL, "ISS-P32");
    assert_eq!(CostBounderFloors::SURGE_LATENCY, "ISS-P33");
    assert!(CostBounderFloors::OQ_C_DEFAULT_TO_BEAT.contains("5%"));
}
