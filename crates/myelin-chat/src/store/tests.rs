//! Unit tests for the CHAT-P4 `MessageStore` trait + tiers (the DB-free, behaviour-identical
//! floor). The PG hot tier proves the SAME surface against the live dev stack in
//! `tests/integration_chat_p4_message_store.rs` (the 0-divergence GATE's PG leg).

use std::sync::Arc;

use myelin_events::{Actor, CausedBy, EmitContextBase, OutboxStore, OutboxTransaction, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

use super::*;

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
    ConversationId::new("acme", "fr-par", "01J0CONV")
}

fn new_msg(nonce: &str, author: &str, body: &str) -> NewMessage {
    NewMessage {
        conv: conv(),
        thread_root_id: None,
        author: author.into(),
        author_kind: AuthorKind::Human,
        body_inline: body.as_bytes().to_vec(),
        body_nodes: Vec::new(),
        client_nonce: nonce.into(),
    }
}

fn append(
    store: &MemHotTier,
    ob: &OutboxStore,
    m: &Arc<myelin_events::MonotonicMinter>,
    msg: NewMessage,
) -> MessageId {
    let mut t = tx(ob, m);
    let id = store.append(&mut t, msg).expect("append");
    t.commit().expect("commit");
    id
}

// ── ULID: lexical order == mint order (k-sortable; the per-conversation total order) ────────────

#[test]
fn ulid_lexical_order_equals_mint_order() {
    let src = MonotonicUlidSource::new();
    let mut ids = Vec::new();
    for _ in 0..1000 {
        ids.push(src.mint());
    }
    // Each minted id sorts strictly after the previous one (0 out-of-order ids).
    for w in ids.windows(2) {
        assert!(w[0] < w[1], "ULID order broke: {:?} !< {:?}", w[0], w[1]);
    }
    // The rendered form is the canonical 26-char ULID width.
    assert!(ids.iter().all(|id| id.as_str().len() == 26));
}

#[test]
fn system_ulid_source_is_monotone_under_burst() {
    // The wall-clock source's monotonic guard: even when many mints land in the same ms, order is
    // never violated.
    let src = SystemUlidSource::new();
    let ids: Vec<MessageId> = (0..5000).map(|_| src.mint()).collect();
    for w in ids.windows(2) {
        assert!(w[0] < w[1], "system ULID order broke under burst");
    }
}

// ── append → range round-trip, ULID monotone per conversation ───────────────────────────────────

#[test]
fn append_then_range_is_ulid_ordered() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let mut ids = Vec::new();
    for i in 0..50 {
        ids.push(append(
            &store,
            &ob,
            &m,
            new_msg(&format!("n{i}"), "alice", &format!("msg {i}")),
        ));
    }
    // resync from the very start sees every message, gap-free, in mint order.
    let all = store.range(&conv(), RangeCursor::Recent, 1000).unwrap();
    assert_eq!(all.len(), 50);
    let got: Vec<MessageId> = all.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(got, ids, "range order must equal append (ULID) order");
    // 0 out-of-order ids within the conversation.
    for w in got.windows(2) {
        assert!(w[0] < w[1]);
    }
}

#[test]
fn range_recent_before_after_cursors() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let ids: Vec<MessageId> = (0..20)
        .map(|i| {
            append(
                &store,
                &ob,
                &m,
                new_msg(&format!("n{i}"), "a", &format!("m{i}")),
            )
        })
        .collect();

    // Recent-N: the last 5.
    let recent = store.range(&conv(), RangeCursor::Recent, 5).unwrap();
    assert_eq!(recent.len(), 5);
    assert_eq!(recent.first().unwrap().message_id, ids[15]);
    assert_eq!(recent.last().unwrap().message_id, ids[19]);

    // Before(ids[10]): scroll-back page of 5 ending just before ids[10] → ids[5..10].
    let before = store
        .range(&conv(), RangeCursor::Before(ids[10].clone()), 5)
        .unwrap();
    let got: Vec<MessageId> = before.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(got, ids[5..10].to_vec());

    // After(ids[10]): the resume gap → everything strictly after.
    let after = store
        .range(&conv(), RangeCursor::After(ids[10].clone()), 1000)
        .unwrap();
    let got: Vec<MessageId> = after.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(got, ids[11..].to_vec());
}

// ── resync_from: the resume backbone, gap-free, ordered ─────────────────────────────────────────

#[test]
fn resync_from_is_gap_free_after_cursor() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let ids: Vec<MessageId> = (0..30)
        .map(|i| {
            append(
                &store,
                &ob,
                &m,
                new_msg(&format!("n{i}"), "a", &format!("m{i}")),
            )
        })
        .collect();
    let gap = store.resync_from(&conv(), &ids[9]).unwrap();
    let got: Vec<MessageId> = gap.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(
        got,
        ids[10..].to_vec(),
        "resync_from must be everything after the cursor, gap-free"
    );
}

// ── idempotent send: a retried nonce returns the existing id (no second row) ─────────────────────

#[test]
fn idempotent_send_on_client_nonce() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let first = append(&store, &ob, &m, new_msg("same-nonce", "a", "hello"));
    let again = append(&store, &ob, &m, new_msg("same-nonce", "a", "hello (retry)"));
    assert_eq!(first, again, "a retried send dedups to one message");
    assert_eq!(
        store
            .range(&conv(), RangeCursor::Recent, 100)
            .unwrap()
            .len(),
        1
    );
}

// ── revise: CAS-on-edit, stable id, refused clobber ─────────────────────────────────────────────

#[test]
fn revise_bumps_edited_seq_under_cas_and_keeps_id() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let id = append(&store, &ob, &m, new_msg("n", "a", "v0"));

    let mut t = tx(&ob, &m);
    store
        .revise(&mut t, &id, b"v1".to_vec(), Vec::new(), 0)
        .unwrap();
    t.commit().unwrap();

    let msg = &store.range(&conv(), RangeCursor::Recent, 1).unwrap()[0];
    assert_eq!(msg.message_id, id, "the id is STABLE across edits");
    assert_eq!(msg.edited_seq, 1);
    assert_eq!(msg.body_inline, b"v1");
    assert_eq!(msg.state, MessageState::Edited);

    // A stale expect_seq is a refused clobber (CAS conflict), not a silent overwrite.
    let mut t2 = tx(&ob, &m);
    let err = store
        .revise(&mut t2, &id, b"v2".to_vec(), Vec::new(), 0)
        .unwrap_err();
    assert_eq!(
        err,
        StoreError::CasConflict {
            message_id: id,
            expected: 0,
            actual: 1
        }
    );
}

// ── tombstone: keep the fact, drop the body ─────────────────────────────────────────────────────

#[test]
fn tombstone_keeps_the_record_clears_the_body() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let id = append(&store, &ob, &m, new_msg("n", "a", "secret"));

    let mut t = tx(&ob, &m);
    store
        .tombstone(&mut t, &id, TombstoneReason::SubjectErased)
        .unwrap();
    t.commit().unwrap();

    let all = store.range(&conv(), RangeCursor::Recent, 100).unwrap();
    assert_eq!(all.len(), 1, "the record survives (the fact is kept)");
    assert_eq!(all[0].state, MessageState::Tombstoned);
    assert!(all[0].body_inline.is_empty(), "the body is dropped");
}

// ── partition / residency-pin: a write lands ONLY in its (tenant, region) partition ─────────────

#[test]
fn writes_are_partitioned_by_tenant_region_zero_cross_region() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();

    let fr = ConversationId::new("acme", "fr-par", "01J0CONV");
    let de = ConversationId::new("acme", "de-fra", "01J0CONV"); // same conv id, DIFFERENT region

    let mut t = tx(&ob, &m);
    store
        .append(
            &mut t,
            NewMessage {
                conv: fr.clone(),
                ..new_msg("a", "alice", "fr message")
            },
        )
        .unwrap();
    t.commit().unwrap();

    // The de-fra partition is a DIFFERENT partition key → 0 rows (the residency-pin holds: a write
    // never crosses regions; a read in another region sees none of it).
    assert_eq!(store.range(&de, RangeCursor::Recent, 100).unwrap().len(), 0);
    assert_eq!(store.range(&fr, RangeCursor::Recent, 100).unwrap().len(), 1);

    // A different tenant is a different partition too (0 cross-tenant).
    let other_tenant = ConversationId::new("globex", "fr-par", "01J0CONV");
    assert_eq!(
        store
            .range(&other_tenant, RangeCursor::Recent, 100)
            .unwrap()
            .len(),
        0
    );
}

// ── the cold tier: seal a prefix, reads stay IDENTICAL (transparent cold reads) ─────────────────

#[test]
fn cold_seal_is_transparent_to_range_and_resync() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let ids: Vec<MessageId> = (0..20)
        .map(|i| {
            append(
                &store,
                &ob,
                &m,
                new_msg(&format!("n{i}"), "a", &format!("m{i}")),
            )
        })
        .collect();

    // The full ordered view BEFORE sealing.
    let before_seal = store.range(&conv(), RangeCursor::Recent, 1000).unwrap();

    // Seal the first 10 to the cold object segment (the detach job).
    let sealed = store.seal_before(&conv(), &ids[10]).unwrap();
    assert_eq!(sealed, 10);

    // The trait surface is IDENTICAL whether messages are hot or cold (0 behavioural divergence):
    // the same ordered view, the same resync, the same recent-N.
    let after_seal = store.range(&conv(), RangeCursor::Recent, 1000).unwrap();
    assert_eq!(
        before_seal, after_seal,
        "cold reads are transparent — the view is identical"
    );

    // resync across the cold/hot seam is still gap-free and ordered.
    let gap = store.resync_from(&conv(), &ids[4]).unwrap();
    let got: Vec<MessageId> = gap.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(
        got,
        ids[5..].to_vec(),
        "resync spans the cold/hot seam gap-free"
    );

    // a scroll-back page that straddles the seam returns the cold rows transparently.
    let page = store
        .range(&conv(), RangeCursor::Before(ids[12].clone()), 5)
        .unwrap();
    let got: Vec<MessageId> = page.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(got, ids[7..12].to_vec());
}

// ── the cold segment round-trips the body bytes verbatim (the store is body-opaque) ─────────────

#[test]
fn cold_segment_round_trips_body_verbatim() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let mut msg = new_msg("n", "alice", "the quick brown fox");
    msg.body_nodes = vec![1, 2, 3, 4, 5];
    msg.author_kind = AuthorKind::Agent;
    let mut t = tx(&ob, &m);
    let id = store.append(&mut t, msg).unwrap();
    t.commit().unwrap();

    store
        .seal_before(&conv(), &MessageId::from_u128(u128::MAX))
        .unwrap();

    let read = store.range(&conv(), RangeCursor::Recent, 1).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].message_id, id);
    assert_eq!(read[0].body_inline, b"the quick brown fox");
    assert_eq!(read[0].body_nodes, vec![1, 2, 3, 4, 5]);
    assert_eq!(read[0].author_kind, AuthorKind::Agent);
}
