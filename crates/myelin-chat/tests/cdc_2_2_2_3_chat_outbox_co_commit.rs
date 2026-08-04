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

#[test]
fn chat_send_co_commits_via_the_frozen_outbox_emit_surface() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();

    let mut t = tx(&ob, &m);
    let id = store.append(&mut t, new_msg("n0", "hi")).unwrap();
    assert_eq!(ob.outbox_depth(), 0);
    t.commit().unwrap();

    let rows = ob.committed_rows();
    assert_eq!(rows.len(), 1, "exactly one co-committed event");
    assert_eq!(rows[0].envelope.type_.0, "chat.message.created");
    assert_eq!(rows[0].aggregate.0, "01J0CONVCDC");
    assert!(rows[0].subject.0.contains(id.as_str()));
}

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
