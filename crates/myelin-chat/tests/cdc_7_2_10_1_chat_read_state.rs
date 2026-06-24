//! # CDC pair — contract 7.2 (read-state truth) + 10.1 (the read-state `PersonalDataHolder`) for the
//! Chat Read-state Service (CHAT-P16 / P-410, M4-C5)
//!
//! **The two halves this artifact proves (the prompt's GATE):**
//! - **7.2 — read-state truth (CONSUMED).** PROVIDER: the Chat Read-state Service
//!   ([`myelin_chat::ReadStateService`]) is the ONE read-state truth for a Chat conversation — the
//!   per-`(user × conversation)` last-read marker. A mark flushed to the durable record is the SAME
//!   value a later read returns; the cache is a write-back tier in front of it (cache-never-
//!   authoritative). CONSUMER: a read-side (`read_pos` / `unread_count`) that reads the ONE truth —
//!   cache-first, durable-record-authoritative — and recomputes unread as a bounded range read, never
//!   a second store. Chat's per-channel scroll-state is LINKED to Notif's per-item state at the
//!   mention (§5.3), never a third copy (the one-read-state-truth posture, C-9).
//! - **10.1 — the read-state `PersonalDataHolder` (H5).** PROVIDER: the read-state durable record is
//!   its OWN Chat store ([`myelin_chat::CHAT_READ_STATE_STORE`]) auto-registered through the harness
//!   ONE door + classified to the exhaustive **H5 (`H5Chat`)**. CONSUMER: the DSR-facing surface — the
//!   read-state markers PURGE on erasure (a person's scroll-position footprint, D-C8), and the holder
//!   completeness assertion sees 0 orphan Chat stores (the read-state store cannot silently miss the
//!   DSR fan-out).
//!
//! The provider + consumer are the SAME frozen shapes (one read-state service, one holder registry —
//! EI-01 §7), proven against the in-memory message store + the in-memory cache (DB-free). The
//! live-Postgres durable record + REAL Valkey hot markers are the `integration`-feature artifact
//! (`tests/integration_chat_p16_read_state.rs`, the CHAT-D12 real-data leg).

use myelin_chat::{
    chat_store_classifier, register_chat_holders, AuthorKind, ConversationId, MemHotTier,
    MessageId, MessageStore, NewMessage, ReadMarker, ReadStateService, CHAT_OLTP_STORE,
    CHAT_READ_STATE_STORE,
};
use myelin_events::{Actor, CausedBy, EmitContextBase, OutboxStore, OutboxTransaction, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::cache::InMemoryCache;
use myelin_substrate::{
    assert_holder_completeness, classify_store, Holder, HolderRegistry, StoreKind,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

// ───────────────────────────── harness ─────────────────────────────────────────────────────────────

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

// ───────────────────────────── 7.2 — read-state truth (cache-never-authoritative) ─────────────────

/// **PROVIDER (7.2): the read-state service is the ONE truth — a flushed mark is the value a later
/// read returns, and the durable record is authoritative on a cache loss.**
#[test]
fn provider_read_state_is_one_truth_pg_authoritative() {
    let store = MemHotTier::new();
    let ids = seed(&store, 5);
    let svc = ReadStateService::new(InMemoryCache::new(), &store);

    svc.mark_read(ReadMarker::new(conv(), "alice", ids[2].clone()));
    svc.flush();
    // The durable record (the truth) holds the marked position.
    assert_eq!(svc.record().load(&conv(), "alice"), Some(ids[2].clone()));

    // Drop the cache — the PG record is authoritative, the marker reconstructs (7.2: one truth).
    svc.drop_cache(&conv(), "alice");
    assert_eq!(
        svc.read_pos(&conv(), "alice"),
        Some(ids[2].clone()),
        "7.2: the read-state truth survives a cache loss (PG authoritative)"
    );
}

/// **CONSUMER (7.2): a read-side recomputes unread as a bounded range read against the ONE truth —
/// never a second store, never a cached number.** Read up to ids[2] → unread = 2; after a cache loss
/// the unread RECOMPUTES correctly from the durable record + the log.
#[test]
fn consumer_unread_is_a_bounded_range_read_over_the_one_truth() {
    let store = MemHotTier::new();
    let ids = seed(&store, 5);
    let svc = ReadStateService::new(InMemoryCache::new(), &store);

    svc.mark_read(ReadMarker::new(conv(), "bob", ids[2].clone()));
    svc.flush();
    assert_eq!(svc.unread_count(&conv(), "bob").unwrap(), 2);

    // After a cache loss the unread is recomputed against the durable truth (still 2) — not a stale
    // cached count (cache-never-authoritative).
    svc.drop_cache(&conv(), "bob");
    assert_eq!(
        svc.unread_count(&conv(), "bob").unwrap(),
        2,
        "7.2: unread recomputes from the ONE durable truth after a cache loss"
    );

    // And an ambient post writes 0 per-member unread rows (read-fanout, never write-fanout).
    assert_eq!(svc.ambient_post_unread_writes(100_000), 0);
}

// ───────────────────────────── 10.1 — the read-state PersonalDataHolder (H5) ───────────────────────

/// **PROVIDER (10.1): the read-state store auto-registers as holder H5 + classifies — 0 orphans.**
/// The read-state durable record opens through the harness ONE door, so it is a registered H5 holder
/// by construction (the DSR fan-out cannot miss the per-user read-state markers, D-C8).
#[test]
fn provider_read_state_store_registers_and_classifies_to_h5() {
    let registry = register_chat_holders();
    assert!(
        registry.is_registered(StoreKind::Oltp, CHAT_READ_STATE_STORE),
        "the read-state store registered as a holder"
    );
    let classifier = chat_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, CHAT_READ_STATE_STORE, &classifier),
        Some(Holder::H5Chat),
        "10.1: the Chat read-state store is holder H5"
    );
    // The OLTP message store + the read-state store both classify (0 orphans).
    assert_eq!(
        classify_store(StoreKind::Oltp, CHAT_OLTP_STORE, &classifier),
        Some(Holder::H5Chat)
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "10.1: every Chat store (incl. read-state) is in the exhaustive H1–H18 list — 0 orphans"
    );
}

/// **CONSUMER (10.1): the DSR fan-out purges a principal's read-state markers on erasure (D-C8).**
/// Erase a principal → their `(conversation, principal)` markers are removed (0 recoverable
/// read-state footprint); another principal's markers are untouched.
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
    let _ = HolderRegistry::new(); // (the holder surface is exercised via register_chat_holders above)
}
