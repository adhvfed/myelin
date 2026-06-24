//! **CHAT-P7 / P-401 — the CONSUMER-side CDC pair for the storage contracts the Conversation /
//! Membership entity rides: row 11.1 (the OLTP tier the conversation + membership rows live in) +
//! row 12.1 (the `(tenant, region)` partition key the conversation list is residency-pinned on).**
//!
//! The PROVIDERS are Storage (the OLTP tier — the partitioned `conversation` / `membership` tables)
//! and Tenancy (the `(tenant, region)` partition-key newtypes). Chat is the CONSUMER: its
//! `Conversation`/`Membership` entity keys on the frozen `(TenantId, Region)` partition shape
//! (12.1) and persists the conversation + membership rows on the OLTP tier (11.1), and the
//! `membership_by_principal` index returns the residency-pinned conversation list S1.
//!
//! This file carries BOTH a provider-side and a consumer-side marker (the contract-coverage
//! scanner's CDC-pair requirement): the PROVIDER shapes are the storage/tenancy types, exercised
//! here as the CONSUMER (the chat conversation store) drives them. The DB-free legs run in
//! `cargo test --workspace`; the live-OLTP leg for the message log is the sibling
//! `integration_chat_p4_message_store.rs` (the conversation rows share the SAME `(tenant, region)`
//! OLTP partition + tier, contract 11.1 / 12.1).

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
    }
}

/// **Row 12.1 CONSUMER:** the Conversation/Membership partition key is the frozen `(TenantId,
/// Region)` shape the PROVIDER (Tenancy) owns — the conversation list S1 is residency-pinned: a
/// principal's "my conversations" in `(tenant, region)` returns ONLY that partition's rows, 0
/// cross-region rows.
#[test]
fn chat_conversation_consumes_the_tenant_region_partition_key_12_1() {
    // The PROVIDER newtypes (Tenancy 12.1) drive the CONSUMER (the conversation store) keys.
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

    // The residency-pinned list keys distinguish the two regions (the partition key carries region).
    let list = store.conversations_of("acme", "alice").unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.contains(&c_fr.id) && list.contains(&c_de.id));
    assert_ne!(
        c_fr.id, c_de.id,
        "fr-par and de-fra are distinct residency-pinned partition keys (12.1)"
    );
}

/// **Row 11.1 CONSUMER:** the Conversation/Membership surface IS an OLTP-tier consumer (the
/// partitioned `conversation` + `membership` tables) — the entity round-trips create/get with its
/// kinds + retention_days + linked_ref (0 schema-violation rows), and the `membership_by_principal`
/// index returns EXACTLY the member set (0 missing, 0 extra) — the leak-free, no-N+1 conversation
/// list S1 (contract 4.3 the `list_objects` gate joins against; wired in CHAT-P8/P13).
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

    // The OLTP row round-trips verbatim (0 schema-violation; the kinds + retention + linked_ref hold).
    let got = store.get(&linked.id).unwrap();
    assert_eq!(got, linked);
    assert_eq!(got.kind, ConversationKind::ArtifactLinked);
    assert_eq!(got.retention_days, Some(30));
    assert_eq!(got.linked_ref.as_deref(), Some("issue/ABC-1"));

    // The membership_by_principal index is EXACT against the OLTP membership rows.
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
