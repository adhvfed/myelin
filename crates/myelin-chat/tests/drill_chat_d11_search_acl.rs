//! # CHAT-D11 — search as a non-member → 0 results from channels you're not in (CHAT-P20 / P-415, M4-C7)
//!
//! **Drill catalogue** row **CHAT-D11**: "Search as a non-member → 0 results from channels you're not
//! in; the `search-requires-acl-filter` lint fails ANY query path reaching the index without the
//! Filter conjoined." **Thresholds:** the non-member-result signal = 0; the lint signal = 0 unfiltered
//! query paths.
//!
//! This drill proves the chat search no-leak PROPERTY structurally over the chat-owned ACL-conjoined
//! feeder ([`myelin_chat::AclConjoinedSearchFeeder`]) and the REAL `myelin_search::query` surface (the
//! ONE conjoining entry — there is no chat-private message index, no post-filter):
//! 1. **The non-member sees 0 results** — a viewer in NO channel (`SetExpr::None`) gets 0 message
//!    results, not in the rows AND not in the count, even though every message matches the term.
//! 2. **The chained leak (EI-01 §4)** — grant a member → they see the channel's messages → REVOKE
//!    membership → re-query → 0 results again (the pre-filter is the live `list_objects`, not a
//!    cached allow-set).
//! 3. **The HYOK structural skip (11.3)** — a HYOK tenant produces 0 indexed message bodies (you
//!    cannot index what you cannot decrypt), so there is nothing to leak.
//!
//! Architecture: `chat/architecture/03-events-contracts-and-glue.md` §7 (Search ALWAYS conjoins the
//! frozen `list_objects` Filter over `message.id`; the search-as-non-member = 0 drill). The drill uses
//! a deterministic `ListObjectsPort` fake modelling the per-viewer allow-set (the SAME fake shape the
//! Search/composer CDCs use — chat never authors a second authz path); the conjoin is the REAL Search
//! engine pre-filter.

use myelin_chat::{
    admit_message_indexing, may_index_messages, AclConjoinedSearchFeeder, FT_BODY_FIELD,
};
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, Predicate, QueryAst};
use myelin_search::{
    FieldDecl, FieldSchema, IndexBackend, IndexDocument, ListObjectsPort, Page, QueryStats,
    ScopedEngine, TantivyBackend,
};
use myelin_storage::{
    kms::{DekHandle, KekId, KmsEngine, KEY_LEN},
    Byok, Dek, Hyok, HyokKeyService, HyokServiceDenied, IndexAdmission, PlatformManaged,
    WrappedDek,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::BTreeMap;

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-bob".into()),
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

fn schema() -> FieldSchema {
    FieldSchema::new().with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
}

/// A corpus of messages, all matching "incident" — so visibility is decided ONLY by the ACL filter,
/// never by relevance.
fn corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&BTreeMap::new()).expect("open");
    for (id, body) in [
        (
            "myelin://acme/chat/message/m-private-1",
            "incident bridge in #ops",
        ),
        (
            "myelin://acme/chat/message/m-private-2",
            "incident postmortem in #ops",
        ),
    ] {
        be.upsert(&IndexDocument::new(id, body)).unwrap();
    }
    be
}

/// A re-armable per-viewer allow-set (the live `list_objects` answer) — a grant then a revoke flips
/// the SAME port's answer, so the drill proves the pre-filter reads the LIVE membership, not a cache.
struct LiveAuthz {
    allow: RefCell<ListObjectsResult>,
}
impl LiveAuthz {
    fn non_member() -> LiveAuthz {
        LiveAuthz {
            allow: RefCell::new(ListObjectsResult::Filter {
                set_expr: SetExpr::None,
                zookie: Zookie("z".into()),
            }),
        }
    }
    fn grant(&self, ids: &[&str]) {
        *self.allow.borrow_mut() = ListObjectsResult::Ids {
            ids: ids.iter().map(|i| ObjectId((*i).into())).collect(),
            zookie: Zookie("z".into()),
        };
    }
    fn revoke(&self) {
        *self.allow.borrow_mut() = ListObjectsResult::Filter {
            set_expr: SetExpr::None,
            zookie: Zookie("z".into()),
        };
    }
}
impl ListObjectsPort for LiveAuthz {
    fn list_objects(
        &self,
        _s: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        _a: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        assert_eq!(
            permission.0, "read",
            "the message search lists under the frozen Search `read` permission"
        );
        assert_eq!(
            ty.0, "message",
            "the conjoin keys on the `message` object (message.id)"
        );
        Ok(self.allow.borrow().clone())
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

/// **CHAT-D11 — search as a non-member → 0 results (the headline threshold = 0).** A viewer in no
/// channel sees 0 messages even though both match the term — not in the rows, not in the count. Then a
/// grant surfaces them, then a REVOKE returns to 0 (the pre-filter reads live membership).
#[test]
fn search_as_non_member_returns_zero_then_grant_then_revoke() {
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let authz = LiveAuthz::non_member();
    let feeder = AclConjoinedSearchFeeder::new(&eng, &authz);

    // 1. NON-MEMBER → 0 results (the headline).
    let r0 = feeder
        .search_messages(
            &ast("incident"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &QueryStats::new(),
        )
        .expect("reachable");
    assert_eq!(
        r0.hits.len(),
        0,
        "CHAT-D11: a non-member sees 0 message results from channels they're not in (incl. count)"
    );

    // 2. GRANT both → both surface on the SAME surface.
    authz.grant(&[
        "myelin://acme/chat/message/m-private-1",
        "myelin://acme/chat/message/m-private-2",
    ]);
    let r1 = feeder
        .search_messages(
            &ast("incident"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &QueryStats::new(),
        )
        .expect("reachable");
    assert_eq!(
        r1.hits.len(),
        2,
        "after grant the member sees the channel's messages"
    );

    // 3. REVOKE → back to 0 (the chained no-leak; the pre-filter is the live list_objects).
    authz.revoke();
    let r2 = feeder
        .search_messages(
            &ast("incident"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &QueryStats::new(),
        )
        .expect("reachable");
    assert_eq!(
        r2.hits.len(),
        0,
        "CHAT-D11 chained: post-revoke the non-member sees 0 again (live membership, not a cache)"
    );
}

/// **CHAT-D11 partial visibility — a viewer in ONE of two channels sees only that channel's message.**
/// The grant covers only `m-private-1`; the other is excluded incl. count (no rank/existence leak).
#[test]
fn partial_membership_sees_only_granted_messages() {
    let be = corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let authz = LiveAuthz::non_member();
    authz.grant(&["myelin://acme/chat/message/m-private-1"]);
    let feeder = AclConjoinedSearchFeeder::new(&eng, &authz);
    let res = feeder
        .search_messages(
            &ast("incident"),
            &viewer(),
            &consistency(),
            Page::FIRST,
            &QueryStats::new(),
        )
        .expect("reachable");
    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(ids, ["myelin://acme/chat/message/m-private-1"]);
    assert_eq!(
        res.hits.len(),
        1,
        "the count reveals only the visible message"
    );
}

/// **CHAT-D11 / 11.3 — a HYOK tenant produces 0 indexed message bodies (the structural skip).** There
/// is nothing to leak because nothing was indexed: a HYOK origin's `can_derive_plaintext_index()` is
/// false, so `admit_message_indexing` is `SkipHyok` and `may_index_messages` is false. Platform/BYOK
/// admit (full search).
#[test]
fn hyok_tenant_indexes_zero_message_bodies() {
    let engine = KmsEngine::new();
    let region = Region("fr-par".into());
    engine.ensure_kek(&KekId::new(TenantId("acme".into()), region.clone()));

    let platform = PlatformManaged::new(&engine, region.clone());
    assert!(
        may_index_messages(&platform),
        "platform-managed: full search/RAG"
    );

    let byok = Byok::new(&engine, region.clone(), "kms-customer://acme/k");
    assert!(
        may_index_messages(&byok),
        "BYOK: key live in-engine → can index"
    );

    assert_eq!(admit_message_indexing(&platform), IndexAdmission::Admit);
    assert_eq!(admit_message_indexing(&byok), IndexAdmission::Admit);

    // THE HEADLINE HYOK LEG: a HYOK class is REFUSED → 0 indexed message bodies (the structural skip).
    let hyok = Hyok::new(MockHyok::new());
    assert_eq!(
        admit_message_indexing(&hyok),
        IndexAdmission::SkipHyok,
        "a HYOK class is refused by construction (you cannot index what you cannot decrypt)"
    );
    assert!(
        !may_index_messages(&hyok),
        "0 indexed message bodies for a HYOK tenant → nothing to leak (11.3)"
    );
}

/// A deterministic mock customer HYOK key service (the customer holds the key OUTSIDE Myelin's reach).
/// The admission gate never calls wrap/unwrap — a HYOK class is refused before any key op.
struct MockHyok {
    key: [u8; KEY_LEN],
}
impl MockHyok {
    fn new() -> MockHyok {
        MockHyok {
            key: [7u8; KEY_LEN],
        }
    }
}
impl HyokKeyService for MockHyok {
    fn wrap(&self, _dek: &Dek) -> Result<WrappedDek, HyokServiceDenied> {
        Ok(WrappedDek {
            nonce: [0u8; 12],
            wrapped: self.key.to_vec(),
            kek_epoch: 0,
        })
    }
    fn unwrap(&self, _w: &WrappedDek) -> Result<DekHandle, HyokServiceDenied> {
        Ok(DekHandle::from_raw(self.key))
    }
    fn destroy(&self) {}
}
