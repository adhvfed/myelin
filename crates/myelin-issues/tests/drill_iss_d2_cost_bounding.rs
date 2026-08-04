use myelin_identity::{ObjectId, Principal, PrincipalId, PrincipalKind, SetExpr, Zookie};
use myelin_issues::{plan_board_query, CostBudget, FacetCatalog, PlanOutcome};
use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
use myelin_tenancy::{Region, TenantId};

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:eng".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}
fn acl() -> SetExpr {
    SetExpr::Ids(vec![ObjectId("ENG-1".into()), ObjectId("ENG-2".into())])
}
fn ast_over(field: &str) -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(field.into()),
        rhs: Expr::Lit(myelin_identity::Literal::Str("x".into())),
    })
    .unwrap()
}

#[test]
fn iss_d2_no_unbounded_scan_across_the_classification_sweep() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let zk = Zookie("zk-0000000010".into());

    let fields_cold = ["state", "severity", "customer_tier", "text", "semantic"];
    let fanouts = [
        10u64, 1_000, 50_000, 100_000, 1_000_000, 6_000_000, 50_000_000,
    ];

    let mut served = 0u32;
    let mut escalated = 0u32;
    let mut refined = 0u32;

    for promote_severity in [false, true] {
        let mut cat = FacetCatalog::new();
        if promote_severity {
            cat.promote("severity");
        }
        for field in fields_cold {
            for fanout in fanouts {
                let outcome = plan_board_query(
                    &ast_over(field),
                    &acl(),
                    &viewer(),
                    &tenant,
                    &region,
                    &zk,
                    &cat,
                    &CostBudget::DEFAULT,
                    fanout,
                );
                assert!(
                    outcome.assert_no_unbounded_scan(),
                    "field={field} fanout={fanout} promoted={promote_severity}: the cost-bounder \
                     emitted an UNBOUNDED outcome (the ISS-D2 no-full-scan invariant)"
                );
                match outcome {
                    PlanOutcome::ServeOltp(q) => {
                        assert!(
                            q.is_bounded(),
                            "a served query is paginated + statement-timeout'd"
                        );
                        served += 1;
                    }
                    PlanOutcome::EscalateToSearch(e) => {
                        assert_eq!(
                            e.set_expr,
                            acl(),
                            "the escalation carries the SAME Filter (4.3)"
                        );
                        escalated += 1;
                    }
                    PlanOutcome::Refine(r) => {
                        assert!(r.estimated_cost > 0);
                        refined += 1;
                    }
                }
            }
        }
    }

    assert!(served > 0, "some queries serve on OLTP");
    assert!(escalated > 0, "some queries escalate to Search");
    assert!(
        refined > 0,
        "some queries return Refine (cost beyond Search's bound)"
    );
    println!(
        "[ISS-D2 DECISION DRILL GREEN] sweep over {} scenarios: served={served} escalated={escalated} \
         refined={refined}; NO unbounded JSONB scan in any outcome.",
        served + escalated + refined
    );
}

#[test]
fn iss_d2_fulltext_always_escalates_never_oltp_scan() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let zk = Zookie("zk-0".into());
    for fanout in [1u64, 100, 10_000, 1_000_000] {
        let outcome = plan_board_query(
            &ast_over("text"),
            &acl(),
            &viewer(),
            &tenant,
            &region,
            &zk,
            &FacetCatalog::new(),
            &CostBudget::DEFAULT,
            fanout,
        );
        assert!(
            outcome.is_escalate() || outcome.is_refine(),
            "a full-text leg escalates (or refines) - never an OLTP full-text scan"
        );
    }
}
