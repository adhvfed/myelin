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

fn fifty_field_typed_core_predicate(n: usize) -> Predicate {
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
    let pred = fifty_field_typed_core_predicate(55);
    let ast = QueryAst::compiled(pred).expect("a 55-field AND is within the static cost bound");
    let cat = FacetCatalog::new();
    if let Some(p) = ast.predicate() {
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
        2_000,
    );
    assert!(
        outcome.is_serve_oltp(),
        "a 50+-field typed-core board query stays on OLTP (Tier-1 index range)"
    );
    assert!(
        outcome.assert_no_unbounded_scan(),
        "the served query is paginated + statement-timeout'd - never an unbounded scan"
    );
    if let PlanOutcome::ServeOltp(q) = outcome {
        assert_eq!(q.tier, Tier::TypedCore);
        assert!(q.is_bounded());
        assert!(q.composed.sql.contains("authz_visible"));
        assert!(q.composed.sql.contains("tenant_id = :tenant"));
        assert_eq!(q.composed.statement_count(), 1, "ONE query, no N+1");
    }
}

#[test]
fn cold_ad_hoc_query_escalates_to_search_with_same_filter() {
    let ast = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("customer_tier".into()),
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
        500_000,
    );
    assert!(
        outcome.is_escalate(),
        "a cold ad-hoc query over budget escalates to Search - never scans the JSONB tail unbounded"
    );
    if let PlanOutcome::EscalateToSearch(esc) = outcome {
        assert_eq!(esc.set_expr, view_acl());
        assert_eq!(esc.zookie, zk());
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
    let ast = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("severity".into()),
        rhs: Expr::Lit(myelin_identity::Literal::Int(1)),
    })
    .unwrap();
    let fanout = 10_000;

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

    let mut cat = FacetCatalog::new();
    cat.promote("severity");
    assert_eq!(classify_field("severity", &cat), Tier::GeneratedFacet);

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
        assert!(
            q.composed.sql.contains("AND NOT"),
            "the confidential set-difference excludes"
        );
    } else {
        panic!("a small typed-core query serves on OLTP");
    }

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
