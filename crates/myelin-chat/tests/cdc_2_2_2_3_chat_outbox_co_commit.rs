//! **CHAT-P5 / P-399 — the CONSUMER-side CDC pair for the outbox co-commit Chat's message send
//! rides: row 2.2 (`OutboxTx::emit` — the only emit path) + row 2.3 (the `outbox` table +
//! per-aggregate ordering, `UNIQUE(aggregate, seq)`).**
//!
//! The PROVIDER is the Bus: the frozen `OutboxTx::emit(draft, cause)` co-commit surface
//! (`myelin_events::OutboxStore` / `OutboxTransaction`) + the per-aggregate commit-order `seq`
//! allocation. Chat is the CONSUMER: its `MessageStore::append` / `revise` / `tombstone` co-commit a
//! real `chat.message.*` event onto the caller's transaction via `emit`, stamping
//! `aggregate = conversation_id` so per-conversation events stay per-aggregate ordered (the 2.3
//! ordering key Chat's CHAT-D2 total-order property builds on).
//!
//! This file carries BOTH a provider-side and a consumer-side marker (the contract-coverage
//! scanner's CDC-pair requirement): the PROVIDER shape is the Bus outbox emit surface, exercised
//! here as the CONSUMER (the chat store) drives it through a real message send. DB-free — the
//! in-memory `OutboxStore` models the 2.3 table semantics byte-for-byte (the live-Postgres co-commit
//! leg is `tests/integration_chat_p5_co_commit.rs`, behind the `integration` feature).

use std::sync::Arc;

use myelin_chat::store::{
    AuthorKind, ConversationId, MemHotTier, MessageStore, NewMessage, TombstoneReason,
};
use myelin_events::{
    Actor, EmitContextBase, MonotonicMinter, OutboxStore, OutboxTransaction, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        caused_by: None,
    }
}

fn outbox() -> (OutboxStore, Arc<MonotonicMinter>) {
    (OutboxStore::new(), Arc::new(MonotonicMinter::new()))
}

fn tx(store: &OutboxStore, minter: &Arc<MonotonicMinter>) -> OutboxTransaction {
    store.begin(minter.clone(), ctx_base())
}

fn conv() -> ConversationId {
    ConversationId::new("acme", "fr-par", "01J0CONVCDC")
}

fn new_msg(nonce: &str, body: &str) -> NewMessage {
    NewMessage {
        conv: conv(),
        thread_root_id: None,
        author: "alice".into(),
        author_kind: AuthorKind::Human,
        body_inline: body.as_bytes().to_vec(),
        body_nodes: Vec::new(),
        client_nonce: nonce.into(),
    }
}

/// **PROVIDER (Bus 2.2) ⇄ CONSUMER (Chat store):** a Chat message send co-commits its
/// `chat.message.created` event through the frozen `OutboxTx::emit` surface — the message persist
/// and the event are ONE transaction (emit-iff-committed). The provider's emit surface accepts
/// Chat's draft; the consumer (the store) is the only emit path (no raw publish).
#[test]
fn chat_send_co_commits_via_the_frozen_outbox_emit_surface() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();

    let mut t = tx(&ob, &m);
    let id = store.append(&mut t, new_msg("n0", "hi")).unwrap();
    // emit-iff-committed: before commit the provider has written nothing.
    assert_eq!(ob.outbox_depth(), 0);
    t.commit().unwrap();

    let rows = ob.committed_rows();
    assert_eq!(rows.len(), 1, "exactly one co-committed event");
    // The CONSUMER stamped a registered durable chat token + the conversation aggregate (2.3).
    assert_eq!(rows[0].envelope.type_.0, "chat.message.created");
    assert_eq!(rows[0].aggregate.0, "01J0CONVCDC");
    assert!(rows[0].subject.0.contains(id.as_str()));
}

/// **PROVIDER (Bus 2.3) ⇄ CONSUMER (Chat store):** `aggregate = conversation_id`, so a chained
/// send → edit → erase for one conversation carries monotonic, gap-free, per-aggregate seqs (the
/// per-conversation total order, contract 2.3 / the CHAT-D2 / D-9 property). The provider allocates
/// the seq at commit; the consumer supplies the conversation aggregate.
#[test]
fn per_conversation_events_are_per_aggregate_ordered() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();

    let mut t = tx(&ob, &m);
    let id = store.append(&mut t, new_msg("n0", "v0")).unwrap();
    t.commit().unwrap();

    let mut t2 = tx(&ob, &m);
    store
        .revise(&mut t2, &id, b"v1".to_vec(), Vec::new(), 0)
        .unwrap();
    t2.commit().unwrap();

    let mut t3 = tx(&ob, &m);
    store
        .tombstone(&mut t3, &id, TombstoneReason::SubjectErased)
        .unwrap();
    t3.commit().unwrap();

    let rows: Vec<_> = ob
        .committed_rows()
        .into_iter()
        .filter(|r| r.aggregate.0 == "01J0CONVCDC")
        .collect();
    let seqs: Vec<u64> = rows.iter().map(|r| r.seq).collect();
    assert_eq!(
        seqs,
        vec![0, 1, 2],
        "monotonic, gap-free per-aggregate seqs"
    );
    let types: Vec<&str> = rows.iter().map(|r| r.envelope.type_.0.as_str()).collect();
    assert_eq!(
        types,
        vec![
            "chat.message.created",
            "chat.message.edited",
            "chat.message.erased"
        ],
        "the conversation's lifecycle events are per-aggregate ordered"
    );
}
