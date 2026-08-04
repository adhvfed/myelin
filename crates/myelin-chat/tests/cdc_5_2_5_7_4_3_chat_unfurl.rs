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

#[test]
fn cdc_4_2_unfurl_gate_member_sees_projection_non_member_tombstones() {
    let grants = [add("channel:board-secret", "member", "p:alice")];
    let svc = UnfurlService::new(check_engine(&grants), SyntheticResolver::live());

    let card = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert_eq!(
        card.exposed_title(),
        Some(SECRET_TITLE),
        "a member sees the channel projection title"
    );

    let card = svc.resolve_one(&candidate(), &principal("p:mallory"));
    assert!(card.is_tombstone(), "a non-member sees a tombstone");
    assert_eq!(card.exposed_title(), None, "0 title leak for a non-member");
    match card {
        Card::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::Denied),
        other => panic!("expected a Denied tombstone, got {other:?}"),
    }
}

#[test]
fn cdc_4_3_list_objects_filter_lowers_to_one_unfurl_join() {
    let grants = [
        add("channel:c1", "member", "p:viewer"),
        add("channel:c2", "member", "p:viewer"),
    ];
    let lo = list_objects_engine(1, &grants);
    let viewer = principal("p:viewer");

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

#[test]
fn cdc_4_3_class_evaluation_is_leak_free() {
    let index = AuthzVisibleIndex::new();
    let viewer = principal("p:viewer");
    let tenant = TenantId(TENANT.into());
    let region = Region(REGION.into());
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

#[test]
fn cdc_5_7_ladder_outcomes_are_leak_free_cards() {
    let grants = [add("channel:board-secret", "member", "p:alice")];

    let svc = UnfurlService::new(check_engine(&grants), SyntheticResolver::live());
    assert!(matches!(
        svc.resolve_one(&candidate(), &principal("p:alice")),
        Card::Live { .. }
    ));

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

#[test]
fn chat_d5_chained_member_then_revoke_then_tombstone_zero_leak() {
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

    let before = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert_eq!(
        before.exposed_title(),
        Some(SECRET_TITLE),
        "the member sees the title"
    );
    assert_eq!(svc.cache().entry_count(), 1);

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

    let after = svc.resolve_one(&candidate(), &principal("p:alice"));
    assert!(after.is_tombstone(), "post-revoke the card is a tombstone");
    assert_eq!(after.exposed_title(), None, "0 title leak post-revoke");
    assert_eq!(
        svc.cache().entry_count(),
        1,
        "still ONE cache entry per ref (never per (ref, viewer))"
    );
    assert_eq!(channel_object("board-secret"), "channel:board-secret");
}
