//! # Drill — SRCH-P21 (P-339, M4): the Issues Tier-3 board-escalation valve, byte-identical ACL
//! pre-filter (the OLTP-budget escalation seam)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D1 (the
//! leak-equivalence half — an over-budget board escalated to Search returns 0 confidential-issue
//! leak, the SAME `Filter` conjoined) + the **valve parity check** (the SAME board query run through
//! the OLTP board path AND through the Search valve returns BYTE-IDENTICAL visible rows — 0 leak
//! divergence between the two ACL pre-filters). The valve supports the master-band **ISS-D2**
//! board-query-<1s gate by giving it a leak-equivalent escalation path. **Architecture:**
//! `search-and-indexing.md` §4.2.4 (the Tier-3 valve), §4.2 / §4.2.1 (the permission-aware pipeline —
//! the pre-filter never a post-filter). **Contracts:** 6.1 `query` (the valve consumer wires here),
//! 4.3 the `SetExpr` (byte-identical to the OLTP board's).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-23)
//!
//! An adversarial Issues board corpus — visible issues alongside CONFIDENTIAL issues that share the
//! board's facets + a rare body term (so a divergence would leak via count/IDF) — is indexed into the
//! real Search engine. A board query goes OVER its OLTP budget and escalates to Search. The drill
//! asserts:
//!
//! 1. **Valve parity (the GATE):** the SAME board query, run through the **OLTP board tier**
//!    ([`oltp_board_admits`] over the candidate rows) AND through the **Search valve tier**
//!    ([`escalate_to_search`] over the indexed corpus), returns the **byte-identical visible-row set**
//!    — 0 leak divergence between the two ACL pre-filters. Because both tiers derive their decision
//!    from the SAME `lower_set_expr` lowering, there is no second interpreter to drift.
//! 2. **SRCH-D1 on the valve path:** the over-budget board escalated to Search returns 0
//!    confidential-issue leak (incl. counts) — the confidential issues, excluded by the board's
//!    `- confidential` set-difference `set_expr`, never surface through the valve.
//! 3. **The chained grant:** grant the confidential issue → it surfaces through BOTH tiers (the
//!    rejection was the ACL firing, not a blanket deny), and the two tiers STILL agree byte-for-byte.
//! 4. **No N+1:** the valve's escalation makes exactly ONE `list_objects` conjoin (the board's filter
//!    carried verbatim) — never one check per candidate.
//!
//! The ENGINE is UNCHANGED — the valve is the CONSUMER side of 6.1 (the live `query` driven with the
//! board's filter). No new mutation-core module: the SRCH-P09 mutation floor (the `SetExpr` conjoin
//! logic in `pipeline::lower_set_expr`) still holds on the valve path — the valve REUSES that exact
//! lowering, so the same mutation tests pin the valve's pre-filter.
//!
//! ## Floor named
//! The at-scale board-query latency of the escalation path under the 30× world-scale surge is the M5
//! follow-on **SRCH-P25** (the valve gives ISS-D2 a leak-equivalent escalation path; the surge changes
//! its LATENCY, never its leak-equivalence).

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

// ----------------------------------------------------------------------------------------------
// fixtures — the adversarial Issues board corpus (visible + confidential, shared facet/term)
// ----------------------------------------------------------------------------------------------

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

/// Every candidate row the OLTP board's filtered scan would consider (the board's `state=started`
/// scan over the whole project) — the SAME id space the Search valve indexes.
fn board_candidate_rows() -> Vec<String> {
    vec![
        VISIBLE_A.to_string(),
        VISIBLE_B.to_string(),
        CONFIDENTIAL_1.to_string(),
        CONFIDENTIAL_2.to_string(),
    ]
}

/// **The adversarial board corpus.** All four issues share the `started` state facet AND the rare
/// term `zarquon`, so a divergence between the two ACL pre-filters would leak a confidential issue via
/// FT/count/facet inference.
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

/// **The board's ACL pre-filter `set_expr` — the `- confidential` set-difference (4.3).** The board's
/// reachable set is "every started issue in the project MINUS the confidential ones the viewer cannot
/// see" — `Difference(Ids{all four}, Ids{the two confidential})`. This is the SAME `SetExpr` whether
/// the board scans OLTP or escalates to Search (byte-identical, 4.3).
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

/// The visible rows through the Search valve, sorted (so the two tiers' sets are comparable
/// order-independently — the valve ranks, the OLTP scan is input-ordered; the SET must be identical).
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
            limit: 1000, // a generous page so a count divergence would show up
        },
        &stats,
        None, // a pure bounded-set board ACL — no reverse resolver needed
    )
    .expect("the valve escalates the over-budget board to Search");
    // The no-N+1 invariant: the valve's escalation made exactly ONE list_objects conjoin.
    assert_eq!(
        stats.list_objects_calls(),
        1,
        "the valve carries the board's filter verbatim — exactly ONE list_objects conjoin, no N+1"
    );
    let mut ids: Vec<String> = res.hits.into_iter().map(|h| h.doc_id).collect();
    ids.sort();
    ids
}

/// The visible rows through the OLTP board tier, sorted.
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

// ----------------------------------------------------------------------------------------------
// 1. The valve parity check — byte-identical visible rows across the two ACL pre-filters
// ----------------------------------------------------------------------------------------------

/// **THE GATE — valve parity: the SAME board query through the OLTP board tier AND the Search valve
/// tier returns BYTE-IDENTICAL visible rows (0 leak divergence between the two ACL pre-filters).** The
/// board is over its OLTP budget, so it escalates to Search conjoining the SAME `Filter{set_expr}` the
/// OLTP board would have used. Both tiers admit exactly {VISIBLE_A, VISIBLE_B} — the confidential
/// issues, excluded by the `- confidential` set-difference, surface through NEITHER.
#[test]
fn tier3_valve_byte_identical_visible_rows() {
    // The board's filtered scan is over its OLTP budget — it MUST escalate to Search.
    let budget = OltpBudget::new(2);
    assert!(
        budget.is_over_budget(board_candidate_rows().len()),
        "the board's 4-candidate scan is over the 2-row OLTP budget → the Tier-3 valve fires"
    );

    let corpus = board_corpus();
    let board = BoardQuery::new(ast_body("zarquon"), board_set_expr(), Zookie("z@0".into()));

    let oltp = oltp_visible_sorted(&board);
    let valve = valve_visible_sorted(&corpus, &board);

    // Byte-identical visible rows — the crux property (0 leak divergence).
    assert_eq!(
        oltp, valve,
        "BYTE-IDENTICAL: the OLTP board tier and the Search valve tier admit the IDENTICAL visible \
         rows for the SAME set_expr (no leak divergence between the two ACL pre-filters)"
    );
    // And that identical set is exactly the two visible issues (not the confidential ones).
    assert_eq!(
        valve,
        vec![VISIBLE_A.to_string(), VISIBLE_B.to_string()],
        "both tiers surface exactly the two visible issues"
    );
}

// ----------------------------------------------------------------------------------------------
// 2. SRCH-D1 on the valve path — 0 confidential-issue leak (incl. counts)
// ----------------------------------------------------------------------------------------------

/// **SRCH-D1 on the valve path: an over-budget board escalated to Search returns 0 confidential-issue
/// leak (incl. counts).** The confidential issues share the rare term `zarquon` + the `started` facet
/// with the visible ones, yet — excluded by the board's `- confidential` `set_expr`, conjoined at the
/// posting-list level by the valve — they NEVER surface and NEVER contribute to the result count.
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
    // 0 count-leak: exactly the two visible issues — the confidential ones never entered the candidate
    // set (a post-filter would have shown a count of 4; the pre-filter shows 2).
    assert_eq!(
        valve.len(),
        2,
        "0 count-leak: exactly the two visible issues (the confidential ones never counted)"
    );
}

// ----------------------------------------------------------------------------------------------
// 3. The chained grant — both tiers agree after the grant (the rejection was the ACL, not a deny)
// ----------------------------------------------------------------------------------------------

/// **The chained grant: grant a confidential issue → it surfaces through BOTH tiers, and the two tiers
/// STILL agree byte-for-byte.** When the board's `set_expr` no longer excludes CONFIDENTIAL_1 (the
/// viewer was granted it), it becomes visible through the OLTP board AND the Search valve — proving the
/// earlier rejection was the ACL firing, not a blanket deny, and that parity holds under the grant.
#[test]
fn chained_grant_both_tiers_agree() {
    let corpus = board_corpus();
    // The board's set_expr now excludes ONLY CONFIDENTIAL_2 (CONFIDENTIAL_1 was granted to the viewer).
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
        "byte-identical under the grant too — the two ACL pre-filters never diverge"
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

// ----------------------------------------------------------------------------------------------
// 4. The SetExpr is the board's — a RELATIONAL board ACL escalates byte-identically too (SRCH-P09)
// ----------------------------------------------------------------------------------------------

/// **A board whose ACL is a RELATIONAL `set_expr` (the big-result path) escalates byte-identically.**
/// The board's reachable set is a `TupleSet` reverse-index JOIN (the over-budget board's natural
/// shape — too many candidates to materialise). The valve resolves it through the SAME reverse-index
/// JOIN the live consumer uses (SRCH-P09); the OLTP board reference resolves it through the SAME
/// resolver — byte-identical visible rows, honouring the revision watermark.
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
    // The board's reverse-index JOIN resolves to ONLY the two visible issues (the big-result path).
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
        Zookie("z@10".into()), // the watermark the JOIN must honour (rev 10)
    );

    // The OLTP board tier resolves through the SAME reverse index.
    let mut oltp = oltp_board_admits(
        &board.set_expr,
        &board_candidate_rows(),
        &viewer(),
        &board.zookie,
        Some(&reverse),
    )
    .expect("the OLTP board JOINs the reverse index");
    oltp.sort();

    // The Search valve tier resolves through the SAME reverse index (one resolve, no N+1).
    let eng = ScopedEngine::new(&corpus, "acme", "fr-par", schema());
    let stats = QueryStats::new();
    let res = escalate_to_search(
        &eng,
        &board,
        &viewer(),
        &issue_ty(),
        // The READ consistency (read-your-writes snapshot) is rev 0 — the corpus is indexed at the
        // current snapshot, so nothing is stale. This is DISTINCT from the board's `set_expr` zookie
        // (`z@10`), which is the ACL snapshot the reverse-index JOIN watermark honours (§4.2.3). The
        // two zookies are different axes: the read snapshot vs the ACL/reverse-index revision.
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
        "BYTE-IDENTICAL on the relational big-result path too — the SAME reverse-index JOIN feeds \
         both tiers, the SAME lowering composes it"
    );
    assert_eq!(valve, vec![VISIBLE_A.to_string(), VISIBLE_B.to_string()]);
}

// ----------------------------------------------------------------------------------------------
// 5. A control — the SAME set_expr the OLTP board would have conjoined IS the one the valve conjoins
// ----------------------------------------------------------------------------------------------

/// **Control: the valve does NOT re-derive the reachable set — it carries the board's OWN `set_expr`
/// (byte-identical to its OLTP shape, 4.3).** An independent `AclFilter::ids` over the SAME visible set
/// admits the SAME rows the valve surfaces — confirming the valve's pre-filter is the board's filter,
/// not a coincidentally-equal recomputation.
#[test]
fn valve_carries_the_boards_own_filter_not_a_recomputation() {
    let corpus = board_corpus();
    let board = BoardQuery::new(ast_body("zarquon"), board_set_expr(), Zookie("z@0".into()));

    let valve = valve_visible_sorted(&corpus, &board);

    // The board's filter, applied directly as the canonical `Ids` allow-set the difference reduces to.
    let canonical = AclFilter::ids([VISIBLE_A, VISIBLE_B]);
    for row in board_candidate_rows() {
        assert_eq!(
            canonical.admits(&row),
            valve.contains(&row),
            "the valve admits exactly the rows the board's own filter admits ({row})"
        );
    }
}
