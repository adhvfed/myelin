use std::collections::BTreeMap;

use myelin_identity::{
    Consistency, ConsistencyMode, Literal, ObjectId, ObjectType, Principal, PrincipalId,
    PrincipalKind, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_tenancy::TenantId;

use myelin_search::{
    escalate_to_search, oltp_board_admits, AclFilter, BoardQuery, Embedding, FieldDecl,
    FieldSchema, IndexBackend, IndexDocument, OltpBudget, Page, QueryStats, ScopedEngine,
    TantivyBackend, FT_BODY_FIELD, ORDER_KEY_FIELD,
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

fn consistency() -> Consistency {
    Consistency {
        at_least: Zookie("z@0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn issue_ty() -> ObjectType {
    ObjectType("issue".into())
}

fn ast_body(term: &str) -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(FT_BODY_FIELD.into()),
        rhs: Expr::Lit(Literal::Str(term.into())),
    })
    .expect("within cost bounds")
}

const VISIBLE_A: &str = "myelin://acme/issue/issue/ENG-1";
const VISIBLE_B: &str = "myelin://acme/issue/issue/ENG-2";
const CONFIDENTIAL_1: &str = "myelin://acme/issue/issue/ENG-90";
const CONFIDENTIAL_2: &str = "myelin://acme/issue/issue/ENG-91";

fn board_candidate_rows() -> Vec<String> {
    vec![
        VISIBLE_A.to_string(),
        VISIBLE_B.to_string(),
        CONFIDENTIAL_1.to_string(),
        CONFIDENTIAL_2.to_string(),
    ]
}

fn board_corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let k = OrderKey::bisect(None, None);
    let doc = |id: &str, body: &str| {
        IndexDocument::new(id, body)
            .with_field(STATE_FACET, FieldValue::Select("started".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()))
            .with_embedding(Embedding::new(vec![0.5, 0.5, 0.0]), "text-embed@1")
    };
    be.upsert(&doc(VISIBLE_A, "zarquon rollout plan public"))
        .unwrap();
    be.upsert(&doc(VISIBLE_B, "zarquon onboarding public"))
        .unwrap();
    be.upsert(&doc(CONFIDENTIAL_1, "zarquon acquisition classified"))
        .unwrap();
    be.upsert(&doc(CONFIDENTIAL_2, "zarquon merger classified"))
        .unwrap();
    be
}

fn board_set_expr() -> SetExpr {
    SetExpr::Difference(
        Box::new(SetExpr::Ids(vec![
            ObjectId(VISIBLE_A.into()),
            ObjectId(VISIBLE_B.into()),
            ObjectId(CONFIDENTIAL_1.into()),
            ObjectId(CONFIDENTIAL_2.into()),
        ])),
        Box::new(SetExpr::Ids(vec![
            ObjectId(CONFIDENTIAL_1.into()),
            ObjectId(CONFIDENTIAL_2.into()),
        ])),
    )
}

fn valve_visible_sorted(corpus: &TantivyBackend, board: &BoardQuery) -> Vec<String> {
    let eng = ScopedEngine::new(corpus, "acme", "fr-par", schema());
    let stats = QueryStats::new();
    let res = escalate_to_search(
        &eng,
        board,
        &viewer(),
        &issue_ty(),
        &consistency(),
        Page {
            offset: 0,
            limit: 1000,
        },
        &stats,
        None,
    )
    .expect("the valve escalates the over-budget board to Search");
    assert_eq!(
        stats.list_objects_calls(),
        1,
        "the valve carries the board's filter verbatim - exactly ONE list_objects conjoin, no N+1"
    );
    let mut ids: Vec<String> = res.hits.into_iter().map(|h| h.doc_id).collect();
    ids.sort();
    ids
}

fn oltp_visible_sorted(board: &BoardQuery) -> Vec<String> {
    let mut ids = oltp_board_admits(
        &board.set_expr,
        &board_candidate_rows(),
        &viewer(),
        &board.zookie,
        None,
    )
    .expect("the OLTP board applies its ACL pre-filter");
    ids.sort();
    ids
}

#[test]
fn tier3_valve_byte_identical_visible_rows() {
    let budget = OltpBudget::new(2);
    assert!(
        budget.is_over_budget(board_candidate_rows().len()),
        "the board's 4-candidate scan is over the 2-row OLTP budget → the Tier-3 valve fires"
    );

    let corpus = board_corpus();
    let board = BoardQuery::new(ast_body("zarquon"), board_set_expr(), Zookie("z@0".into()));

    let oltp = oltp_visible_sorted(&board);
    let valve = valve_visible_sorted(&corpus, &board);

    assert_eq!(
        oltp, valve,
        "BYTE-IDENTICAL: the OLTP board tier and the Search valve tier admit the IDENTICAL visible \
         rows for the SAME set_expr (no leak divergence between the two ACL pre-filters)"
    );
    assert_eq!(
        valve,
        vec![VISIBLE_A.to_string(), VISIBLE_B.to_string()],
        "both tiers surface exactly the two visible issues"
    );
}

#[test]
fn srch_d1_on_the_valve_path_zero_confidential_leak() {
    let corpus = board_corpus();
    let board = BoardQuery::new(ast_body("zarquon"), board_set_expr(), Zookie("z@0".into()));

    let valve = valve_visible_sorted(&corpus, &board);

    for confidential in [CONFIDENTIAL_1, CONFIDENTIAL_2] {
        assert!(
            !valve.contains(&confidential.to_string()),
            "0 leak: the confidential issue `{confidential}` never surfaces through the valve"
        );
    }
    assert_eq!(
        valve.len(),
        2,
        "0 count-leak: exactly the two visible issues (the confidential ones never counted)"
    );
}

#[test]
fn chained_grant_both_tiers_agree() {
    let corpus = board_corpus();
    let granted_set_expr = SetExpr::Difference(
        Box::new(SetExpr::Ids(vec![
            ObjectId(VISIBLE_A.into()),
            ObjectId(VISIBLE_B.into()),
            ObjectId(CONFIDENTIAL_1.into()),
            ObjectId(CONFIDENTIAL_2.into()),
        ])),
        Box::new(SetExpr::Ids(vec![ObjectId(CONFIDENTIAL_2.into())])),
    );
    let board = BoardQuery::new(ast_body("zarquon"), granted_set_expr, Zookie("z@0".into()));

    let oltp = oltp_visible_sorted(&board);
    let valve = valve_visible_sorted(&corpus, &board);

    assert_eq!(
        oltp, valve,
        "byte-identical under the grant too - the two ACL pre-filters never diverge"
    );
    assert!(
        valve.contains(&CONFIDENTIAL_1.to_string()),
        "the granted issue now surfaces (the rejection was the ACL, not a deny)"
    );
    assert!(
        !valve.contains(&CONFIDENTIAL_2.to_string()),
        "the still-confidential issue stays hidden through both tiers"
    );
    assert_eq!(
        valve,
        vec![
            VISIBLE_A.to_string(),
            VISIBLE_B.to_string(),
            CONFIDENTIAL_1.to_string(),
        ],
        "exactly the two visible + the one granted issue"
    );
}

#[test]
fn relational_board_acl_escalates_byte_identically() {
    use myelin_search::{RelationalLeaf, ReverseIndexAnswer};
    use myelin_search::{ReverseResolver, RevisionWatermark};

    struct BoardReverseIndex {
        visible: Vec<String>,
        revision: u64,
    }
    impl ReverseResolver for BoardReverseIndex {
        fn resolve(
            &self,
            _s: &Principal,
            form: &RelationalLeaf,
            required: &RevisionWatermark,
        ) -> myelin_identity::Result<ReverseIndexAnswer> {
            assert!(
                matches!(form, RelationalLeaf::TupleSet { .. }),
                "the big-result board ACL is a TupleSet JOIN"
            );
            assert!(
                RevisionWatermark(self.revision) >= *required,
                "the reverse index serves a fresh-enough revision (the watermark, §4.2.3)"
            );
            Ok(ReverseIndexAnswer {
                object_ids: self.visible.clone(),
                revision: RevisionWatermark(self.revision),
            })
        }
    }

    let corpus = board_corpus();
    let reverse = BoardReverseIndex {
        visible: vec![VISIBLE_A.to_string(), VISIBLE_B.to_string()],
        revision: 10,
    };
    let relational_set_expr = SetExpr::TupleSet {
        index: myelin_identity::AuthzIndexRef("authz_visible".into()),
    };
    let board = BoardQuery::new(
        ast_body("zarquon"),
        relational_set_expr,
        Zookie("z@10".into()),
    );

    let mut oltp = oltp_board_admits(
        &board.set_expr,
        &board_candidate_rows(),
        &viewer(),
        &board.zookie,
        Some(&reverse),
    )
    .expect("the OLTP board JOINs the reverse index");
    oltp.sort();

    let eng = ScopedEngine::new(&corpus, "acme", "fr-par", schema());
    let stats = QueryStats::new();
    let res = escalate_to_search(
        &eng,
        &board,
        &viewer(),
        &issue_ty(),
        &consistency(),
        Page {
            offset: 0,
            limit: 1000,
        },
        &stats,
        Some(&reverse),
    )
    .expect("the valve escalates the relational board ACL to Search");
    assert_eq!(
        stats.reverse_index_joins(),
        1,
        "exactly ONE reverse-index JOIN per relational leaf (the big-result path is one JOIN, no N+1)"
    );
    let mut valve: Vec<String> = res.hits.into_iter().map(|h| h.doc_id).collect();
    valve.sort();

    assert_eq!(
        oltp, valve,
        "BYTE-IDENTICAL on the relational big-result path too - the SAME reverse-index JOIN feeds \
         both tiers, the SAME lowering composes it"
    );
    assert_eq!(valve, vec![VISIBLE_A.to_string(), VISIBLE_B.to_string()]);
}

#[test]
fn valve_carries_the_boards_own_filter_not_a_recomputation() {
    let corpus = board_corpus();
    let board = BoardQuery::new(ast_body("zarquon"), board_set_expr(), Zookie("z@0".into()));

    let valve = valve_visible_sorted(&corpus, &board);

    let canonical = AclFilter::ids([VISIBLE_A, VISIBLE_B]);
    for row in board_candidate_rows() {
        assert_eq!(
            canonical.admits(&row, &row),
            valve.contains(&row),
            "the valve admits exactly the rows the board's own filter admits ({row})"
        );
    }
}
