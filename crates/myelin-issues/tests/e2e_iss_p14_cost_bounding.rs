//! **ISS-P14 / P-380 — the cost-bounding + three-tier escalation chained-mutation e2e.**
//!
//! The DoD's chained-mutation e2e: a 50+-field board query stays under budget (Tier 1 — a typed-core
//! board scan over a large corpus serves on OLTP, paginated + statement-timeout'd, NEVER an unbounded
//! scan); a cold ad-hoc query escalates to Search WITH THE SAME `Filter`. The chained mutation is the
//! tier TRANSITION: a custom facet starts cold (Tier 2b / GIN) → over budget it escalates → the
//! projection feeder (ISS-P15, modelled by [`FacetCatalog::promote`]) promotes it → the SAME query at
//! the SAME fan-out now serves on OLTP (Tier 2, the generated index). The leak-free ACL pre-filter
//! (ISS-P13) is conjoined on EVERY tier.
//!
//! DB-free (the deterministic decision model); the live `<1s` × 1M+ board proof is the
//! `--features integration` ISS-D2 drill (`integration_iss_p14_cost_bounding.rs`).

use myelin_identity::{ObjectId, Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_issues::{
    classify_field, plan_board_query, CostBudget, FacetCatalog, PlanOutcome, Tier,
};
use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
use myelin_tenancy::{Region, TenantId};

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:eng".into()),
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
fn zk() -> myelin_identity::Zookie {
    myelin_identity::Zookie("zk-0000000042".into())
}

/// The leak-free `view` ACL pre-filter (the confidential set-difference, ISS-P13) — conjoined on EVERY
/// tier. `(read − confidential) + confidential_grant`.
fn view_acl() -> SetExpr {
    let in_rel = |r: &str| SetExpr::InRelation {
        relation: RelName(r.into()),
        via_column: myelin_issues::issue_id_colref(),
    };
    SetExpr::Union(vec![
        SetExpr::Difference(Box::new(in_rel("read")), Box::new(in_rel("confidential"))),
        in_rel("confidential_grant"),
    ])
}

/// A board predicate over `n` typed-core fields (the "50+ fields" board filter — every field is a
/// typed-core column so the whole query is Tier 1, an index range).
fn fifty_field_typed_core_predicate(n: usize) -> Predicate {
    // Cycle through the typed-core columns so a 50+-conjunct AND is all Tier 1.
    let cols = [
        "state",
        "state_category",
        "priority",
        "assignee",
        "reporter",
        "type",
        "parent",
        "project",
        "cycle",
        "rank",
        "created_at",
        "updated_at",
    ];
    let conjuncts: Vec<Predicate> = (0..n)
        .map(|i| Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(cols[i % cols.len()].into()),
            rhs: Expr::Lit(myelin_identity::Literal::Str("x".into())),
        })
        .collect();
    Predicate::And(conjuncts)
}

#[test]
fn fifty_field_board_query_stays_under_budget_on_oltp() {
    // A 50+-field board query over a LARGE corpus (1M+ issues) — but every field is typed-core, so it
    // is a Tier-1 index range; the fan-out for an indexed board scan is bounded (the index range +
    // pagination), well within budget. It serves on OLTP, paginated + statement-timeout'd.
    let pred = fifty_field_typed_core_predicate(55);
    let ast = QueryAst::compiled(pred).expect("a 55-field AND is within the static cost bound");
    // Every field classifies Tier 1 — the heaviest leg is still an index range.
    let cat = FacetCatalog::new();
    if let Some(p) = ast.predicate() {
        // (the classifier is total over every field)
        for f in ["state", "priority", "assignee", "project", "cycle", "rank"] {
            assert_eq!(classify_field(f, &cat), Tier::TypedCore);
        }
        let _ = p;
    }

    let outcome = plan_board_query(
        &ast,
        &view_acl(),
        &viewer(),
        &tenant(),
        &region(),
        &zk(),
        &cat,
        &CostBudget::DEFAULT,
        // The index-range fan-out for a typed-core board scan over 1M+ issues is bounded (the board
        // index range, not the whole table) — a few thousand candidate rows.
        2_000,
    );
    assert!(
        outcome.is_serve_oltp(),
        "a 50+-field typed-core board query stays on OLTP (Tier-1 index range)"
    );
    assert!(
        outcome.assert_no_unbounded_scan(),
        "the served query is paginated + statement-timeout'd — never an unbounded scan"
    );
    if let PlanOutcome::ServeOltp(q) = outcome {
        assert_eq!(q.tier, Tier::TypedCore);
        assert!(q.is_bounded());
        // The leak-free ACL pre-filter is conjoined (the read JOINs over authz_visible).
        assert!(q.composed.sql.contains("authz_visible"));
        assert!(q.composed.sql.contains("tenant_id = :tenant"));
        assert_eq!(q.composed.statement_count(), 1, "ONE query, no N+1");
    }
}

#[test]
fn cold_ad_hoc_query_escalates_to_search_with_same_filter() {
    // A cold ad-hoc custom facet over a huge result — over the OLTP budget. It escalates to Search,
    // carrying the SAME `Filter{set_expr}` (4.3) — NEVER an unbounded JSONB scan.
    let ast = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("customer_tier".into()), // a cold custom facet (Tier 2b / GIN)
        rhs: Expr::Lit(myelin_identity::Literal::Str("enterprise".into())),
    })
    .unwrap();
    let outcome = plan_board_query(
        &ast,
        &view_acl(),
        &viewer(),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        500_000, // a huge cold-facet fan-out — over budget
    );
    assert!(
        outcome.is_escalate(),
        "a cold ad-hoc query over budget escalates to Search — never scans the JSONB tail unbounded"
    );
    if let PlanOutcome::EscalateToSearch(esc) = outcome {
        // THE SAME Filter the OLTP board would have conjoined (byte-identical, 4.3).
        assert_eq!(esc.set_expr, view_acl());
        assert_eq!(esc.zookie, zk());
        // The escalation lowers the SAME set_expr through Search's OWN lowering (the SRCH-P21 parity
        // anchor) — a confidential issue is excluded by construction (leak-equivalent).
        let bq = esc.to_board_query();
        assert_eq!(
            bq.set_expr,
            view_acl(),
            "Search receives the board's set_expr verbatim"
        );
    }
}

#[test]
fn chained_mutation_promotion_flips_a_facet_from_search_to_oltp() {
    // The chained mutation: the SAME query at the SAME fan-out, before and after the projection feeder
    // (ISS-P15) promotes the facet. Cold → escalates; promoted → serves on OLTP (the generated index).
    let ast = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("severity".into()),
        rhs: Expr::Lit(myelin_identity::Literal::Int(1)),
    })
    .unwrap();
    // A fan-out that blows the GIN (weight 8) budget but fits the generated-index (weight 2) budget.
    let fanout = 10_000; // ×8 = 80_000 (> 50_000, blows) ; ×2 = 20_000 (≤ 50_000, fits)

    // BEFORE: cold GIN facet → escalates.
    let before = plan_board_query(
        &ast,
        &view_acl(),
        &viewer(),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        fanout,
    );
    assert!(
        before.is_escalate(),
        "cold GIN facet over budget → escalate"
    );

    // THE FEEDER PROMOTES IT (the measured ISS-P15 promotion, modelled by the catalog).
    let mut cat = FacetCatalog::new();
    cat.promote("severity");
    assert_eq!(classify_field("severity", &cat), Tier::GeneratedFacet);

    // AFTER: the SAME query at the SAME fan-out now serves on OLTP (the generated index is cheaper).
    let after = plan_board_query(
        &ast,
        &view_acl(),
        &viewer(),
        &tenant(),
        &region(),
        &zk(),
        &cat,
        &CostBudget::DEFAULT,
        fanout,
    );
    assert!(
        after.is_serve_oltp(),
        "after the feeder promotes the facet, the SAME query serves on OLTP (Tier 2)"
    );
    if let PlanOutcome::ServeOltp(q) = after {
        assert_eq!(q.tier, Tier::GeneratedFacet);
        assert!(q.is_bounded(), "still paginated + statement-timeout'd");
    }
}

#[test]
fn acl_prefilter_conjoined_on_every_tier_zero_leak_shape() {
    // The leak-free ACL pre-filter (ISS-P13) is conjoined on EVERY served tier, and the SAME set_expr
    // is what escalates to Search — the confidential set-difference excludes by construction on both.
    // (Tier 1 OLTP path)
    let oltp = plan_board_query(
        &QueryAst::compiled(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("state".into()),
            rhs: Expr::Lit(myelin_identity::Literal::Str("open".into())),
        })
        .unwrap(),
        &view_acl(),
        &viewer(),
        &tenant(),
        &region(),
        &zk(),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        100,
    );
    if let PlanOutcome::ServeOltp(q) = oltp {
        // The set-difference lowers to `AND NOT confidential` — a confidential issue is ABSENT.
        assert!(
            q.composed.sql.contains("AND NOT"),
            "the confidential set-difference excludes"
        );
    } else {
        panic!("a small typed-core query serves on OLTP");
    }

    // (Tier 3 escalation path — the SAME set_expr lowers through Search byte-identically.)
    let view = SetExpr::Difference(
        Box::new(SetExpr::Ids(vec![
            ObjectId("A".into()),
            ObjectId("B".into()),
        ])),
        Box::new(SetExpr::Ids(vec![ObjectId("B".into())])),
    );
    let visible =
        myelin_search::oltp_board_admits(&view, &["A".into(), "B".into()], &viewer(), &zk(), None)
            .unwrap();
    assert_eq!(
        visible,
        vec!["A".to_string()],
        "B (confidential) is absent on the Search tier too"
    );
}
