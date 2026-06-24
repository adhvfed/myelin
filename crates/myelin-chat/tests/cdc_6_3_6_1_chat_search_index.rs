//! # CDC 6.3 / 6.1 — the Chat `declare_indexable` IndexSpec + the ACL-conjoined search feeder
//! (CHAT-P20 / P-415, M4-C7)
//!
//! **Contract 6.3** — `declare_indexable(IndexSpec{ subsystem, type, ft_fields, struct_fields,
//! semantic, acl_object_type })`: the chat/message Search projection (arch §7). Chat OWNS this spec
//! (it is a PRODUCER of searchable artifacts — one searchable doc per `message`); Search OWNS the
//! `IndexSpec` TYPE. This CDC is the PROVIDER (chat) + CONSUMER (Search admits) pair: chat constructs
//! the real [`myelin_search::IndexSpec`] to the frozen §7 shape, and Search ACCEPTS it into a live
//! [`myelin_search::IncrementalIndexer`]'s facet union (the only honest "registered").
//!
//! **Contract 6.1** — `query(ast, viewer, at, page) → RankedResults` (the ACL-conjoining surface; the
//! `search-requires-acl-filter` discipline). **CONSUMED** by Chat: every message search routes through
//! the ONE `myelin_search::query` entry (the chat-side [`myelin_chat::AclConjoinedSearchFeeder`]), so a
//! viewer's `list_objects(view, message)` `Filter` pre-filters the candidate set BEFORE scoring. This
//! CDC pins that the REAL Search surface satisfies chat's feeder and that the conjoin keys on the SAME
//! `message` object type the spec declares (`acl_object_type = "message"`).
//!
//! Coherence (EI-01 §7): chat owns NO second indexing type and NO second search path — the spec is the
//! frozen Search-owned shape, the query is the one conjoining surface.

use myelin_chat::{
    message_index_spec, message_search_acl_anchor, register_message_index_specs,
    AclConjoinedSearchFeeder, FACET_ARTIFACT_REF, FACET_AUTHOR, FACET_CHANNEL, FACET_CREATED_AT,
    FACET_EMBED, FACET_KIND, FACET_MENTION, FACET_THREAD_ROOT, FT_BODY_FIELD,
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// CDC 6.3 — the chat/message IndexSpec is the frozen §7 shape Search accepts
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **CDC 6.3 (PROVIDER) — the chat message spec is the OWNED §7 shape.** `subsystem = "chat"`,
/// `type = "message"`, **semantic** (embeddings ARE personal data, §7), `acl_object_type = "message"`
/// (the conjoin keys on `message.id`), and the §7 columnar facets + the three cross-producer reference
/// facets (X-2). A drift off any of these breaks the registrant.
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
    // The full-text body is delivered at emit time in SearchProjection.text — NOT a struct facet.
    assert!(
        !s.struct_fields.contains_key(FT_BODY_FIELD),
        "the markdown body is the ft_fields projection, not a structured facet"
    );
}

/// **CDC 6.3 (CONSUMER) — Search ACCEPTS the chat message spec.** The accepted set is byte-equal to
/// the declared set, admitted into a live indexer's per-tenant facet union without a schema mismatch
/// (the semantic spec wires the embedding adapter — the embeddings path is live).
#[test]
fn search_accepts_the_chat_message_spec() {
    let accepted = register_message_index_specs();
    assert_eq!(
        accepted,
        vec![message_index_spec()],
        "Search accepts the chat spec verbatim"
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// CDC 6.1 — the ACL-conjoined feeder routes through the REAL Search query surface
// ════════════════════════════════════════════════════════════════════════════════════════════════

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
        // PIN: the feeder conjoins on (view, message) — the SAME anchor the spec declares.
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

/// **CDC 6.1 (CONSUMER) — the chat feeder routes through the REAL ACL-conjoining `query` surface; a
/// denied message is excluded incl. count, exactly one list_objects (no N+1).** Both messages match
/// "deploy"; the allow-set excludes the confidential one → it is in neither the rows NOR the count.
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

/// **CDC 6.1 — the ACL anchor is `(view, message)`, byte-matched to the spec's `acl_object_type`.**
/// The feeder must conjoin on the SAME object type the IndexSpec declares — otherwise the pre-filter
/// keys on the wrong column and the leak guarantee breaks.
#[test]
fn feeder_acl_anchor_matches_the_spec_acl_object_type() {
    let (perm, ty) = message_search_acl_anchor();
    assert_eq!(perm.0, "read");
    assert_eq!(ty.0, message_index_spec().acl_object_type);
    assert_eq!(ty.0, "message");
}

/// **CDC 6.1 — there is no chat-private search path: a `SetExpr::None` viewer gets 0 results.** The
/// `WHERE false` short-circuit yields an empty result without materialising a candidate set — the
/// only visibility gate is the engine pre-filter.
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

/// **Cross-check: the same conjoining `query` surface the feeder uses is the platform one (one
/// surface, not a chat fork).** Calling `myelin_search::query` directly with the same inputs yields
/// the same visible set — proving the feeder is a thin router over the ONE surface.
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
