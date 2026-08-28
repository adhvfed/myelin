use myelin_chat::{
    message_index_spec, message_search_acl_anchor, AclConjoinedSearchFeeder, FACET_ARTIFACT_REF,
    FACET_AUTHOR, FACET_CHANNEL, FACET_CREATED_AT, FACET_EMBED, FACET_KIND, FACET_MENTION,
    FACET_THREAD_ROOT, FT_BODY_FIELD,
};
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, Predicate, QueryAst};
use myelin_search::{
    query as search_query, FieldDecl, FieldSchema, IndexBackend, IndexDocument, ListObjectsPort,
    Page, QueryStats, ScopedEngine, TantivyBackend,
};
use myelin_tenancy::TenantId;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-alice".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn consistency() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

#[test]
fn chat_message_spec_is_the_frozen_6_3_shape() {
    let s = message_index_spec();
    assert_eq!(s.subsystem, "chat");
    assert_eq!(s.type_, "message");
    assert_eq!(
        s.acl_object_type, "message",
        "§7: Search ALWAYS conjoins the list_objects Filter over message.id"
    );
    assert!(
        s.semantic,
        "§7: message bodies are RAG-embedded (embeddings ARE personal data)"
    );

    for facet in [
        FACET_CHANNEL,
        FACET_AUTHOR,
        FACET_THREAD_ROOT,
        FACET_CREATED_AT,
        FACET_KIND,
    ] {
        assert!(
            s.struct_fields.contains_key(facet),
            "§7 struct_field `{facet}` present"
        );
    }
    for facet in [FACET_MENTION, FACET_ARTIFACT_REF, FACET_EMBED] {
        assert_eq!(
            s.struct_fields.get(facet),
            Some(&FieldType::Relation),
            "`{facet}` is a dependable cross-producer reference facet (X-2)"
        );
    }
    assert!(
        !s.struct_fields.contains_key(FT_BODY_FIELD),
        "the markdown body is the ft_fields projection, not a structured facet"
    );
}

fn schema() -> FieldSchema {
    FieldSchema::new().with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
}

fn corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&BTreeMap::new()).expect("open");
    for (id, body) in [
        (
            "myelin://acme/chat/message/m-public",
            "deploy the public service",
        ),
        (
            "myelin://acme/chat/message/m-secret",
            "deploy the confidential incident fix",
        ),
    ] {
        be.upsert(&IndexDocument::new(id, body)).unwrap();
    }
    be
}

struct FakeAuthz {
    answer: ListObjectsResult,
    calls: AtomicU64,
}
impl FakeAuthz {
    fn ids(ids: &[&str]) -> FakeAuthz {
        FakeAuthz {
            answer: ListObjectsResult::Ids {
                ids: ids.iter().map(|i| ObjectId((*i).into())).collect(),
                zookie: Zookie("z-acl".into()),
            },
            calls: AtomicU64::new(0),
        }
    }
}
impl ListObjectsPort for FakeAuthz {
    fn list_objects(
        &self,
        _subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        assert_eq!(permission.0, "read");
        assert_eq!(ty.0, "message");
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.answer.clone())
    }
}

fn ast(term: &str) -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(FT_BODY_FIELD.into()),
        rhs: Expr::Lit(Literal::Str(term.into())),
    })
    .expect("within cost bounds")
}

#[test]
fn chat_feeder_is_acl_conjoined_through_the_real_query_surface() {
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let authz = FakeAuthz::ids(&["myelin://acme/chat/message/m-public"]);
    let feeder = AclConjoinedSearchFeeder::new(&eng, &authz);
    let stats = QueryStats::new();

    let res = feeder
        .search_messages(
            &ast("deploy"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("the real Search query surface is reachable");

    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(
        ids,
        ["myelin://acme/chat/message/m-public"],
        "the confidential message is excluded by the Search pre-filter (0 leak)"
    );
    assert_eq!(
        res.hits.len(),
        1,
        "the count reveals only the visible message"
    );
    assert_eq!(
        authz.calls.load(Ordering::Relaxed),
        1,
        "exactly ONE list_objects (the conjoined pre-filter; no N+1)"
    );
}

#[test]
fn feeder_acl_anchor_matches_the_spec_acl_object_type() {
    let (perm, ty) = message_search_acl_anchor();
    assert_eq!(perm.0, "read");
    assert_eq!(ty.0, message_index_spec().acl_object_type);
    assert_eq!(ty.0, "message");
}

#[test]
fn non_member_setexpr_none_yields_zero() {
    struct NoneAuthz;
    impl ListObjectsPort for NoneAuthz {
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _a: &Consistency,
        ) -> AuthzResult<ListObjectsResult> {
            Ok(ListObjectsResult::Filter {
                set_expr: SetExpr::None,
                zookie: Zookie("z".into()),
            })
        }
    }
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let none = NoneAuthz;
    let feeder = AclConjoinedSearchFeeder::new(&eng, &none);
    let stats = QueryStats::new();
    let res = feeder
        .search_messages(
            &ast("deploy"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("reachable");
    assert!(
        res.hits.is_empty(),
        "a non-member sees 0 message results (0 count leak)"
    );
}

#[test]
fn feeder_is_a_thin_router_over_the_one_query_surface() {
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let authz = FakeAuthz::ids(&["myelin://acme/chat/message/m-public"]);
    let ty = ObjectType("message".into());
    let stats = QueryStats::new();
    let direct = search_query(
        &eng,
        &authz,
        &ast("deploy"),
        &viewer(),
        &ty,
        &consistency(),
        Page::FIRST,
        &stats,
    )
    .expect("reachable");
    let direct_ids: Vec<&str> = direct.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(direct_ids, ["myelin://acme/chat/message/m-public"]);
}
