use myelin_events::{BusTransport, EventHandler as _, InProcessBus, OutboxStore, Relay, Timestamp};
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, Decision, FragmentAdmit, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind,
    RelName, RelationTuple, SetExpr, TupleDelta, Zookie,
};
use myelin_identity_service::{
    chat_fragment, ListObjects, NamespaceEngine, ReverseIndex, ReverseIndexConsumer,
    StoreBackedCheck, TupleStore, CHANNEL_MEMBER, CHANNEL_READ, MESSAGE_VIEW,
};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn scope(tenant: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p-admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region("eu-west".into()))
}

fn subject(id: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    p.region = Region("eu-west".into());
    p
}

fn at_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

fn add(object: &str, relation: &str, subject: &str) -> TupleDelta {
    TupleDelta::Add(RelationTuple {
        object: ObjectId(object.into()),
        relation: RelName(relation.into()),
        subject: PrincipalId(subject.into()),
        caveat: None,
    })
}

fn provider(scope: &TenantScope, tuples: &[TupleDelta]) -> StoreBackedCheck {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            tuples,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
        )
        .expect("seed tuples");
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    for env in bus.consume("") {
        consumer.handle(&env, &mut myelin_events::HandlerTx::none());
    }
    let svc = StoreBackedCheck::with_index(store, index);
    for admit in svc.admit_chat_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "Id's compiled Chat fragment admits: {admit:?}"
        );
    }
    svc
}

#[test]
fn cdc_4_9_id_compiled_chat_fragment_admits() {
    let s = scope("acme");
    let svc = provider(&s, &[]);
    let ns = svc.namespace();
    for ty in ["channel", "message", "unfurl"] {
        assert!(
            ns.object_types().contains(&ty.to_string()),
            "`{ty}` admitted into the cell schema"
        );
    }
    assert!(ns.resolve_permission("channel", CHANNEL_READ).is_some());
    assert!(ns.resolve_permission("message", MESSAGE_VIEW).is_some());
    assert!(ns.resolve_permission("unfurl", MESSAGE_VIEW).is_some());
    assert!(
        ns.is_watchable("channel"),
        "the channel is watchable (the 50k-density read-fanout, §7.5)"
    );
}

#[test]
fn cdc_4_9_channel_read_resolves_via_member_and_via_project() {
    let s = scope("acme");
    let svc = provider(
        &s,
        &[
            add("channel:general", CHANNEL_MEMBER, "p:alice"),
            add("project:proj", "reader", "p:bob"),
            add("channel:general", "parent_project", "project:proj#view"),
            add("message:m1", "parent_channel", "channel:general#read"),
        ],
    );
    let can_read = |actor: &Principal, channel: &str| {
        matches!(
            svc.check(
                actor,
                &Permission(CHANNEL_READ.into()),
                &ArtifactRef(channel.into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    let can_view_msg = |actor: &Principal, message: &str| {
        matches!(
            svc.check(
                actor,
                &Permission(MESSAGE_VIEW.into()),
                &ArtifactRef(message.into()),
                &at_latest(),
                None
            ),
            Ok(Decision::Allow)
        )
    };
    assert!(
        can_read(&subject("p:alice"), "channel:general"),
        "a direct member reads the channel (the `member` arm)"
    );
    assert!(
        can_read(&subject("p:bob"), "channel:general"),
        "a project reader reads the channel (the parent_project->view arm)"
    );
    assert!(
        !can_read(&subject("p:carol"), "channel:general"),
        "a non-member denies (in neither arm of channel.read)"
    );
    assert!(
        can_view_msg(&subject("p:alice"), "message:m1"),
        "a member views the channel's messages (message.view = parent_channel->read)"
    );
    assert!(
        !can_view_msg(&subject("p:carol"), "message:m1"),
        "a non-member cannot view the channel's messages (by construction)"
    );
}

fn wired(cap: usize, scope: &TenantScope, grants: &[TupleDelta]) -> ListObjects {
    let outbox = OutboxStore::new();
    let store = TupleStore::new(outbox.clone());
    let index = ReverseIndex::new();
    let consumer = ReverseIndexConsumer::new(index.clone());

    let mut namespace = NamespaceEngine::with_core_hierarchy();
    for def in chat_fragment::chat_fragment_defs() {
        let admit = namespace.admit(&def);
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Chat `{}` fragment admits",
            def.object_type.0
        );
    }
    store
        .write_tuples(
            scope,
            &subject("p-admin"),
            grants,
            None,
            None,
            Timestamp("2026-06-22T00:00:00Z".into()),
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
fn cdc_4_9_channel_list_conjoins_in_one_query() {
    let s = scope("acme");
    let mut grants: Vec<TupleDelta> = Vec::new();
    for i in 0..8 {
        grants.push(add(&format!("channel:c-{i}"), CHANNEL_MEMBER, "p:alice"));
    }
    let lo = wired(2, &s, &grants);
    let result = lo
        .list_objects(
            &s,
            &subject("p:alice"),
            &Permission(CHANNEL_READ.into()),
            &ObjectType("channel".into()),
            &at_latest(),
        )
        .expect("read relationships for the pushed-down channel list");
    match result {
        ListObjectsResult::Filter { set_expr, .. } => match set_expr {
            SetExpr::InRelation { via_column, .. } => {
                assert_eq!(
                    via_column,
                    ColRef {
                        table: "channel".into(),
                        column: "id".into()
                    },
                    "the channel-list Filter names the consumer's own id column (channel.id, §7.3) - \
                     one query, no N+1"
                );
            }
            other => {
                panic!("the channel-list Filter is the InRelation push-down shape, got {other:?}")
            }
        },
        ListObjectsResult::Ids { .. } => {
            panic!("above the cap the channel list must push down to the channel.id Filter")
        }
    }
}

#[test]
fn cdc_4_9_non_member_channel_list_is_leak_free() {
    let s = scope("acme");
    let grants = vec![
        add("channel:visible-1", CHANNEL_MEMBER, "p:alice"),
        add("channel:visible-2", CHANNEL_MEMBER, "p:alice"),
        add("channel:secret", CHANNEL_MEMBER, "p:other"),
    ];
    let lo = wired(100, &s, &grants);
    let result = lo
        .list_objects(
            &s,
            &subject("p:alice"),
            &Permission(CHANNEL_READ.into()),
            &ObjectType("channel".into()),
            &at_latest(),
        )
        .expect("read relationships for the materialized channel list");
    let ids = match result {
        ListObjectsResult::Ids { ids, .. } => ids.into_iter().map(|o| o.0).collect::<Vec<String>>(),
        ListObjectsResult::Filter { .. } => {
            panic!("below the cap the channel list materialises Ids")
        }
    };
    assert!(
        ids.iter().any(|i| i == "channel:visible-1")
            && ids.iter().any(|i| i == "channel:visible-2"),
        "alice's channel list shows her two member channels: {ids:?}"
    );
    assert!(
        !ids.iter().any(|i| i == "channel:secret"),
        "a private channel alice is not a member of is ABSENT (leak-free, no count leak; \
         search-as-non-member 0 results): {ids:?}"
    );
}
