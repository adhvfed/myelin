use std::sync::Arc;

use myelin_events::{Actor, CausedBy, EmitContextBase, OutboxStore, OutboxTransaction, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::cache::{Cache, InMemoryCache};
use myelin_tenancy::{Region, TenantId};

use super::*;
use crate::store::{AuthorKind, MemHotTier, NewMessage};

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
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn outbox() -> (OutboxStore, Arc<myelin_events::MonotonicMinter>) {
    (
        OutboxStore::new(),
        Arc::new(myelin_events::MonotonicMinter::new()),
    )
}

fn tx(store: &OutboxStore, minter: &Arc<myelin_events::MonotonicMinter>) -> OutboxTransaction {
    store.begin(minter.clone(), ctx_base())
}

fn conv() -> ConversationId {
    ConversationId::new("acme", "fr-par", "01J0CONVRS")
}

fn seed_messages(store: &MemHotTier, n: usize) -> Vec<MessageId> {
    let (ob, minter) = outbox();
    let mut ids = Vec::new();
    for i in 0..n {
        let mut t = tx(&ob, &minter);
        let id = store
            .append(
                &mut t,
                NewMessage {
                    conv: conv(),
                    thread_root_id: None,
                    author: "alice".into(),
                    author_kind: AuthorKind::Human,
                    body_inline: format!("msg {i}").into_bytes(),
                    body_nodes: Vec::new(),
                    client_nonce: format!("n{i}"),
                },
            )
            .expect("append");
        ids.push(id);
    }
    ids
}

fn service<'a>(store: &'a MemHotTier) -> ReadStateService<'a, MemHotTier, InMemoryCache> {
    ReadStateService::new(InMemoryCache::new(), store)
}

#[test]
fn flushed_marker_survives_a_cache_drop_pg_authoritative() {
    let store = MemHotTier::new();
    let ids = seed_messages(&store, 5);
    let svc = service(&store);

    let marker = ReadMarker::new(conv(), "alice", ids[2].clone());
    svc.mark_read(marker.clone());
    assert_eq!(svc.read_pos(&conv(), "alice"), Some(ids[2].clone()));

    assert_eq!(svc.flush(), 1, "the marker advanced the durable record");
    assert_eq!(svc.pending_len(), 0, "the flush drained the buffer");
    assert_eq!(svc.record().load(&conv(), "alice"), Some(ids[2].clone()));

    svc.drop_cache(&conv(), "alice");
    assert_eq!(
        svc.read_pos(&conv(), "alice"),
        Some(ids[2].clone()),
        "CHAT-D12: the PG record is authoritative after a cache loss (0 lost read-state)"
    );
}

#[test]
fn an_unflushed_marker_lost_on_cache_drop_is_benign_and_bounded() {
    let store = MemHotTier::new();
    let ids = seed_messages(&store, 5);
    let svc = service(&store);

    svc.mark_read(ReadMarker::new(conv(), "alice", ids[1].clone()));
    svc.flush();

    svc.mark_read(ReadMarker::new(conv(), "alice", ids[3].clone()));
    assert_eq!(svc.read_pos(&conv(), "alice"), Some(ids[3].clone()), "hot");

    svc.drop_cache(&conv(), "alice");
    assert_eq!(
        svc.read_pos(&conv(), "alice"),
        Some(ids[1].clone()),
        "benign+bounded: fall back to the durable truth (re-see a few read messages), never wrong"
    );
    assert!(svc.read_pos(&conv(), "alice").unwrap() < ids[3]);
}

#[test]
fn the_durable_flush_is_monotone_never_rewinds() {
    let store = MemHotTier::new();
    let ids = seed_messages(&store, 5);
    let svc = service(&store);

    svc.mark_read(ReadMarker::new(conv(), "alice", ids[3].clone()));
    svc.flush();
    assert!(
        !svc.record()
            .upsert(&ReadMarker::new(conv(), "alice", ids[1].clone())),
        "a stale flush does not rewind"
    );
    assert_eq!(svc.record().load(&conv(), "alice"), Some(ids[3].clone()));
}

#[test]
fn unread_is_a_bounded_range_read_count_id_gt_last_read() {
    let store = MemHotTier::new();
    let ids = seed_messages(&store, 5);
    let svc = service(&store);

    assert_eq!(svc.unread_count(&conv(), "bob").unwrap(), 5);

    svc.mark_read(ReadMarker::new(conv(), "bob", ids[2].clone()));
    assert_eq!(
        svc.unread_count(&conv(), "bob").unwrap(),
        2,
        "unread = count(id > last_read) = {ids:?} after ids[2]"
    );

    svc.mark_read(ReadMarker::new(conv(), "bob", ids[4].clone()));
    assert_eq!(svc.unread_count(&conv(), "bob").unwrap(), 0);
}

#[test]
fn unread_recomputes_correctly_from_the_durable_truth_after_cache_loss() {
    let store = MemHotTier::new();
    let ids = seed_messages(&store, 6);
    let svc = service(&store);

    svc.mark_read(ReadMarker::new(conv(), "carol", ids[3].clone()));
    svc.flush();
    svc.drop_cache(&conv(), "carol");

    assert_eq!(
        svc.unread_count(&conv(), "carol").unwrap(),
        2,
        "CHAT-D12: unread counts recompute correctly after a cache loss (PG authoritative)"
    );
}

#[test]
fn an_ambient_post_does_zero_per_member_unread_writes() {
    let store = MemHotTier::new();
    let svc = service(&store);
    assert_eq!(svc.ambient_post_unread_writes(1), 0);
    assert_eq!(
        svc.ambient_post_unread_writes(100_000),
        0,
        "a 100k-member post does 0 per-member unread writes (read-fanout, not write-fanout)"
    );
    assert!(
        svc.record().is_empty(),
        "no per-member unread rows materialised"
    );
}

#[test]
fn a_rapid_scroll_coalesces_to_one_pending_marker() {
    let store = MemHotTier::new();
    let ids = seed_messages(&store, 5);
    let svc = service(&store);

    for id in &ids {
        svc.mark_read(ReadMarker::new(conv(), "dave", id.clone()));
    }
    assert_eq!(
        svc.pending_len(),
        1,
        "the rapid scroll coalesced to one marker"
    );
    assert_eq!(svc.flush(), 1, "one UPSERT, not five");
    assert_eq!(svc.record().load(&conv(), "dave"), Some(ids[4].clone()));
}

#[derive(Default)]
struct CapturingPush {
    pushed: std::sync::Mutex<Vec<(String, MessageId)>>,
    seq: std::sync::atomic::AtomicU64,
}

impl ReadStatePush for CapturingPush {
    fn push_read_state(&self, scope: &FirehoseScope, marker: &ReadMarker) -> u64 {
        self.pushed
            .lock()
            .unwrap()
            .push((scope.selector(), marker.last_read.clone()));
        self.seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

#[test]
fn mark_read_pushes_the_coarse_frame_on_the_bounded_channel_scope() {
    let store = MemHotTier::new();
    let ids = seed_messages(&store, 3);
    let svc = service(&store);
    let push = CapturingPush::default();

    let seq = svc
        .mark_read_and_push(ReadMarker::new(conv(), "erin", ids[1].clone()), &push)
        .expect("the channel scope is bounded (never *)");
    assert_eq!(
        seq, 0,
        "the first push gets frame seq 0 (the resume cursor)"
    );

    let pushed = push.pushed.lock().unwrap();
    assert_eq!(pushed.len(), 1);
    assert_eq!(
        pushed[0].0, "channel:01J0CONVRS",
        "the coarse frame rides the BOUNDED channel:<id> scope (3.5, never *)"
    );
    assert_eq!(
        pushed[0].1, ids[1],
        "the frame carries the last-read marker ref"
    );

    assert_eq!(READ_STATE_UPDATED, "chat.read_state.updated");
}

#[test]
fn erasure_purges_the_principals_read_state_markers() {
    let store = MemHotTier::new();
    let ids = seed_messages(&store, 4);
    let svc = service(&store);

    svc.mark_read(ReadMarker::new(conv(), "frank", ids[1].clone()));
    svc.mark_read(ReadMarker::new(conv(), "grace", ids[2].clone()));
    svc.flush();
    assert_eq!(svc.record().len(), 2);

    let purged = svc.record().purge_principal("frank");
    assert_eq!(purged, 1, "exactly frank's marker purged");
    assert_eq!(
        svc.record().load(&conv(), "frank"),
        None,
        "0 recoverable read-state for frank"
    );
    assert_eq!(
        svc.record().load(&conv(), "grace"),
        Some(ids[2].clone()),
        "grace's read-state is untouched"
    );
}

#[test]
fn the_hot_marker_cache_key_is_isolated_per_tenant() {
    let key = ReadMarker::cache_key(&conv(), "alice");
    assert_eq!(key, "read:fr-par:alice:01J0CONVRS");

    let cache = InMemoryCache::new();
    cache
        .set(&TenantId("t1".into()), &key, b"id-a", HOT_MARKER_TTL)
        .unwrap();
    assert_eq!(
        cache.get(&TenantId("t2".into()), &key).unwrap(),
        None,
        "tenant t2 does not read t1's hot marker"
    );
}

#[test]
fn the_flush_cadence_and_ttl_are_named_tunables() {
    assert_eq!(DEFAULT_FLUSH_CADENCE, std::time::Duration::from_secs(2));
    assert_eq!(HOT_MARKER_TTL, std::time::Duration::from_secs(300));
    assert_eq!(CHAT_READ_STATE_STORE, "chat_read_state");
}
