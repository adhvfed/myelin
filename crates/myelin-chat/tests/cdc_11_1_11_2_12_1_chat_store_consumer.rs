use myelin_chat::store::{
    AuthorKind, ColdSegments, ConversationId, MemHotTier, MessageStore, NewMessage, RangeCursor,
};
use myelin_storage::{BlobStore, FsBlobStore};
use myelin_tenancy::{Region, TenantId};

fn ctx_base() -> myelin_events::EmitContextBase {
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    myelin_events::EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: myelin_events::Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: myelin_events::Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: myelin_events::Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: None,
    }
}

#[test]
fn chat_store_consumes_the_tenant_region_partition_key_12_1() {
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let conv = ConversationId::new(tenant.0.clone(), region.0.clone(), "01J0CONV");

    let store = MemHotTier::new();
    let ob = myelin_events::OutboxStore::new();
    let minter = std::sync::Arc::new(myelin_events::MonotonicMinter::new());
    let mut tx = ob.begin(minter, ctx_base());
    store
        .append(
            &mut tx,
            NewMessage {
                conv: conv.clone(),
                thread_root_id: None,
                author: "alice".into(),
                author_kind: AuthorKind::Human,
                body_inline: b"hello".to_vec(),
                body_nodes: Vec::new(),
                client_nonce: "n0".into(),
            },
        )
        .unwrap();
    tx.commit().unwrap();

    let other_region = ConversationId::new(tenant.0.clone(), "de-fra", "01J0CONV");
    assert_eq!(
        store
            .range(&other_region, RangeCursor::Recent, 10)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        store.range(&conv, RangeCursor::Recent, 10).unwrap().len(),
        1
    );
}

#[test]
fn chat_cold_tier_consumes_the_content_addressed_blobstore_11_2() {
    let provider: FsBlobStore = FsBlobStore::new();
    let tenant = TenantId("acme".into());
    let addr = provider.put(&tenant, b"a sealed segment").unwrap();
    assert_eq!(provider.get(&tenant, &addr).unwrap(), b"a sealed segment");

    let store = MemHotTier::new();
    let ob = myelin_events::OutboxStore::new();
    let minter = std::sync::Arc::new(myelin_events::MonotonicMinter::new());
    let conv = ConversationId::new("acme", "fr-par", "01J0CONV");
    let mut ids = Vec::new();
    for i in 0..6 {
        let mut tx = ob.begin(minter.clone(), ctx_base());
        ids.push(
            store
                .append(
                    &mut tx,
                    NewMessage {
                        conv: conv.clone(),
                        thread_root_id: None,
                        author: "a".into(),
                        author_kind: AuthorKind::Human,
                        body_inline: format!("m{i}").into_bytes(),
                        body_nodes: Vec::new(),
                        client_nonce: format!("n{i}"),
                    },
                )
                .unwrap(),
        );
        tx.commit().unwrap();
    }
    let before = store.range(&conv, RangeCursor::Recent, 100).unwrap();
    store.seal_before(&conv, &ids[3]).unwrap();
    let after = store.range(&conv, RangeCursor::Recent, 100).unwrap();
    assert_eq!(
        before, after,
        "the cold seal is transparent to the trait surface"
    );

    let _cold = ColdSegments::default();
}

#[test]
fn chat_message_store_consumes_the_oltp_log_surface_11_1() {
    let store = MemHotTier::new();
    let ob = myelin_events::OutboxStore::new();
    let minter = std::sync::Arc::new(myelin_events::MonotonicMinter::new());
    let conv = ConversationId::new("acme", "fr-par", "01J0CONV");
    let mut ids = Vec::new();
    for i in 0..10 {
        let mut tx = ob.begin(minter.clone(), ctx_base());
        ids.push(
            store
                .append(
                    &mut tx,
                    NewMessage {
                        conv: conv.clone(),
                        thread_root_id: None,
                        author: "a".into(),
                        author_kind: AuthorKind::Human,
                        body_inline: format!("m{i}").into_bytes(),
                        body_nodes: Vec::new(),
                        client_nonce: format!("n{i}"),
                    },
                )
                .unwrap(),
        );
        tx.commit().unwrap();
    }
    let gap = store.resync_from(&conv, &ids[4]).unwrap();
    let got: Vec<_> = gap.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(
        got,
        ids[5..].to_vec(),
        "the OLTP-log resync is gap-free, ULID-ordered"
    );
}
