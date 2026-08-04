use std::sync::Arc;

use myelin_chat::{
    AuthorKind, ConversationId, MemHotTier, MessageId, MessageStore, NewMessage, ReadMarker,
    ReadStateService,
};
use myelin_events::{Actor, CausedBy, EmitContextBase, OutboxStore, OutboxTransaction, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::cache::InMemoryCache;
use myelin_tenancy::{Region, TenantId};

fn conv() -> ConversationId {
    ConversationId::new("acme", "fr-par", "01J0CONVD12")
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:d12".into())),
    }
}

fn seed(store: &MemHotTier, n: usize) -> Vec<MessageId> {
    let ob = OutboxStore::new();
    let minter = Arc::new(myelin_events::MonotonicMinter::new());
    let mut ids = Vec::new();
    for i in 0..n {
        let mut tx: OutboxTransaction = ob.begin(minter.clone(), ctx_base());
        let id = store
            .append(
                &mut tx,
                NewMessage {
                    conv: conv(),
                    thread_root_id: None,
                    author: "alice".into(),
                    author_kind: AuthorKind::Human,
                    body_inline: format!("m{i}").into_bytes(),
                    body_nodes: Vec::new(),
                    client_nonce: format!("c{i}"),
                },
            )
            .expect("append");
        ids.push(id);
    }
    ids
}

#[test]
fn chat_d12_flush_then_drop_valkey_mid_session_pg_authoritative() {
    let store = MemHotTier::new();
    let ids = seed(&store, 8);
    let svc = ReadStateService::new(InMemoryCache::new(), &store);

    svc.mark_read(ReadMarker::new(conv(), "alice", ids[4].clone()));
    assert_eq!(
        svc.read_pos(&conv(), "alice"),
        Some(ids[4].clone()),
        "the hot marker is served from the cache mid-scroll"
    );
    assert_eq!(svc.unread_count(&conv(), "alice").unwrap(), 3);

    assert_eq!(
        svc.flush(),
        1,
        "the batched flush UPSERTs the durable record"
    );
    assert_eq!(
        svc.record().load(&conv(), "alice"),
        Some(ids[4].clone()),
        "the durable PG record holds the marked position (the truth)"
    );

    svc.drop_cache(&conv(), "alice");

    let lost_read_state = if svc.read_pos(&conv(), "alice") == Some(ids[4].clone()) {
        0
    } else {
        1
    };
    assert_eq!(
        lost_read_state, 0,
        "CHAT-D12: the lost-read-state signal is 0 (the PG record is authoritative)"
    );

    assert_eq!(
        svc.unread_count(&conv(), "alice").unwrap(),
        3,
        "CHAT-D12: unread recomputes correctly after the cache loss (count(id > last_read))"
    );
}

#[test]
fn chat_d12_an_unflushed_marker_is_at_worst_slightly_stale() {
    let store = MemHotTier::new();
    let ids = seed(&store, 8);
    let svc = ReadStateService::new(InMemoryCache::new(), &store);

    svc.mark_read(ReadMarker::new(conv(), "alice", ids[2].clone()));
    svc.flush();

    svc.mark_read(ReadMarker::new(conv(), "alice", ids[6].clone()));

    svc.drop_cache(&conv(), "alice");

    let reconstructed = svc.read_pos(&conv(), "alice").expect("durable marker");
    assert_eq!(
        reconstructed, ids[2],
        "the un-flushed advance is lost → fall back to the durable truth (slightly stale)"
    );
    assert!(
        reconstructed < ids[6],
        "the staleness is BOUNDED by the un-flushed window only (a few re-seen messages)"
    );
    assert_eq!(
        svc.unread_count(&conv(), "alice").unwrap(),
        5,
        "unread recomputes correctly from the durable truth (benign+bounded staleness)"
    );
}

#[test]
fn chat_d12_ambient_post_does_zero_per_member_unread_writes() {
    let store = MemHotTier::new();
    let svc = ReadStateService::new(InMemoryCache::new(), &store);
    assert_eq!(
        svc.ambient_post_unread_writes(100_000),
        0,
        "0 per-member unread writes on an ambient post (read-fanout)"
    );
    assert!(
        svc.record().is_empty(),
        "no per-member unread rows materialised by the post"
    );
}
