use myelin_chat::{
    AuthorKind, ConversationId, MemHotTier, MessageId, MessageStore, NewMessage, ReadMarker,
    ReadStateService,
};
use myelin_events::{Actor, CausedBy, EmitContextBase, OutboxStore, OutboxTransaction, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::cache::InMemoryCache;
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn conv() -> ConversationId {
    ConversationId::new("acme", "fr-par", "01J0CONVCDC")
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
        caused_by: Some(CausedBy("session:cdc".into())),
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
fn provider_read_state_is_one_truth_pg_authoritative() {
    let store = MemHotTier::new();
    let ids = seed(&store, 5);
    let svc = ReadStateService::new(InMemoryCache::new(), &store);

    svc.mark_read(ReadMarker::new(conv(), "alice", ids[2].clone()));
    svc.flush();
    assert_eq!(svc.record().load(&conv(), "alice"), Some(ids[2].clone()));

    svc.drop_cache(&conv(), "alice");
    assert_eq!(
        svc.read_pos(&conv(), "alice"),
        Some(ids[2].clone()),
        "7.2: the read-state truth survives a cache loss (PG authoritative)"
    );
}

#[test]
fn consumer_unread_is_a_bounded_range_read_over_the_one_truth() {
    let store = MemHotTier::new();
    let ids = seed(&store, 5);
    let svc = ReadStateService::new(InMemoryCache::new(), &store);

    svc.mark_read(ReadMarker::new(conv(), "bob", ids[2].clone()));
    svc.flush();
    assert_eq!(svc.unread_count(&conv(), "bob").unwrap(), 2);

    svc.drop_cache(&conv(), "bob");
    assert_eq!(
        svc.unread_count(&conv(), "bob").unwrap(),
        2,
        "7.2: unread recomputes from the ONE durable truth after a cache loss"
    );

    assert_eq!(svc.ambient_post_unread_writes(100_000), 0);
}

#[test]
fn consumer_erasure_purges_the_read_state_markers() {
    let store = MemHotTier::new();
    let ids = seed(&store, 4);
    let svc = ReadStateService::new(InMemoryCache::new(), &store);

    svc.mark_read(ReadMarker::new(conv(), "frank", ids[1].clone()));
    svc.mark_read(ReadMarker::new(conv(), "grace", ids[2].clone()));
    svc.flush();

    let purged = svc.record().purge_principal("frank");
    assert_eq!(purged, 1, "D-C8: frank's read-state marker purged");
    assert_eq!(
        svc.record().load(&conv(), "frank"),
        None,
        "10.1/D-C8: 0 recoverable read-state for the erased subject"
    );
    assert_eq!(
        svc.record().load(&conv(), "grace"),
        Some(ids[2].clone()),
        "another principal's read-state is untouched (scoped erasure)"
    );
}
