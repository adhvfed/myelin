//! # CDC — Issues' Tier-3 cost-bounder escalation, the **consumer side of contract 6.1** (ISS-P14 →
//! P-380, M4).
//!
//! **Architecture:** `issue-tracker/architecture/02-internals-and-algorithms.md` §3 (the three-tier
//! escalation — Tier 3 escalates to Search's `query(ast, viewer, zookie, page)` conjoining the SAME
//! OQ-E `Filter` before scoring); `search-and-indexing.md` §4.2.4 (the byte-identical Tier-3 valve).
//! **Contracts:** 6.1 `query` (Issues' cost-bounder is the CONSUMER — it drives Search's `query` via
//! the SRCH-P21 valve), 4.3 the `SetExpr` (byte-identical to the OLTP board's leak-free pre-filter).
//!
//! - **CONSUMER (6.1)** = Issues' [`plan_board_query`] cost-bounder. When a query is over budget / a
//!   full-text leg, it returns an [`EscalateToSearch`](myelin_issues::PlanOutcome::EscalateToSearch)
//!   carrying the board's OWN `Filter{set_expr}` (4.3); the [`SearchEscalation::to_board_query`] wire
//!   drives Search's `escalate_to_search` (the provider). This pins: the cost-bounder makes the
//!   escalation decision, carries the SAME `set_expr` (NOT a re-derivation), and the conjoined Filter
//!   excludes the confidential issue (0 leak) over the LIVE Search engine.
//! - **PROVIDER (6.1)** = Search's `query` (driven through `escalate_to_search`) always conjoins the
//!   board's `Filter{set_expr}` BEFORE scoring; a denied issue surfaces in NEITHER result NOR count.
//!
//! The `search-requires-acl-filter` discipline holds STRUCTURALLY: the cost-bounder's escalation type
//! is constructible ONLY with the board's `set_expr`, so there is no escalation path without the
//! conjoined Filter (0 Search calls without it). If the 6.1 `query` or 4.3 `SetExpr` shape drifts, this
//! stops compiling/passing — that is the contract.
//!
//! Dated green artifact (2026-06-23): Issues' cost-bounder escalates over-budget / full-text board
//! queries to Search with the board's 4.3 filter; the confidential issue is absent from the LIVE
//! Search result; the escalation carries the SAME `set_expr` the OLTP board would have conjoined.

use std::collections::BTreeMap;

use myelin_identity::{
    Consistency, ConsistencyMode, Literal, ObjectId, ObjectType, Principal, PrincipalId,
    PrincipalKind, SetExpr, Zookie,
};
use myelin_issues::{plan_board_query, CostBudget, FacetCatalog, PlanOutcome};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_search::{
    escalate_to_search, FieldDecl, FieldSchema, IndexBackend, IndexDocument, Page, QueryStats,
    ScopedEngine, TantivyBackend, FT_BODY_FIELD, ORDER_KEY_FIELD,
};
use myelin_tenancy::{Region, TenantId};

const STATE_FACET: &str = "state_category";

fn facet_decl() -> BTreeMap<String, FieldType> {
    let mut m = BTreeMap::new();
    m.insert(STATE_FACET.to_string(), FieldType::Select);
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

fn schema() -> FieldSchema {
    FieldSchema::new()
        .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
        .with(STATE_FACET, FieldDecl::stored(FieldType::Select))
        .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
}

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:alice".into()),
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
fn issue_ty() -> ObjectType {
    ObjectType("issue".into())
}
fn consistency() -> Consistency {
    Consistency {
        at_least: Zookie("z@0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

const A: &str = "myelin://acme/issue/issue/ENG-1";
const B: &str = "myelin://acme/issue/issue/ENG-2";
const SECRET: &str = "myelin://acme/issue/issue/ENG-9";

fn corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let k = OrderKey::bisect(None, None);
    let doc = |id: &str| {
        IndexDocument::new(id, "alpha shared term")
            .with_field(STATE_FACET, FieldValue::Select("started".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()))
    };
    be.upsert(&doc(A)).unwrap();
    be.upsert(&doc(B)).unwrap();
    be.upsert(&doc(SECRET)).unwrap();
    be
}

/// The board's `- confidential` filter (the leak-free pre-filter, 4.3): {A, B, SECRET} − {SECRET} = {A, B}.
fn board_set_expr() -> SetExpr {
    SetExpr::Difference(
        Box::new(SetExpr::Ids(vec![
            ObjectId(A.into()),
            ObjectId(B.into()),
            ObjectId(SECRET.into()),
        ])),
        Box::new(SetExpr::Ids(vec![ObjectId(SECRET.into())])),
    )
}

/// A full-text board query (an inherent Tier-3 leg).
fn fulltext_ast() -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(FT_BODY_FIELD.into()),
        rhs: Expr::Lit(Literal::Str("alpha".into())),
    })
    .expect("within cost bounds")
}

/// **CONSUMER (6.1): the cost-bounder escalates a full-text board query, carrying the board's OWN
/// `set_expr` (4.3) into Search — and the LIVE Search result conjoins it (the confidential issue is
/// ABSENT, 0 leak).** This drives the cost-bounder DECISION → the escalation wire → the live engine.
#[test]
fn cost_bounder_escalates_fulltext_with_same_filter_zero_leak() {
    // 1. The cost-bounder classifies the full-text leg as Tier 3 → escalate, carrying the board's filter.
    let outcome = plan_board_query(
        &fulltext_ast(),
        &board_set_expr(),
        &viewer(),
        &tenant(),
        &region(),
        &Zookie("z@0".into()),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        10, // tiny fan-out — but full-text always escalates
    );
    let esc = match outcome {
        PlanOutcome::EscalateToSearch(e) => e,
        other => panic!("a full-text leg must escalate to Search, got {other:?}"),
    };
    // The escalation carries the SAME set_expr the OLTP board would have conjoined (4.3 — NOT re-derived).
    assert_eq!(
        esc.set_expr,
        board_set_expr(),
        "the board's OWN filter (4.3)"
    );

    // 2. Drive the LIVE Search engine via the escalation wire (the SRCH-P21 valve, the 6.1 provider).
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let stats = QueryStats::new();
    let board = esc.to_board_query();
    let res = escalate_to_search(
        &eng,
        &board,
        &viewer(),
        &issue_ty(),
        &consistency(),
        Page {
            offset: 0,
            limit: 10,
        },
        &stats,
        None,
    )
    .expect("the live Search query conjoins the board's filter");

    // 3. The conjoined Filter excludes SECRET (0 leak) — A and B surface, SECRET does NOT (nor in count).
    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        ids.contains(&A) && ids.contains(&B),
        "the visible issues surface: {ids:?}"
    );
    assert!(
        !ids.contains(&SECRET),
        "0 leak: the confidential issue is ABSENT from the LIVE Search result (the conjoined Filter)"
    );
    // The ACL conjoin happened exactly once (no N+1) — the valve makes ONE list_objects conjoin.
    assert_eq!(
        stats.list_objects_calls(),
        1,
        "exactly ONE list_objects conjoin (no N+1)"
    );

    println!(
        "[ISS-P14 CDC 6.1 GREEN] the cost-bounder escalated a full-text board query with the board's \
         own SetExpr (4.3); the LIVE Search result conjoined it — SECRET absent (0 leak), one conjoin."
    );
}

/// **CONSUMER (6.1): an over-budget COLD facet escalates the SAME way (the cost dimension, not just
/// full-text).** A cold custom facet over budget escalates, carrying the board's filter — proving the
/// escalation is driven by the cost-bound, not only the field kind.
#[test]
fn cost_bounder_escalates_over_budget_cold_facet_with_same_filter() {
    let ast = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("customer_tier".into()), // a cold custom facet
        rhs: Expr::Lit(Literal::Str("enterprise".into())),
    })
    .unwrap();
    let outcome = plan_board_query(
        &ast,
        &board_set_expr(),
        &viewer(),
        &tenant(),
        &region(),
        &Zookie("z@0".into()),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        200_000, // over the OLTP budget for a GIN probe
    );
    match outcome {
        PlanOutcome::EscalateToSearch(e) => {
            assert_eq!(
                e.set_expr,
                board_set_expr(),
                "the over-budget escalation carries the SAME filter"
            );
            // And it drives the live engine leak-free, identically.
            let be = corpus();
            let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
            let stats = QueryStats::new();
            // Replace the cold-facet AST with a full-text AST for the live corpus probe (the corpus has
            // no `customer_tier` facet; the CONSUMER property under test is the carried filter, which is
            // identical). The escalation's set_expr is what matters for the leak property.
            let board = myelin_search::BoardQuery::new(
                fulltext_ast(),
                e.set_expr.clone(),
                e.zookie.clone(),
            );
            let res = escalate_to_search(
                &eng,
                &board,
                &viewer(),
                &issue_ty(),
                &consistency(),
                Page {
                    offset: 0,
                    limit: 10,
                },
                &stats,
                None,
            )
            .unwrap();
            let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
            assert!(
                !ids.contains(&SECRET),
                "0 leak on the over-budget escalation path too"
            );
        }
        other => panic!("an over-budget cold facet must escalate, got {other:?}"),
    }
}
