//! **ISS-D2 drill (DB-free decision half) — the cost-bounder never emits a full JSONB scan.**
//!
//! The ISS-D2 gate: a 50+ custom-field × 1M+ issues board query under the `<1s` keyboard budget with
//! the `SetExpr` JOIN; a cold ad-hoc query escalates to Search (same `Filter`); the planner NEVER emits
//! a full JSONB scan. THIS file is the DETERMINISTIC decision drill — the structural no-full-scan +
//! always-escalate-or-refine property over an exhaustive scenario sweep. The LIVE wall-clock proof
//! (p99 `< 1s`, 1M+ rows, real Postgres EXPLAIN shows no `Seq Scan` on the JSONB tail) is the
//! `--features integration` artifact (`integration_iss_p14_cost_bounding.rs`).
//!
//! Survival signal: for EVERY classification × fan-out in the sweep, the outcome is bounded — a served
//! query is paginated + statement-timeout'd, or it escalates (the SAME Filter), or it returns Refine.
//! There is NO outcome that runs an unbounded JSONB scan. That is the no-full-scan property the live
//! drill confirms in wall-clock.

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

    // The exhaustive scenario sweep: every tier × a fan-out from tiny to enormous. A custom facet is
    // tested both cold (GIN, Tier 2b) and promoted (generated, Tier 2).
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
                // THE INVARIANT: every outcome is bounded — NEVER an unbounded JSONB scan.
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
                        // A served query NEVER serves a cold-facet huge result (that must escalate):
                        // it is only served when the cost fit the budget.
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

    // The sweep exercised all three outcomes (a real cost-bounder, not a degenerate always-escalate).
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
    // Free-text has no OLTP index that serves the keyboard budget — it escalates at ANY fan-out.
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
            "a full-text leg escalates (or refines) — never an OLTP full-text scan"
        );
    }
}
