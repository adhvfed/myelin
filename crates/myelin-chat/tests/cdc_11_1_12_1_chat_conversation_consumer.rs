use myelin_chat::conversation::{
    Conversation, ConversationKind, ConversationStore, MemConversationStore, Membership,
};
use myelin_chat::store::ConversationId;
use myelin_tenancy::{Region, TenantId};

fn conv_row(tenant: &TenantId, region: &Region, id: &str, kind: ConversationKind) -> Conversation {
    let cid = ConversationId::new(tenant.0.clone(), region.0.clone(), id);
    Conversation {
        home_cell: Conversation::home_cell_for(&cid),
        id: cid,
        kind,
        parent_project: Some("proj".into()),
        name: Some("general".into()),
        topic: None,
        linked_ref: None,
        pinned_canvas: None,
        retention_days: Some(90),
        archived: false,
        created_by: "psn:creator".into(),
        acl_zookie: None,
    }
}

#[test]
fn chat_conversation_consumes_the_tenant_region_partition_key_12_1() {
    let tenant = TenantId("acme".into());
    let fr = Region("fr-par".into());
    let de = Region("de-fra".into());

    let store = MemConversationStore::new();
    let c_fr = conv_row(&tenant, &fr, "conv", ConversationKind::ChannelPrivate);
    let c_de = conv_row(&tenant, &de, "conv", ConversationKind::ChannelPrivate);
    store.create(c_fr.clone()).unwrap();
    store.create(c_de.clone()).unwrap();
    store
        .join(Membership::member(c_fr.id.clone(), "alice"))
        .unwrap();
    store
        .join(Membership::member(c_de.id.clone(), "alice"))
        .unwrap();

    let list = store.conversations_of("acme", "alice").unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.contains(&c_fr.id) && list.contains(&c_de.id));
    assert_ne!(
        c_fr.id, c_de.id,
        "fr-par and de-fra are distinct residency-pinned partition keys (12.1)"
    );
}

#[test]
fn chat_conversation_consumes_the_oltp_tier_surface_11_1() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let store = MemConversationStore::new();

    let mut linked = conv_row(
        &tenant,
        &region,
        "c-linked",
        ConversationKind::ArtifactLinked,
    );
    linked.linked_ref = Some("issue/ABC-1".into());
    linked.retention_days = Some(30);
    store.create(linked.clone()).unwrap();

    let got = store.get(&linked.id).unwrap();
    assert_eq!(got, linked);
    assert_eq!(got.kind, ConversationKind::ArtifactLinked);
    assert_eq!(got.retention_days, Some(30));
    assert_eq!(got.linked_ref.as_deref(), Some("issue/ABC-1"));

    store
        .create(conv_row(&tenant, &region, "c2", ConversationKind::Dm))
        .unwrap();
    store
        .join(Membership::member(linked.id.clone(), "alice"))
        .unwrap();
    store
        .join(Membership::member(
            ConversationId::new("acme", "fr-par", "c2"),
            "alice",
        ))
        .unwrap();
    let alice = store.conversations_of("acme", "alice").unwrap();
    assert_eq!(
        alice.len(),
        2,
        "EXACTLY Alice's two memberships (0 missing, 0 extra)"
    );
    assert!(store
        .conversations_of("acme", "stranger")
        .unwrap()
        .is_empty());
}
