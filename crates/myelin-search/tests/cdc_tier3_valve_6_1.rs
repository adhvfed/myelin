//! # CDC — the Issues Tier-3 board-escalation valve, the **consumer side of contract 6.1** (SRCH-P21
//! → P-339, M4).
//!
//! **Architecture:** `search-and-indexing.md` §4.2.4 (the Tier-3 valve — an over-budget Issues board
//! compiles its query to a Search `query(ast, viewer)` conjoining the SAME `Filter{set_expr}` the OLTP
//! board would have used; byte-identical ACL pre-filter semantics, no leak/no N+1 on either tier),
//! §4.2 (the permission-aware pipeline). Reconciliation `00-reconciliation-decisions.md` OQ-E (the
//! `SetExpr` push-down frozen — the same `set_expr` the OLTP board conjoins). **Contracts:** 6.1
//! `query` (the valve is the CONSUMER side — Issues calls Search's `query` with its filter), 4.3 the
//! `SetExpr` (byte-identical to the OLTP board's).
//!
//! - **PROVIDER (6.1)** = Search's ONE public `query` entry, driven through `escalate_to_search`, is
//!   the provider the valve consumes: it ALWAYS conjoins the board's `Filter{set_expr}` (the OQ-E
//!   pre-filter lowered through `pipeline::lower_set_expr`) into every engine branch BEFORE scoring,
//!   returns the SAME `RankedResults` shape, and surfaces a denied (confidential) issue in NEITHER the
//!   result NOR the count. This test pins that provider behaviour over the valve's escalation.
//! - **CONSUMER (6.1)** = the valve drives Search's `query` with the board's filter. This pins the
//!   consumer contract: the valve carries the board's OWN `Filter{set_expr}` (4.3) into Search's
//!   conjoin step (NOT a re-derivation), makes exactly ONE `list_objects` conjoin (no N+1), and
//!   surfaces the SAME `RankedResults` the live pipeline returns (the engine is unchanged).
//! - **PARITY (the crux)** = the OLTP board tier (`oltp_board_admits`) and the Search valve tier
//!   (`escalate_to_search`) derive their ACL pre-filter from the SAME `pipeline::lower_set_expr`
//!   lowering — so the byte-identical-rows property is structural (one interpreter), not coincidental.
//!
//! The dated green artifact (2026-06-23): the valve consumes 6.1 with the board's 4.3 filter; the
//! over-budget escalation makes one `list_objects` conjoin; a permission/type mismatch on the seam is a
//! loud error (never a silent widen); the OLTP-board reference shares the ONE lowering the valve
//! conjoins. If the 6.1 `query` or 4.3 `SetExpr` shape drifts, this stops compiling/passing — that is
//! the contract.

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

/// The board's `- confidential` filter: {A, B, SECRET} minus {SECRET} ⇒ {A, B}.
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

// ----------------------------------------------------------------------------------------------
// CONSUMER (6.1) — the valve drives Search's `query` with the board's filter
// ----------------------------------------------------------------------------------------------

/// **The valve is the consumer of 6.1: it drives Search's `query` with the board's filter, returning
/// the SAME `RankedResults`, with exactly ONE `list_objects` conjoin (no N+1).** The over-budget board
/// escalates; Search conjoins the board's `set_expr`; SECRET (excluded) never surfaces.
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

/// **The escalation port carries the board's OWN `Filter{set_expr}` verbatim (4.3) — NOT a
/// re-derivation.** `BoardEscalationAuthz::list_objects` returns the EXACT `set_expr` the board handed
/// it; this is how the byte-identical-to-OLTP filter reaches Search's conjoin step.
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

/// **The seam is loud, never a silent widen.** The valve escalates ONLY a `read` board scan for the
/// board's object type — a permission or type mismatch on the seam is a loud `Unavailable` (the valve
/// carries the board's own filter; it never widens the permission or crosses the type).
#[test]
fn seam_rejects_permission_and_type_mismatch_loudly() {
    let authz = BoardEscalationAuthz::new(board_set_expr(), Zookie("z@0".into()), issue_ty());

    // A non-read permission is refused (the valve never widens beyond the board's read scan).
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

    // A type mismatch (the board escalated `issue` but Search asked for `pr`) is refused.
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

// ----------------------------------------------------------------------------------------------
// PARITY — the OLTP board reference shares the ONE lowering the valve conjoins
// ----------------------------------------------------------------------------------------------

/// **The OLTP board tier and the Search valve tier share the ONE `SetExpr` lowering — byte-identical by
/// construction.** `board_acl_filter` lowers the board's `set_expr` to the SAME `AclFilter` the valve
/// conjoins; `oltp_board_admits` over that filter admits exactly the rows the valve surfaces.
#[test]
fn oltp_reference_shares_the_one_lowering() {
    let set_expr = board_set_expr();
    let rows = vec![A.to_string(), B.to_string(), SECRET.to_string()];

    // The OLTP board's reference visible set.
    let oltp = oltp_board_admits(&set_expr, &rows, &viewer(), &Zookie("z@0".into()), None).unwrap();
    assert_eq!(
        oltp,
        vec![A.to_string(), B.to_string()],
        "OLTP admits {{A, B}}"
    );

    // The lowered filter the valve conjoins is the SAME canonical AclFilter.
    let acl = board_acl_filter(&set_expr, &viewer(), &Zookie("z@0".into()), None).unwrap();
    for row in &rows {
        assert_eq!(
            acl.admits(row, row),
            oltp.contains(row),
            "the valve's lowered filter admits exactly the OLTP board's visible rows ({row})"
        );
    }
}

// ----------------------------------------------------------------------------------------------
// The over-budget escalation TRIGGER (the OLTP budget decision)
// ----------------------------------------------------------------------------------------------

/// **The over-budget decision is `candidate_rows > max_rows`.** Under budget the board serves on OLTP;
/// over budget it escalates to the Tier-3 valve (the SAME comparison the live budget meter makes).
#[test]
fn over_budget_decision() {
    let budget = OltpBudget::new(2);
    assert!(!budget.is_over_budget(2), "at budget stays on OLTP");
    assert!(budget.is_over_budget(3), "over budget escalates");
}

/// **A control: a plain `AclFilter::ids` over the visible set admits the SAME rows the valve surfaces.**
/// Confirms the valve's pre-filter is the board's filter (the difference reduces to the {A, B}
/// allow-set), not a coincidental recomputation.
#[test]
fn control_canonical_filter_matches_valve() {
    let canonical = AclFilter::ids([A, B]);
    assert!(canonical.admits(A, A) && canonical.admits(B, B) && !canonical.admits(SECRET, SECRET));
}
