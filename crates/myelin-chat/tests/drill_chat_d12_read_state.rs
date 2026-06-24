//! # CHAT-D12 — Read-state cache-loss is benign (CHAT-P16 / P-410, M4-C5)
//!
//! **The drill-catalogue row (testing-strategy/01 CHAT-D12):** "Flush + drop Valkey mid-session → the
//! PG record is authoritative; a marker is at-worst slightly stale (re-see a few read messages);
//! unread counts recompute correctly." Thresholds:
//! - **the lost-read-state signal = 0 (PG authoritative)** — a FLUSHED marker survives a cache loss;
//! - **a marker is at-worst slightly stale** — only the UN-FLUSHED tail is lost (benign+bounded);
//! - **unread counts recompute correctly** — `count(id > last_read)` against the durable truth + log.
//!
//! Architecture: `02-internals-and-algorithms.md` §3 (the read-state hot path: Valkey hot markers +
//! batched PG flush; Valkey NEVER authoritative; cache loss is benign+bounded; unread is a bounded
//! range read). This is the CHAINED harness (EI-01 §4): seed a conversation → mark + flush → DROP the
//! cache mid-session → reconstruct from the durable record → recompute unread, all over the REAL
//! [`myelin_chat::ReadStateService`] + the in-memory [`MemHotTier`] message store + the [`InMemoryCache`]
//! Valkey floor. The live-Postgres + REAL Valkey leg is `tests/integration_chat_p16_read_state.rs`.

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

/// **CHAT-D12 (the chained drill): flush + drop Valkey mid-session → PG authoritative; a marker is
/// at-worst slightly stale; unread recomputes correctly.** The full lifecycle in one harness.
#[test]
fn chat_d12_flush_then_drop_valkey_mid_session_pg_authoritative() {
    // ── seed: a conversation with 8 messages; a session reading it ──
    let store = MemHotTier::new();
    let ids = seed(&store, 8);
    let svc = ReadStateService::new(InMemoryCache::new(), &store);

    // ── 1. the high-frequency write: scroll to ids[4], the hot marker is in the cache ──
    svc.mark_read(ReadMarker::new(conv(), "alice", ids[4].clone()));
    assert_eq!(
        svc.read_pos(&conv(), "alice"),
        Some(ids[4].clone()),
        "the hot marker is served from the cache mid-scroll"
    );
    // unread = the 3 messages strictly after ids[4] (ids[5], ids[6], ids[7]).
    assert_eq!(svc.unread_count(&conv(), "alice").unwrap(), 3);

    // ── 2. the batched flush (cadence ~seconds): the marker is now in the durable record (truth) ──
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

    // ── 3. DROP Valkey mid-session (the cache loss) ──
    svc.drop_cache(&conv(), "alice");

    // ── 4. the lost-read-state signal == 0: the PG record is authoritative — the marker reconstructs ──
    let lost_read_state = if svc.read_pos(&conv(), "alice") == Some(ids[4].clone()) {
        0
    } else {
        1
    };
    assert_eq!(
        lost_read_state, 0,
        "CHAT-D12: the lost-read-state signal is 0 (the PG record is authoritative)"
    );

    // ── 5. unread counts RECOMPUTE correctly from the durable truth + the log (still 3) ──
    assert_eq!(
        svc.unread_count(&conv(), "alice").unwrap(),
        3,
        "CHAT-D12: unread recomputes correctly after the cache loss (count(id > last_read))"
    );
}

/// **CHAT-D12 (the benign+bounded staleness): only the UN-FLUSHED tail is lost on a mid-session cache
/// drop — a marker is at-worst slightly stale, never wrong, never below the durable truth.**
#[test]
fn chat_d12_an_unflushed_marker_is_at_worst_slightly_stale() {
    let store = MemHotTier::new();
    let ids = seed(&store, 8);
    let svc = ReadStateService::new(InMemoryCache::new(), &store);

    // Read up to ids[2], flush (durable = ids[2]).
    svc.mark_read(ReadMarker::new(conv(), "alice", ids[2].clone()));
    svc.flush();

    // Read further to ids[6] but the flush cadence has NOT fired yet (the un-flushed tail).
    svc.mark_read(ReadMarker::new(conv(), "alice", ids[6].clone()));

    // DROP Valkey mid-session BEFORE the flush — the un-flushed advance is lost.
    svc.drop_cache(&conv(), "alice");

    // The marker falls back to the durable ids[2] — at-worst slightly stale (re-see ids[3..=6]),
    // never wrong, never below the durable truth.
    let reconstructed = svc.read_pos(&conv(), "alice").expect("durable marker");
    assert_eq!(
        reconstructed, ids[2],
        "the un-flushed advance is lost → fall back to the durable truth (slightly stale)"
    );
    assert!(
        reconstructed < ids[6],
        "the staleness is BOUNDED by the un-flushed window only (a few re-seen messages)"
    );
    // Unread recomputes correctly against the (slightly stale) durable position: ids[3..=7] = 5.
    assert_eq!(
        svc.unread_count(&conv(), "alice").unwrap(),
        5,
        "unread recomputes correctly from the durable truth (benign+bounded staleness)"
    );
}

/// **CHAT-D12 (the read-fanout invariant): an ambient post to a huge channel does 0 per-member unread
/// writes — unread is derived lazily, never write-fanned-out.**
#[test]
fn chat_d12_ambient_post_does_zero_per_member_unread_writes() {
    let store = MemHotTier::new();
    let svc = ReadStateService::new(InMemoryCache::new(), &store);
    // Posting to a 100k-member channel writes ZERO read-state rows (the celebrity-fanout property).
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
