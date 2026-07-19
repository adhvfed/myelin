//! # CDC — the Search **query pipeline** (contract 6.1 provider) + the **`list_objects` consumer**
//! (contract 4.3) (SRCH-P08 → P-171).
//!
//! **Architecture:** `search-and-indexing.md` §4.2 / §4.2.1 (the permission-aware query pipeline:
//! `acl ← list_objects → compile(ast) → CONJOIN acl_clause into EVERY branch → engine.search at the
//! posting-list level → rank/fuse`; the bounded-set `Ids/All/None/NotIds` lowering; All → no clause,
//! None → short-circuit empty). Reconciliation `00-reconciliation-decisions.md` OQ-E (the `SetExpr`
//! push-down frozen — `Ids/All/None` are the bounded-set forms lowered here; the relational forms
//! are SRCH-P09).
//!
//! - **PROVIDER (6.1)** = Search's ONE public [`myelin_search::query`] entry — `query(ast, viewer,
//!   zookie?, page) -> RankedResults`, ALWAYS conjoining the OQ-E filter (the
//!   `search-requires-acl-filter` lint, contract 1.6). This test pins the provider's observable
//!   contract: the answer is permission-filtered (a denied doc never surfaces), tenant-confined
//!   (cross-tenant 0, SRCH-D3), and computed with exactly ONE `list_objects` call (no N+1).
//! - **CONSUMER (4.3)** = Search is one of the five named `SetExpr` consumers. It consumes
//!   `list_objects -> Ids{ids,zookie} | Filter{set_expr, zookie}` (NO Id signature change) and lowers
//!   the bounded-set modes (`Ids/All/None/NotIds`) to the engine [`myelin_search::AclFilter`]. This
//!   test pins the consumer side: every bounded-set mode lowers to the expected engine behaviour, and
//!   a RELATIONAL form is a loud floor (SRCH-P09), never a silent widen.
//!
//! The dated green artifact (2026-06-20): the query path conjoins the ACL filter into every branch
//! BEFORE scoring; the bounded-set `Ids/All/None/NotIds` modes lower correctly; cross-tenant results
//! are 0 (SRCH-D3); the query issues exactly one `list_objects` (no N+1); the relational forms are a
//! named floor (SRCH-P09). If the 4.3 `ListObjectsResult`/`SetExpr` shape or the 6.1 query contract
//! drifts, this stops compiling/passing — that is the contract.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_tenancy::TenantId;

use myelin_search::{
    query, FieldDecl, FieldSchema, IndexBackend, IndexDocument, ListObjectsPort, Page, QueryError,
    QueryStats, RelationalLeaf, ReverseIndexAnswer, RevisionWatermark, ScopedEngine,
    TantivyBackend, FT_BODY_FIELD, ORDER_KEY_FIELD,
};

fn var(name: &str) -> Expr {
    Expr::Var(name.into())
}
fn s(v: &str) -> Expr {
    Expr::Lit(Literal::Str(v.into()))
}

fn schema() -> FieldSchema {
    FieldSchema::new()
        .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
        .with("status", FieldDecl::stored(FieldType::Select))
        .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
}

fn facet_decl() -> BTreeMap<String, FieldType> {
    let mut m = BTreeMap::new();
    m.insert("status".to_string(), FieldType::Select);
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

fn doc(id: &str, text: &str) -> IndexDocument {
    let k = OrderKey::bisect(None, None);
    IndexDocument::new(id, text)
        .with_field("status", FieldValue::Select("open".into()))
        .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
}

fn corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    be.upsert(&doc("acme/issue/PUB-1", "shared deadlock note"))
        .unwrap();
    be.upsert(&doc("acme/issue/SECRET-9", "private deadlock note"))
        .unwrap();
    be
}

fn viewer(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p:alice".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn consistency() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn ast() -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(FT_BODY_FIELD),
        rhs: s("deadlock"),
    })
    .expect("within cost bounds")
}

/// A scripted [`ListObjectsPort`] returning a canned answer + counting calls (the 4.3 consumer CDC
/// provider double).
struct ScriptedAuthz {
    answer: ListObjectsResult,
    calls: AtomicU64,
    /// The canned reverse-index JOIN answer (SRCH-P09) — `None` if the test exercises only the
    /// bounded-set path (then a relational leaf fails closed via the default `resolve_relation`).
    reverse: Option<ReverseIndexAnswer>,
    resolve_calls: AtomicU64,
}
impl ScriptedAuthz {
    fn new(answer: ListObjectsResult) -> ScriptedAuthz {
        ScriptedAuthz {
            answer,
            calls: AtomicU64::new(0),
            reverse: None,
            resolve_calls: AtomicU64::new(0),
        }
    }
    fn with_reverse(answer: ListObjectsResult, reverse: ReverseIndexAnswer) -> ScriptedAuthz {
        ScriptedAuthz {
            answer,
            calls: AtomicU64::new(0),
            reverse: Some(reverse),
            resolve_calls: AtomicU64::new(0),
        }
    }
}
impl ListObjectsPort for ScriptedAuthz {
    fn list_objects(
        &self,
        _subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        // **The 4.3 consumer-side contract:** Search asks for `read` over the object type — pin it so
        // an Id-side rename of the permission/type shape breaks this CDC now.
        assert_eq!(
            permission,
            &Permission("read".into()),
            "Search lists objects under `read`"
        );
        assert_eq!(
            ty,
            &ObjectType("issue".into()),
            "the object type is forwarded verbatim"
        );
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.answer.clone())
    }
    fn resolve_relation(
        &self,
        _subject: &Principal,
        _form: &RelationalLeaf,
        _required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        self.resolve_calls.fetch_add(1, Ordering::Relaxed);
        self.reverse
            .clone()
            .ok_or_else(|| myelin_identity::AuthzError::Unavailable("no reverse index".into()))
    }
}

fn run(
    be: &TantivyBackend,
    answer: ListObjectsResult,
    v: &Principal,
) -> (
    Result<myelin_search::RankedResults, QueryError>,
    u64,
    QueryStats,
) {
    let eng = ScopedEngine::new(be, "acme", "eu-west", schema());
    let authz = ScriptedAuthz::new(answer);
    let stats = QueryStats::new();
    let res = query(
        &eng,
        &authz,
        &ast(),
        v,
        &ObjectType("issue".into()),
        &consistency(),
        Page::FIRST,
        &stats,
    );
    (res, authz.calls.load(Ordering::Relaxed), stats)
}

/// **PROVIDER 6.1 + CONSUMER 4.3 (Ids mode): a materialised allow-set surfaces only the visible
/// doc, with exactly ONE list_objects call.**
#[test]
fn cdc_6_1_ids_mode_filters_and_no_n_plus_1() {
    let be = corpus();
    let answer = ListObjectsResult::Ids {
        ids: vec![ObjectId("acme/issue/PUB-1".into())],
        zookie: Zookie("z-ids".into()),
    };
    let (res, calls, stats) = run(&be, answer, &viewer("acme"));
    let res = res.expect("query");
    assert_eq!(
        res.hits
            .iter()
            .map(|h| h.doc_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/issue/PUB-1"],
        "only the allow-set doc surfaces (the confidential one is pre-filtered out)"
    );
    assert_eq!(
        res.zookie, "z-ids",
        "the list_objects zookie is threaded onto the result (6.1)"
    );
    assert_eq!(calls, 1, "EXACTLY one list_objects (no N+1)");
    assert_eq!(stats.list_objects_calls(), 1);
}

/// **CONSUMER 4.3 (Filter{All} mode): admin sees every matching doc (no ACL clause).**
#[test]
fn cdc_4_3_filter_all_mode_admits_all() {
    let be = corpus();
    let (res, calls, _) = run(
        &be,
        ListObjectsResult::Filter {
            set_expr: SetExpr::All,
            zookie: Zookie("z".into()),
        },
        &viewer("acme"),
    );
    let res = res.expect("query");
    let ids: std::collections::BTreeSet<&str> =
        res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        ids.contains("acme/issue/PUB-1") && ids.contains("acme/issue/SECRET-9"),
        "All ⇒ every matching doc surfaces: {ids:?}"
    );
    assert_eq!(calls, 1);
}

/// **CONSUMER 4.3 (Filter{None} mode): short-circuit to empty (no doc surfaces).**
#[test]
fn cdc_4_3_filter_none_mode_short_circuits() {
    let be = corpus();
    let (res, _, stats) = run(
        &be,
        ListObjectsResult::Filter {
            set_expr: SetExpr::None,
            zookie: Zookie("z".into()),
        },
        &viewer("acme"),
    );
    let res = res.expect("query");
    assert!(res.hits.is_empty(), "None ⇒ empty");
    assert_eq!(
        stats.engine_branches(),
        0,
        "the engine is never queried on None (no count leak)"
    );
}

/// **CONSUMER 4.3 (Filter{NotIds} mode): the bounded deny-set hides exactly the denied doc.**
#[test]
fn cdc_4_3_filter_not_ids_mode_denies_bounded() {
    let be = corpus();
    let answer = ListObjectsResult::Filter {
        set_expr: SetExpr::NotIds(vec![ObjectId("acme/issue/SECRET-9".into())]),
        zookie: Zookie("z".into()),
    };
    let (res, _, _) = run(&be, answer, &viewer("acme"));
    let res = res.expect("query");
    assert_eq!(
        res.hits
            .iter()
            .map(|h| h.doc_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/issue/PUB-1"],
        "the denied doc is excluded; the rest surface"
    );
}

/// **PROVIDER 6.1 (SRCH-D3): a cross-tenant viewer is rejected — 0 cross-tenant results, and the
/// authz dependency is never even consulted.**
#[test]
fn cdc_6_1_cross_tenant_zero() {
    let be = corpus();
    let (res, calls, stats) = run(
        &be,
        ListObjectsResult::Filter {
            set_expr: SetExpr::All,
            zookie: Zookie("z".into()),
        },
        &viewer("evil"),
    );
    let err = res.expect_err("a cross-tenant query is rejected (SRCH-D3)");
    assert!(matches!(err, QueryError::TenantMismatch));
    assert_eq!(
        calls, 0,
        "the wrong-tenant query never reaches list_objects"
    );
    assert_eq!(stats.engine_branches(), 0, "0 cross-tenant engine touches");
}

/// **CONSUMER 4.3 (the SRCH-P09 relational reverse-index JOIN): a `TupleSet` form resolves through
/// the per-tenant authz reverse index to the visible-id set (an `Ids` membership clause) — one JOIN,
/// honouring the revision watermark — never a silent widen to All.**
#[test]
fn cdc_4_3_relational_tuple_set_joins_the_reverse_index() {
    use myelin_identity::AuthzIndexRef;
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    // The reverse-index JOIN resolves ONLY PUB-1 at revision 4; the watermark from `z@4` is 4.
    let authz = ScriptedAuthz::with_reverse(
        ListObjectsResult::Filter {
            set_expr: SetExpr::TupleSet {
                index: AuthzIndexRef("authz_visible".into()),
            },
            zookie: Zookie("z@4".into()),
        },
        ReverseIndexAnswer {
            object_ids: vec!["acme/issue/PUB-1".into()],
            revision: RevisionWatermark(4),
        },
    );
    let stats = QueryStats::new();
    let res = query(
        &eng,
        &authz,
        &ast(),
        &viewer("acme"),
        &ObjectType("issue".into()),
        &consistency(),
        Page::FIRST,
        &stats,
    )
    .expect("the relational JOIN resolves");
    assert_eq!(
        res.hits
            .iter()
            .map(|h| h.doc_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/issue/PUB-1"],
        "the JOIN resolves to the visible-id set; the confidential doc never surfaces"
    );
    assert_eq!(
        authz.resolve_calls.load(Ordering::Relaxed),
        1,
        "exactly ONE reverse-index JOIN (no N+1)"
    );
    assert_eq!(
        authz.calls.load(Ordering::Relaxed),
        1,
        "and exactly one list_objects"
    );
}

/// **CONSUMER 4.3 / contract 4.10 (the revision watermark): a reverse-index revision STALER than the
/// `list_objects` watermark is refused (StaleReverseIndex) — never read stale (SRCH-P09; the full
/// fail-static path is SRCH-P10).**
#[test]
fn cdc_4_10_stale_reverse_index_revision_is_refused() {
    use myelin_identity::AuthzIndexRef;
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    // The zookie requires watermark 9; the reverse index serves a stale revision 3.
    let authz = ScriptedAuthz::with_reverse(
        ListObjectsResult::Filter {
            set_expr: SetExpr::TupleSet {
                index: AuthzIndexRef("ix".into()),
            },
            zookie: Zookie("z@9".into()),
        },
        ReverseIndexAnswer {
            object_ids: vec!["acme/issue/PUB-1".into()],
            revision: RevisionWatermark(3),
        },
    );
    let stats = QueryStats::new();
    let res = query(
        &eng,
        &authz,
        &ast(),
        &viewer("acme"),
        &ObjectType("issue".into()),
        &consistency(),
        Page::FIRST,
        &stats,
    );
    let err = res.expect_err("a stale reverse-index revision is refused (4.10)");
    assert!(
        matches!(err, QueryError::StaleReverseIndex { .. }),
        "the stale revision is loud"
    );
    assert!(
        err.to_string().contains("4.10"),
        "the error names the watermark contract"
    );
}
