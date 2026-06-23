//! Unit tests for the ISS-P14 cost-bounder + three-tier escalation (the latency-correctness seam —
//! mandatory-core, like the leak seam). These pin: the §3 classification (each field → the right
//! tier); the cost-bound decision (a too-large scan ESCALATES or returns REFINE, never an unbounded
//! scan); every served query is paginated + statement-timeout'd; the Tier-3 escalation carries the
//! SAME `set_expr` (the `search-requires-acl-filter` discipline, structural). The live `<1s` × 1M+
//! board proof is the `--features integration` ISS-D2 drill; the chained-mutation e2e + the drill
//! scenario are the `tests/` files. These are the deterministic, DB-free unit drills.

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

/// A simple `field == lit` predicate over one field (the board filter shape).
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

/// The viewer's ACL pre-filter — a bounded allow-set (the leak-free `SetExpr`, ISS-P13).
fn acl() -> SetExpr {
    SetExpr::Ids(vec![ObjectId("ENG-1".into()), ObjectId("ENG-2".into())])
}

// ───────────────────────────── §3 classification (each field → the right tier) ──────────────────

/// **A typed-core field → Tier 1** (the `issue_board`/`issue_assignee` index range — the 90% hot path).
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

/// **A cold custom facet → Tier 2b (the GIN probe)** by default — the projection feeder (ISS-P15) has
/// not promoted it. NOT a typed-core mis-hit, NOT an unbounded scan.
#[test]
fn cold_custom_facet_classifies_tier_2b_gin() {
    let cat = FacetCatalog::new();
    assert_eq!(classify_field("severity", &cat), Tier::GinProbe);
    assert_eq!(classify_field("story_points", &cat), Tier::GinProbe);
    // An UNKNOWN field is a cold custom facet (Tier 2b), never a typed-core mis-hit (fail-conservative).
    assert_eq!(
        classify_field("totally_unknown_field", &cat),
        Tier::GinProbe
    );
}

/// **A MEASURED-HOT custom facet (the feeder promoted it) → Tier 2 (the generated index).** Promotion
/// is data-driven — the catalog reflects the ISS-P15 feeder, the cost-bounder never predicts it.
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
    // A NON-promoted facet is still Tier 2b — promotion is per-field, measured.
    assert_eq!(classify_field("story_points", &cat), Tier::GinProbe);
}

/// **A full-text / semantic / cross-artifact field → Tier 3 (escalate to Search) regardless of cost.**
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

// ───────────────────────────── the cost-bound decision (escalate / refine / serve) ──────────────

/// **Within budget → serve on the OLTP tier, paginated + statement-timeout'd (never unbounded).** A
/// typed-core board scan over a small fan-out stays on Tier 1.
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
        100, // small fan-out — well within budget
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
        // The page + timeout are BOUND params, never interpolated.
        let params = q.params();
        assert!(params.iter().any(|p| p.placeholder == ":page"));
        assert!(params
            .iter()
            .any(|p| p.placeholder == ":statement_timeout_ms"));
    } else {
        unreachable!();
    }
}

/// **A cold huge-result custom facet OVER budget → escalate to Search (the SAME Filter), never an
/// unbounded JSONB scan.** This is the §3 crux: a GIN probe whose fan-out blows the budget MUST escalate.
#[test]
fn over_budget_cold_facet_escalates_never_scans() {
    let outcome = plan_board_query(
        &ast_over("severity"), // cold custom facet (Tier 2b, weight 8)
        &acl(),
        &viewer("u"),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        // fan-out × GIN weight (8) blows max_scanned_cost (50_000) but is under the refine ceiling.
        100_000,
    );
    assert!(
        outcome.is_escalate(),
        "an over-budget cold facet escalates to Search — NEVER an unbounded JSONB scan"
    );
    assert!(outcome.assert_no_unbounded_scan());
    if let PlanOutcome::EscalateToSearch(e) = outcome {
        // The escalation carries the board's OWN set_expr (4.3) — byte-identical, leak-equivalent.
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

/// **An inherent Tier-3 leg (full-text) escalates to Search EVEN at a small fan-out.** Free-text has no
/// OLTP index that serves the keyboard budget — it always escalates.
#[test]
fn fulltext_leg_escalates_regardless_of_fanout() {
    let outcome = plan_board_query(
        &ast_over("text"), // inherently Tier 3
        &acl(),
        &viewer("u"),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        10, // tiny fan-out — but full-text still escalates
    );
    assert!(
        outcome.is_escalate(),
        "a full-text leg always escalates to Search"
    );
}

/// **A cost beyond even Search's bound → Refine, never a scan.** A cold huge-result ad-hoc facet the
/// operator must narrow returns a hint — the cost-bounder NEVER runs the scan.
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
        // fan-out beyond the refine ceiling (5_000_000) — even Search cannot serve it within budget.
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

/// **A promoted (Tier 2) facet at the SAME fan-out that a cold (Tier 2b) facet escalates at stays on
/// OLTP** — the generated index is cheaper (weight 2 vs 8), so it fits the budget. This pins that the
/// tier weight actually drives the escalation decision (not just the fan-out).
#[test]
fn promotion_keeps_a_hot_facet_on_oltp() {
    let mut cat = FacetCatalog::new();
    cat.promote("severity");
    let fanout = 20_000; // × generated weight 2 = 40_000 ≤ 50_000 (fits); × GIN weight 8 = 160_000 (blows)
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
    // The SAME facet, NOT promoted (cold GIN), blows the budget at the same fan-out → escalates.
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

// ───────────────────────────── the escalation carries the SAME Filter (search-requires-acl-filter) ──

/// **The Tier-3 escalation builds a `BoardQuery` carrying the board's OWN set_expr (6.1 / SRCH-P21).**
/// The seam to Search's valve carries the SAME `ast` + `set_expr` + zookie — byte-identical ACL
/// pre-filter, no second interpreter. The `search-requires-acl-filter` discipline is STRUCTURAL:
/// `SearchEscalation` is constructible only WITH the set_expr.
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
    // And the relational view set-expr lowers byte-identically through Search's lowering (the SRCH-P21
    // parity anchor) — a confidential set-difference excludes by construction.
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

/// **There is NO escalation path WITHOUT the conjoined Filter** — `SearchEscalation` has no constructor
/// that omits `set_expr` (the field is required; the only constructor takes it). This is the structural
/// `search-requires-acl-filter` guarantee, restated as a compile-time property (this test documents it).
#[test]
fn no_escalation_without_acl_filter_structurally() {
    // The ONLY way to build a SearchEscalation is `new(ast, set_expr, zookie, page)` — set_expr is not
    // Option, has no Default, and `plan_board_query` always passes the board's lowered set_expr. A
    // path that escalated without it would not type-check.
    let esc = SearchEscalation::new(ast_over("text"), acl(), zk(), 50);
    assert_eq!(esc.set_expr, acl());
}

// ───────────────────────────── cost estimate + budget bounds ─────────────────────────────────────

/// **The cost estimate is `row_fanout × max(OLTP tier-weight)` (the bottleneck leg); a Tier-3 leg pays
/// 0 OLTP cost.** A conjunction is bounded by its HEAVIEST access path (index intersection), not by the
/// number of conjuncts — a 50-field typed-core board scan is still one Tier-1 index range.
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
    // A pure Tier-3 leg pays 0 OLTP cost (it does not touch the OLTP store — Search owns its cost).
    assert_eq!(
        estimate_cost(&[Tier::Search], 1000),
        0,
        "a Tier-3 leg pays 0 OLTP cost"
    );
    // Many typed-core legs are STILL one index range — the cost is the bottleneck (Tier 1), not the sum.
    assert_eq!(
        estimate_cost(&[Tier::TypedCore; 50], 100),
        100,
        "50 typed-core conjuncts = one index range (max weight 1), NOT 50× the cost"
    );
    // A single cold-GIN leg in an otherwise typed-core query makes the WHOLE query pay the GIN cost.
    assert_eq!(
        estimate_cost(&[Tier::TypedCore, Tier::GinProbe], 100),
        800,
        "100 × max(1, 8) — the GIN probe is the bottleneck"
    );
}

/// **The default budget is paginated, statement-timeout'd under the `<1s` keyboard budget.**
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

/// **The leak-free ACL pre-filter is conjoined into EVERY served OLTP query (ISS-P13 holds under
/// ISS-P14).** A served board query's SQL conjoins the lowered `set_expr` BEFORE `ORDER BY rank LIMIT`.
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
        // The ACL JOIN + tenant predicate come BEFORE the ORDER BY / LIMIT (pre-filter, never post).
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

// ───────────────────────────── the named floors ─────────────────────────────────────────────────

/// **The floors are named (ISS-P14 DoD): the Tier-2 feeder (ISS-P15), distributed-SQL (ISS-P32), and
/// the surge latency (ISS-P33).**
#[test]
fn floors_are_named() {
    assert_eq!(CostBounderFloors::TIER2_FEEDER, "ISS-P15");
    assert_eq!(CostBounderFloors::DISTRIBUTED_SQL, "ISS-P32");
    assert_eq!(CostBounderFloors::SURGE_LATENCY, "ISS-P33");
    assert!(CostBounderFloors::OQ_C_DEFAULT_TO_BEAT.contains("5%"));
}
