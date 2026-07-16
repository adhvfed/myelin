//! # CDC + chained-e2e — the Chat **Unfurl Service** over contracts 5.2 / 5.7 / 4.3 / 4.2 (CHAT-P13 /
//! P-407, M4-C4)
//!
//! The CDC pair the CHAT-P13 TESTS field requires — the unfurl service's per-viewer gate + the
//! SetExpr→JOIN lowering + the 4-step ladder, bound against the REAL engines:
//!
//! **Contract-index rows:**
//! - **4.2** `check(subject, permission, object, zookie?, caveat?)` — CONSUMED: the per-viewer gate the
//!   unfurl service runs FIRST. PROVIDER = Identity's REAL `StoreBackedCheck` resolving the admitted
//!   Chat `channel.read = member ∪ parent_project->view` fragment.
//! - **4.3** `list_objects → Ids | Filter{set_expr, zookie}` — CONSUMED: the membership-as-permission
//!   class precompute. PROVIDER = Identity's REAL `ListObjects` emitting the frozen `Filter{InRelation}`;
//!   CONSUMER = chat's `precompute_visibility_class` lowering it over the unfurl candidate id column
//!   into ONE leak-free JOIN (no N+1, no post-filter).
//! - **5.2** `resolve(ref, viewer, mode) → Projection | Tombstone` — CONSUMED (the unfurl chokepoint):
//!   the unfurl service calls the Refs resolve port on a cache miss. The REAL Refs resolver is the
//!   NAMED FLOOR (REF-P10 / CHAT-P15); here a synthetic resolver models its EXACT contract so the
//!   no-leak PROPERTY is proven structurally (the SAME pattern `drill_chat_d5_humanise_leak.rs` uses).
//! - **5.7** the 4-step tombstone ladder (live/gone/erased for chat) — CONSUMED: the ladder outcomes
//!   map to leak-free cards; the tombstone always carries the ROOT.
//!
//! The PROVIDER↔CONSUMER pair is pinned here so a drift on either side fails the SAME CI job:
//! - **PROVIDER:** Identity's REAL `StoreBackedCheck` (4.2 `check`) + `ListObjects` (4.3 `list_objects`
//!   → `Filter`), over the admitted Chat fragment fed off the bus into the S8 reverse index.
//! - **CONSUMER:** the Chat [`UnfurlService`] (the gate-then-cache-then-ladder path) +
//!   `precompute_visibility_class` (the SetExpr→JOIN lowering over the unfurl candidate column).
//!
//! The chained e2e (EI-01 §4): resolve as member → REVOKE → re-resolve → assert tombstone + 0 title
//! leak — the no-leak property the whole unfurl service exists to guarantee (CHAT-D5).

use myelin_chat::membership::channel_object;
use myelin_chat::unfurl::{
    precompute_visibility_class, AuthzVisibleIndex, Card, LadderOutcome, Projection,
    RefsResolvePort, Tombstone, TombstoneReason, UnfurlCandidate, UnfurlService,
};
use myelin_events::{BusTransport, EventHandler, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, ObjectId, ObjectType, Permission, Principal,
    PrincipalId, PrincipalKind, RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    chat_fragment_defs, ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer,
    StoreBackedCheck, TupleStore,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
/// The secret channel title a denied viewer must NEVER see (the leak-test payload).
const SECRET_TITLE: &str = "#board-leadership-comp";

fn principal(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(TENANT.into()),
    );
    p.region = Region(REGION.into());
    p
}

fn scope_of(p: &Principal) -> TenantScope {
    TenantScope::from_verified_token(p, p.region.clone())
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn strong() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn confidential_ref() -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef("myelin://acme/chat/channel/board-secret".into())
}

fn candidate() -> UnfurlCandidate {
    UnfurlCandidate {
        ref_: confidential_ref(),
        channel_id: Some("board-secret".into()),
    }
}

/// A synthetic Refs resolve chokepoint (5.2; the REF-P10 / CHAT-P15 named floor) — returns a live
/// projection carrying the SECRET_TITLE so a leak would be observable, or a programmable ladder
/// outcome. Models the EXACT `Projection | Tombstone` contract the real resolver returns.
struct SyntheticResolver {
    outcome: LadderOutcome,
}
impl SyntheticResolver {
    fn live() -> SyntheticResolver {
        SyntheticResolver {
            outcome: LadderOutcome::Live(Projection {
                title: SECRET_TITLE.into(),
                state: "active".into(),
                icon: "channel".into(),
                sub_anchor: None,
            }),
        }
    }
    fn with(outcome: LadderOutcome) -> SyntheticResolver {
        SyntheticResolver { outcome }
    }
}
impl RefsResolvePort for SyntheticResolver {
    fn resolve(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        _ref_: &myelin_refs::ArtifactRef,
        _viewer: &Principal,
        _at: &Consistency,
    ) -> LadderOutcome {
        self.outcome.clone()
    }
}

/// Build the REAL Identity `check` engine over a `TupleStore` with the admitted Chat `channel`
/// fragment, sharing the store the grants are written to. The `channel.read = member ∪
/// parent_project->view` rewrite resolves a member → Allow, a non-member → Deny.
fn check_engine(grants: &[TupleDelta]) -> StoreBackedCheck {
    let store = TupleStore::new(OutboxStore::new());
    let svc = StoreBackedCheck::new(store.clone());
    for def in chat_fragment_defs() {
        assert!(
            matches!(
                svc.admit_fragment_def(&def),
                myelin_identity::FragmentAdmit::Admitted { .. }
            ),
            "the Chat `{}` fragment must admit",
            def.object_type.0
        );
    }
    if !grants.is_empty() {
        store
            .write_tuples(
                &scope_of(&principal("p-admin")),
                &principal("p-admin"),
                grants,
                None,
                None,
                Timestamp("2026-06-24T00:00:00Z".into()),
            )
            .expect("seed grants");
    }
    svc
}

/// Wire the REAL Identity `list_objects` over the admitted Chat fragment + a LIVE S8 index fed off the
/// bus from `grants`, at an explicit cardinality `cap` (so the Ids↔Filter switch is deterministic).
fn list_objects_engine(cap: usize, grants: &[TupleDelta]) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in chat_fragment_defs() {
        assert!(matches!(
            namespace.admit(&def),
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    store
        .write_tuples(
            &scope_of(&principal("p-admin")),
            &principal("p-admin"),
            grants,
            None,
            None,
            Timestamp("2026-06-24T00:00:00Z".into()),
        )
        .expect("seed grants");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    ListObjects::with_cap(store, namespace, index, cap)
}

// ───────────────────────────── 4.2 — the per-viewer gate (member vs non-member) ──────────────────

/// **CDC (4.2 / 5.2): the unfurl service gates per-viewer through the REAL `check` — a member sees the
/// projection; a non-member tombstones (0 title leak).** The gate is the leak-free chokepoint: the
/// non-member's title is never fetched.
#[test]
fn cdc_4_2_unfurl_gate_member_sees_projection_non_member_tombstones() {
    // alice is a member of channel:board-secret; mallory is not.
    let grants = [add("channel:board-secret", "member", "p:alice")];
    let svc = UnfurlService::new(check_engine(&grants), SyntheticResolver::live());

    // MEMBER → the projection (the live engine resolves the `member` arm of channel.read).
    let card = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert_eq!(
        card.exposed_title(),
        Some(SECRET_TITLE),
        "a member sees the channel projection title"
    );

    // NON-MEMBER → a tombstone, 0 title leak (the gate denies; the title is never fetched).
    let card = svc.resolve_one(&candidate(), &principal("p:mallory"));
    assert!(card.is_tombstone(), "a non-member sees a tombstone");
    assert_eq!(card.exposed_title(), None, "0 title leak for a non-member");
    match card {
        Card::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::Denied),
        other => panic!("expected a Denied tombstone, got {other:?}"),
    }
}

// ───────────────────────────── 4.3 — the SetExpr → JOIN class precompute ──────────────────────────

/// **CDC (4.3): the REAL Identity `list_objects` emits `Filter{InRelation}`; chat lowers it over the
/// unfurl candidate id column into ONE leak-free JOIN (no N+1, no post-filter).** Both sides agree on
/// the wire shape; the class is precomputed ONCE, not per-candidate.
#[test]
fn cdc_4_3_list_objects_filter_lowers_to_one_unfurl_join() {
    // the viewer can read two channels via `member`; the cap is BELOW that → Filter (the push-down).
    let grants = [
        add("channel:c1", "member", "p:viewer"),
        add("channel:c2", "member", "p:viewer"),
    ];
    let lo = list_objects_engine(1, &grants);
    let viewer = principal("p:viewer");

    // PRODUCER: the real Identity engine returns the frozen Filter{set_expr}.
    let r = lo.list_objects(
        &scope_of(&principal("p-admin")),
        &viewer,
        &Permission("read".into()),
        &ObjectType("channel".into()),
        &strong(),
    );
    let set_expr = match r {
        ListObjectsResult::Filter { set_expr, .. } => set_expr,
        ListObjectsResult::Ids { .. } => panic!("above the cap the producer pushes down to Filter"),
    };
    assert!(
        matches!(set_expr, SetExpr::InRelation { .. }),
        "the producer emits the InRelation push-down shape"
    );

    // CONSUMER: chat lowers it over the unfurl candidate id column into ONE leak-free JOIN.
    let lowered = precompute_visibility_class(&set_expr, &viewer);
    assert_eq!(
        lowered.join_count(),
        1,
        "the consumer lowers to EXACTLY ONE reverse-index JOIN (no N+1)"
    );
    assert!(
        lowered.joins.iter().any(|j| j
            .clause
            .contains("ON av0.object_id = unfurl_candidate.object_id")),
        "the consumer JOINs the producer's reverse index over its own candidate column (§5.3/§7.3)"
    );
}

/// **CDC (4.3) leak-free class evaluation: the lowered JOIN keeps only the channels the viewer holds
/// `member` on — a channel granted to someone ELSE never survives (0 leak), no per-candidate check.**
#[test]
fn cdc_4_3_class_evaluation_is_leak_free() {
    let index = AuthzVisibleIndex::new();
    let viewer = principal("p:viewer");
    let tenant = TenantId(TENANT.into());
    let region = Region(REGION.into());
    // the viewer is a member of c1; c-secret is granted to someone else (the leak witness).
    index.grant(
        &tenant,
        &region,
        "p:viewer",
        "member",
        "channel:c1",
        "zk-01",
    );

    let set_expr = SetExpr::InRelation {
        relation: RelName("member".into()),
        via_column: myelin_chat::unfurl::unfurl_candidate_colref(),
    };
    let lowered = precompute_visibility_class(&set_expr, &viewer);
    let candidates = vec![
        ObjectId("channel:c1".into()),
        ObjectId("channel:c-secret".into()),
    ];
    let visible = index.evaluate(&tenant, &region, &viewer, &lowered, &candidates);
    assert_eq!(
        visible,
        vec![ObjectId("channel:c1".into())],
        "0 leak of the channel the viewer is not a member of"
    );
}

// ───────────────────────────── 5.7 — the 4-step tombstone ladder ──────────────────────────────────

/// **CDC (5.7): the 4-step ladder outcomes (live / gone / erased for a chat ref) map to leak-free
/// cards; the tombstone always carries the ROOT.** Chat consumes the ladder; a gone/erased outcome is
/// a tombstone with no title.
#[test]
fn cdc_5_7_ladder_outcomes_are_leak_free_cards() {
    let grants = [add("channel:board-secret", "member", "p:alice")];

    // LIVE → a live card.
    let svc = UnfurlService::new(check_engine(&grants), SyntheticResolver::live());
    assert!(matches!(
        svc.resolve_one(&candidate(), &principal("p:alice")),
        Card::Live { .. }
    ));

    // GONE → a tombstone carrying the root.
    let svc = UnfurlService::new(
        check_engine(&grants),
        SyntheticResolver::with(LadderOutcome::Gone(Tombstone {
            root: confidential_ref(),
            reason: TombstoneReason::Gone,
        })),
    );
    match svc.resolve_one(&candidate(), &principal("p:alice")) {
        Card::Tombstone(t) => {
            assert_eq!(t.reason, TombstoneReason::Gone);
            assert_eq!(t.root, confidential_ref(), "the tombstone carries the root");
        }
        other => panic!("expected Gone, got {other:?}"),
    }

    // ERASED → a tombstone, "[erased]".
    let svc = UnfurlService::new(
        check_engine(&grants),
        SyntheticResolver::with(LadderOutcome::Erased(Tombstone {
            root: confidential_ref(),
            reason: TombstoneReason::Erased,
        })),
    );
    match svc.resolve_one(&candidate(), &principal("p:alice")) {
        Card::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::Erased),
        other => panic!("expected Erased, got {other:?}"),
    }
}

// ───────────────────────────── the chained no-leak e2e (CHAT-D5; EI-01 §4) ────────────────────────

/// **The CHAINED no-leak property (CHAT-D5; EI-01 §4): resolve as member → REVOKE → re-resolve →
/// tombstone, 0 title leak — the whole point of the unfurl service.** The member sees the title; after
/// the membership tuple is removed (the revoke) the SAME viewer sees a tombstone — the per-viewer gate
/// denies BEFORE the cache (which still holds the shared content) is read, so 0 title leak post-revoke.
#[test]
fn chat_d5_chained_member_then_revoke_then_tombstone_zero_leak() {
    // Build a check engine whose store we can mutate (write the member grant, then remove it).
    let store = TupleStore::new(OutboxStore::new());
    let svc_check = StoreBackedCheck::new(store.clone());
    for def in chat_fragment_defs() {
        assert!(matches!(
            svc_check.admit_fragment_def(&def),
            myelin_identity::FragmentAdmit::Admitted { .. }
        ));
    }
    let scope = scope_of(&principal("p-admin"));
    let admin = principal("p-admin");
    let now = Timestamp("2026-06-24T00:00:00Z".into());

    // GRANT alice the `member` tuple.
    store
        .write_tuples(
            &scope,
            &admin,
            &[add("channel:board-secret", "member", "p:alice")],
            None,
            None,
            now.clone(),
        )
        .expect("grant");

    let svc = UnfurlService::new(svc_check, SyntheticResolver::live());

    // MEMBER resolve → sees the title.
    let before = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert_eq!(
        before.exposed_title(),
        Some(SECRET_TITLE),
        "the member sees the title"
    );
    // the cache now holds the shared projection (one entry per ref).
    assert_eq!(svc.cache().entry_count(), 1);

    // REVOKE: remove alice's member tuple (the new-enemy case — the grant is gone).
    store
        .write_tuples(
            &scope,
            &admin,
            &[TupleDelta::Remove(RelationTuple {
                object: ObjectId("channel:board-secret".into()),
                relation: RelName("member".into()),
                subject: PrincipalId("p:alice".into()),
                caveat: None,
            })],
            None,
            None,
            Timestamp("2026-06-24T00:01:00Z".into()),
        )
        .expect("revoke");

    // RE-RESOLVE → tombstone, 0 title leak (the gate denies BEFORE the cache is read, even though the
    // shared cache STILL holds the projection — the per-viewer decision is the gate, not the cache).
    let after = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert!(after.is_tombstone(), "post-revoke the card is a tombstone");
    assert_eq!(after.exposed_title(), None, "0 title leak post-revoke");
    // the cache is unchanged (one shared entry) — the no-leak is the GATE, not a per-viewer cache.
    assert_eq!(
        svc.cache().entry_count(),
        1,
        "still ONE cache entry per ref (never per (ref, viewer))"
    );
    // sanity: the channel object the gate checked is the membership-tuple object (one object-id lang).
    assert_eq!(channel_object("board-secret"), "channel:board-secret");
}
