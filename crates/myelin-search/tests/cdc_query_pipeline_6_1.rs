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

struct ScriptedAuthz {
    answer: ListObjectsResult,
    calls: AtomicU64,
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
    assert!(matches!(err, QueryError::TenantMismatch { .. }));
    assert_eq!(
        calls, 0,
        "the wrong-tenant query never reaches list_objects"
    );
    assert_eq!(stats.engine_branches(), 0, "0 cross-tenant engine touches");
}

#[test]
fn cdc_4_3_relational_tuple_set_joins_the_reverse_index() {
    use myelin_identity::AuthzIndexRef;
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
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

#[test]
fn cdc_4_10_stale_reverse_index_revision_is_refused() {
    use myelin_identity::AuthzIndexRef;
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
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
