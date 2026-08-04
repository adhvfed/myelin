use std::collections::BTreeMap;

use myelin_identity::{
    Consistency, ConsistencyMode, Literal, ObjectId, ObjectType, Principal, PrincipalId,
    PrincipalKind, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_tenancy::TenantId;

use myelin_identity::{ListObjectsResult, Permission};
use myelin_search::{
    board_acl_filter, escalate_to_search, oltp_board_admits, AclFilter, BoardEscalationAuthz,
    BoardQuery, FieldDecl, FieldSchema, IndexBackend, IndexDocument, ListObjectsPort, OltpBudget,
    Page, QueryStats, ScopedEngine, TantivyBackend, FT_BODY_FIELD, ORDER_KEY_FIELD,
};

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

fn issue_ty() -> ObjectType {
    ObjectType("issue".into())
}

fn consistency() -> Consistency {
    Consistency {
        at_least: Zookie("z@0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn ast() -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(FT_BODY_FIELD.into()),
        rhs: Expr::Lit(Literal::Str("alpha".into())),
    })
    .expect("within cost bounds")
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

#[test]
fn valve_consumes_6_1_with_one_list_objects_conjoin() {
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let board = BoardQuery::new(ast(), board_set_expr(), Zookie("z@0".into()));
    let stats = QueryStats::new();

    let res = escalate_to_search(
        &eng,
        &board,
        &viewer(),
        &issue_ty(),
        &consistency(),
        Page {
            offset: 0,
            limit: 100,
        },
        &stats,
        None,
    )
    .expect("the valve consumes 6.1");

    assert_eq!(
        stats.list_objects_calls(),
        1,
        "the valve makes exactly ONE list_objects conjoin (no N+1)"
    );
    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        ids.contains(&A) && ids.contains(&B),
        "the two visible issues surface"
    );
    assert!(
        !ids.contains(&SECRET),
        "the confidential issue (excluded by the board's set_expr) never surfaces through the valve"
    );
}

#[test]
fn escalation_port_returns_the_boards_own_set_expr() {
    let authz = BoardEscalationAuthz::new(board_set_expr(), Zookie("z@0".into()), issue_ty());
    let got = authz
        .list_objects(
            &viewer(),
            &Permission("read".into()),
            &issue_ty(),
            &consistency(),
        )
        .expect("the port returns the board's filter");
    match got {
        ListObjectsResult::Filter { set_expr, zookie } => {
            assert_eq!(
                set_expr,
                board_set_expr(),
                "the board's OWN set_expr, byte-identical (4.3)"
            );
            assert_eq!(zookie, Zookie("z@0".into()), "at the board's ACL snapshot");
        }
        other => panic!("the valve carries a pushed-down Filter, got {other:?}"),
    }
}

#[test]
fn seam_rejects_permission_and_type_mismatch_loudly() {
    let authz = BoardEscalationAuthz::new(board_set_expr(), Zookie("z@0".into()), issue_ty());

    let perm_err = authz.list_objects(
        &viewer(),
        &Permission("write".into()),
        &issue_ty(),
        &consistency(),
    );
    assert!(
        perm_err.is_err(),
        "a non-read permission is a loud error, never a silent widen"
    );

    let ty_err = authz.list_objects(
        &viewer(),
        &Permission("read".into()),
        &ObjectType("pr".into()),
        &consistency(),
    );
    assert!(
        ty_err.is_err(),
        "a type mismatch is a loud error (the seam carries the board's own type)"
    );
}

#[test]
fn oltp_reference_shares_the_one_lowering() {
    let set_expr = board_set_expr();
    let rows = vec![A.to_string(), B.to_string(), SECRET.to_string()];

    let oltp = oltp_board_admits(&set_expr, &rows, &viewer(), &Zookie("z@0".into()), None).unwrap();
    assert_eq!(
        oltp,
        vec![A.to_string(), B.to_string()],
        "OLTP admits {{A, B}}"
    );

    let acl = board_acl_filter(&set_expr, &viewer(), &Zookie("z@0".into()), None).unwrap();
    for row in &rows {
        assert_eq!(
            acl.admits(row, row),
            oltp.contains(row),
            "the valve's lowered filter admits exactly the OLTP board's visible rows ({row})"
        );
    }
}

#[test]
fn over_budget_decision() {
    let budget = OltpBudget::new(2);
    assert!(!budget.is_over_budget(2), "at budget stays on OLTP");
    assert!(budget.is_over_budget(3), "over budget escalates");
}

#[test]
fn control_canonical_filter_matches_valve() {
    let canonical = AclFilter::ids([A, B]);
    assert!(canonical.admits(A, A) && canonical.admits(B, B) && !canonical.admits(SECRET, SECRET));
}
