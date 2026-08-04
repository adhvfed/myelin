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

fn fulltext_ast() -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(FT_BODY_FIELD.into()),
        rhs: Expr::Lit(Literal::Str("alpha".into())),
    })
    .expect("within cost bounds")
}

#[test]
fn cost_bounder_escalates_fulltext_with_same_filter_zero_leak() {
    let outcome = plan_board_query(
        &fulltext_ast(),
        &board_set_expr(),
        &viewer(),
        &tenant(),
        &region(),
        &Zookie("z@0".into()),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        10,
    );
    let esc = match outcome {
        PlanOutcome::EscalateToSearch(e) => e,
        other => panic!("a full-text leg must escalate to Search, got {other:?}"),
    };
    assert_eq!(
        esc.set_expr,
        board_set_expr(),
        "the board's OWN filter (4.3)"
    );

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

    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        ids.contains(&A) && ids.contains(&B),
        "the visible issues surface: {ids:?}"
    );
    assert!(
        !ids.contains(&SECRET),
        "0 leak: the confidential issue is ABSENT from the LIVE Search result (the conjoined Filter)"
    );
    assert_eq!(
        stats.list_objects_calls(),
        1,
        "exactly ONE list_objects conjoin (no N+1)"
    );

    println!(
        "[ISS-P14 CDC 6.1 GREEN] the cost-bounder escalated a full-text board query with the board's \
         own SetExpr (4.3); the LIVE Search result conjoined it - SECRET absent (0 leak), one conjoin."
    );
}

#[test]
fn cost_bounder_escalates_over_budget_cold_facet_with_same_filter() {
    let ast = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("customer_tier".into()),
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
        200_000,
    );
    match outcome {
        PlanOutcome::EscalateToSearch(e) => {
            assert_eq!(
                e.set_expr,
                board_set_expr(),
                "the over-budget escalation carries the SAME filter"
            );
            let be = corpus();
            let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
            let stats = QueryStats::new();
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
