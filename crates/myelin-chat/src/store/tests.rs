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

#[test]
fn ulid_lexical_order_equals_mint_order() {
    let src = MonotonicUlidSource::new();
    let mut ids = Vec::new();
    for _ in 0..1000 {
        ids.push(src.mint());
    }
    for w in ids.windows(2) {
        assert!(w[0] < w[1], "ULID order broke: {:?} !< {:?}", w[0], w[1]);
    }
    assert!(ids.iter().all(|id| id.as_str().len() == 26));
}

#[test]
fn system_ulid_source_is_monotone_under_burst() {
    let src = SystemUlidSource::new();
    let ids: Vec<MessageId> = (0..5000).map(|_| src.mint()).collect();
    for w in ids.windows(2) {
        assert!(w[0] < w[1], "system ULID order broke under burst");
    }
}

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
    let all = store.range(&conv(), RangeCursor::Recent, 1000).unwrap();
    assert_eq!(all.len(), 50);
    let got: Vec<MessageId> = all.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(got, ids, "range order must equal append (ULID) order");
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

    let recent = store.range(&conv(), RangeCursor::Recent, 5).unwrap();
    assert_eq!(recent.len(), 5);
    assert_eq!(recent.first().unwrap().message_id, ids[15]);
    assert_eq!(recent.last().unwrap().message_id, ids[19]);

    let before = store
        .range(&conv(), RangeCursor::Before(ids[10].clone()), 5)
        .unwrap();
    let got: Vec<MessageId> = before.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(got, ids[5..10].to_vec());

    let after = store
        .range(&conv(), RangeCursor::After(ids[10].clone()), 1000)
        .unwrap();
    let got: Vec<MessageId> = after.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(got, ids[11..].to_vec());
}

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

#[test]
fn append_co_commits_a_real_chat_message_created_event() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();

    let mut t = tx(&ob, &m);
    let id = store
        .append(&mut t, new_msg("n0", "alice", "hello world"))
        .unwrap();
    assert_eq!(
        ob.outbox_depth(),
        0,
        "an open transaction has written nothing"
    );
    t.commit().unwrap();

    let rows = ob.committed_rows();
    assert_eq!(
        rows.len(),
        1,
        "exactly one event co-committed with the message"
    );
    let row = &rows[0];
    assert_eq!(
        row.envelope.type_.0, "chat.message.created",
        "the co-committed event is chat.message.created"
    );
    assert_eq!(
        row.aggregate.0, "channel:01J0CONV",
        "aggregate = the canonical channel partition (contract 2.3 - the CHAT-D2 ordering key)"
    );
    assert!(
        row.subject.0.contains("#message-") && row.subject.0.contains(id.as_str()),
        "subject is the stable message-<id> #sub anchor: {}",
        row.subject.0
    );
    let payload = row.envelope.payload.to_string();
    assert!(
        !payload.contains("hello world"),
        "the body bytes must NEVER ride the bus (references-not-payloads): {payload}"
    );
    assert!(
        payload.contains(id.as_str()) && payload.contains("alice"),
        "the payload carries the message + author refs only: {payload}"
    );
    assert!(
        !row.envelope.contains_personal_data,
        "the event is references-only (no inline PII envelope)"
    );
}

#[test]
fn chat_d13_aborted_append_writes_neither_message_nor_event() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();

    {
        let mut t = tx(&ob, &m);
        let _id = store
            .append(&mut t, new_msg("n0", "alice", "doomed"))
            .unwrap();
    }
    assert_eq!(
        ob.outbox_depth(),
        0,
        "CHAT-D13: an aborted transaction emits 0 phantom events"
    );
    assert_eq!(ob.committed_count(), 0, "CHAT-D13: no ghost outbox row");
    assert_eq!(ob.dead_letter_count(), 0);
}

#[test]
fn chat_d14_retried_nonce_co_commits_exactly_one_message_and_one_event() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();

    let first = append(&store, &ob, &m, new_msg("retry-nonce", "alice", "hello"));
    let again = append(
        &store,
        &ob,
        &m,
        new_msg("retry-nonce", "alice", "hello (retry)"),
    );

    assert_eq!(
        first, again,
        "CHAT-D14: a retried send dedups to one message id"
    );
    assert_eq!(
        store
            .range(&conv(), RangeCursor::Recent, 100)
            .unwrap()
            .len(),
        1,
        "CHAT-D14: message-count = 1"
    );
    let created: Vec<_> = ob
        .committed_rows()
        .into_iter()
        .filter(|r| r.envelope.type_.0 == "chat.message.created")
        .collect();
    assert_eq!(
        created.len(),
        1,
        "CHAT-D14: exactly one chat.message.created event (the retry emitted none)"
    );
}

#[test]
fn chat_d2_burst_from_many_gateways_preserves_per_conversation_total_order() {
    use std::sync::Arc;
    let store = Arc::new(MemHotTier::new());
    let ob = OutboxStore::new();
    let minter = Arc::new(myelin_events::MonotonicMinter::new());
    const N: usize = 64;

    let mut handles = Vec::new();
    for i in 0..N {
        let store = Arc::clone(&store);
        let ob = ob.clone();
        let minter = Arc::clone(&minter);
        handles.push(std::thread::spawn(move || {
            let mut t = ob.begin(minter, ctx_base());
            store
                .append(&mut t, new_msg(&format!("g{i}"), "alice", &format!("m{i}")))
                .unwrap();
            t.commit().unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let mut seqs: Vec<u64> = ob
        .committed_rows()
        .into_iter()
        .filter(|r| r.aggregate.0 == "channel:01J0CONV")
        .map(|r| r.seq)
        .collect();
    seqs.sort_unstable();
    let expected: Vec<u64> = (0..N as u64).collect();
    assert_eq!(
        seqs, expected,
        "CHAT-D2: the per-conversation seqs are the contiguous {{0..N}} set (0 ordering violations)"
    );

    let all = store.range(&conv(), RangeCursor::Recent, 1000).unwrap();
    assert_eq!(all.len(), N, "every burst send persisted exactly once");
    for w in all.windows(2) {
        assert!(
            w[0].message_id < w[1].message_id,
            "CHAT-D2: 0 out-of-order message ids within the conversation"
        );
    }
}

#[test]
fn chat_d2_out_of_order_edit_reconciles_to_stable_id_order() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();

    let a = append(&store, &ob, &m, new_msg("a", "alice", "first"));
    let _b = append(&store, &ob, &m, new_msg("b", "bob", "second"));
    let mut t = tx(&ob, &m);
    store
        .revise(&mut t, &a, b"first (edited)".to_vec(), Vec::new(), 0)
        .unwrap();
    t.commit().unwrap();

    let all = store.range(&conv(), RangeCursor::Recent, 100).unwrap();
    assert_eq!(all[0].message_id, a);
    assert_eq!(all[0].body_inline, b"first (edited)");
    assert!(
        all[0].message_id < all[1].message_id,
        "stable id order holds"
    );

    let types: Vec<String> = ob
        .committed_rows()
        .into_iter()
        .filter(|r| r.aggregate.0 == "channel:01J0CONV")
        .map(|r| r.envelope.type_.0.clone())
        .collect();
    assert!(types.contains(&"chat.message.created".to_string()));
    assert!(types.contains(&"chat.message.edited".to_string()));
}

#[test]
fn revise_and_tombstone_co_commit_their_lifecycle_events() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let id = append(&store, &ob, &m, new_msg("n", "alice", "v0"));

    let mut t = tx(&ob, &m);
    store
        .revise(&mut t, &id, b"v1".to_vec(), Vec::new(), 0)
        .unwrap();
    t.commit().unwrap();

    let mut t2 = tx(&ob, &m);
    store
        .tombstone(&mut t2, &id, TombstoneReason::SubjectErased)
        .unwrap();
    t2.commit().unwrap();

    let types: Vec<String> = ob
        .committed_rows()
        .into_iter()
        .map(|r| r.envelope.type_.0.clone())
        .collect();
    assert_eq!(
        types,
        vec![
            "chat.message.created".to_string(),
            "chat.message.edited".to_string(),
            "chat.message.erased".to_string(),
        ],
        "the lifecycle events co-commit in order: created → edited → erased"
    );
}

#[test]
fn failed_cas_revise_co_commits_no_event() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();
    let id = append(&store, &ob, &m, new_msg("n", "alice", "v0"));
    assert_eq!(ob.committed_count(), 1);

    let mut t = tx(&ob, &m);
    let err = store
        .revise(&mut t, &id, b"clobber".to_vec(), Vec::new(), 99)
        .unwrap_err();
    assert!(matches!(err, StoreError::CasConflict { .. }));
    t.commit().unwrap();
    assert_eq!(
        ob.committed_count(),
        1,
        "a refused CAS emits no chat.message.edited phantom"
    );
}

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

#[test]
fn writes_are_partitioned_by_tenant_region_zero_cross_region() {
    let store = MemHotTier::new();
    let (ob, m) = outbox();

    let fr = ConversationId::new("acme", "fr-par", "01J0CONV");
    let de = ConversationId::new("acme", "de-fra", "01J0CONV");

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

    assert_eq!(store.range(&de, RangeCursor::Recent, 100).unwrap().len(), 0);
    assert_eq!(store.range(&fr, RangeCursor::Recent, 100).unwrap().len(), 1);

    let other_tenant = ConversationId::new("globex", "fr-par", "01J0CONV");
    assert_eq!(
        store
            .range(&other_tenant, RangeCursor::Recent, 100)
            .unwrap()
            .len(),
        0
    );
}

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

    let before_seal = store.range(&conv(), RangeCursor::Recent, 1000).unwrap();

    let sealed = store.seal_before(&conv(), &ids[10]).unwrap();
    assert_eq!(sealed, 10);

    let after_seal = store.range(&conv(), RangeCursor::Recent, 1000).unwrap();
    assert_eq!(
        before_seal, after_seal,
        "cold reads are transparent - the view is identical"
    );

    let gap = store.resync_from(&conv(), &ids[4]).unwrap();
    let got: Vec<MessageId> = gap.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(
        got,
        ids[5..].to_vec(),
        "resync spans the cold/hot seam gap-free"
    );

    let page = store
        .range(&conv(), RangeCursor::Before(ids[12].clone()), 5)
        .unwrap();
    let got: Vec<MessageId> = page.iter().map(|m| m.message_id.clone()).collect();
    assert_eq!(got, ids[7..12].to_vec());
}

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

#[test]
fn cold_segments_refuse_unknown_attribution_and_lifecycle_codes() {
    let message = sample_cold_batch()
        .into_iter()
        .next()
        .expect("the cold-segment fixture has a message");
    let original = serde_json::to_value(super::SegmentRow::from(&message)).unwrap();

    for (field, invalid, consequence) in [
        ("author_kind", 255, "reattributed to a human"),
        ("state", 255, "resurrected as active"),
    ] {
        let mut row = original.clone();
        row[field] = serde_json::json!(invalid);
        let mut encoded = serde_json::to_vec(&row).unwrap();
        encoded.push(b'\n');

        let error = super::decode_segment(&encoded)
            .expect_err("an unknown durable enum code must fail the whole cold-segment read");
        assert!(
            error.to_string().contains("invalid"),
            "the corrupt row was refused instead of being {consequence}: {error}"
        );
    }
}

fn sample_cold_batch() -> Vec<Message> {
    (0..8u128)
        .map(|i| Message {
            message_id: MessageId::from_u128(i),
            conv: conv(),
            thread_root_id: if i % 3 == 0 {
                None
            } else {
                Some(MessageId::from_u128(0))
            },
            author: format!("subject-{i}"),
            author_kind: match i % 3 {
                0 => AuthorKind::Human,
                1 => AuthorKind::Agent,
                _ => AuthorKind::Service,
            },
            body_inline: format!("cold segment body {i} - the quick brown fox").into_bytes(),
            body_nodes: vec![i as u8, 0xAB, 0xCD],
            client_nonce: format!("nonce-{i}"),
            edited_seq: (i % 2) as i32,
            state: MessageState::Active,
        })
        .collect()
}

#[test]
fn chat_cold_blob_store_swap_is_byte_identical_fs_to_fs() {
    let fs_a = myelin_storage::FsBlobStore::new();
    let fs_b = myelin_storage::FsBlobStore::new();
    let tenant = TenantId("acme".into());
    let batch = sample_cold_batch();

    let verdict =
        super::chat_cold_blob_store_parity(&fs_a, &fs_b, &tenant, &batch).expect("parity runs");
    assert_eq!(
        verdict.fs_address, verdict.object_address,
        "BLAKE3-of-the-encoded-segment is backing-independent - the content address is identical"
    );
    assert!(
        verdict.byte_identical,
        "the cold-segment object-store swap is byte-identical to the fs floor (same address, same \
         decoded rows back from both backings) - the swap is behaviour-preserving (11.2)"
    );
}

#[test]
fn cold_segments_is_generic_over_the_blob_store_backing() {
    let default_tier: ColdSegments<myelin_storage::FsBlobStore> = ColdSegments::new();
    let explicit_tier: ColdSegments<myelin_storage::FsBlobStore> =
        ColdSegments::with_blob_store(myelin_storage::FsBlobStore::new());

    let batch = sample_cold_batch();
    default_tier.seal(&conv(), batch.clone()).unwrap();
    explicit_tier.seal(&conv(), batch.clone()).unwrap();

    let from_default = default_tier.read(&conv()).unwrap();
    let from_explicit = explicit_tier.read(&conv()).unwrap();
    assert_eq!(
        from_default, from_explicit,
        "the cold tier reads identically regardless of the BlobStore backing - the swap is a \
         construction-time backing change, not a code change"
    );
    assert_eq!(
        from_default, batch,
        "the cold read round-trips the rows verbatim"
    );
}

struct DivergentBlob {
    inner: myelin_storage::FsBlobStore,
    bad_address: bool,
    bad_bytes: bool,
    last_real: std::sync::Mutex<Option<myelin_storage::ContentHash>>,
}

impl myelin_storage::BlobStore for DivergentBlob {
    fn put(
        &self,
        tenant: &TenantId,
        bytes: &[u8],
    ) -> myelin_storage::blob::Result<myelin_storage::ContentHash> {
        let real = self.inner.put(tenant, bytes)?;
        *self.last_real.lock().unwrap() = Some(real.clone());
        if self.bad_address {
            Ok(myelin_storage::ContentHash::blake3(
                b"a different payload entirely",
            ))
        } else {
            Ok(real)
        }
    }
    fn get(
        &self,
        tenant: &TenantId,
        hash: &myelin_storage::ContentHash,
    ) -> myelin_storage::blob::Result<Vec<u8>> {
        let lookup = if self.bad_address {
            self.last_real
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(hash.clone())
        } else {
            hash.clone()
        };
        let real = self.inner.get(tenant, &lookup)?;
        if self.bad_bytes {
            Ok(super::encode_segment(&[Message {
                message_id: super::MessageId::from_u128(999),
                conv: conv(),
                thread_root_id: None,
                author: "someone-else".into(),
                author_kind: AuthorKind::Human,
                body_inline: b"a different body".to_vec(),
                body_nodes: Vec::new(),
                client_nonce: "different".into(),
                edited_seq: 0,
                state: MessageState::Active,
            }]))
        } else {
            Ok(real)
        }
    }
    fn head(
        &self,
        tenant: &TenantId,
        hash: &myelin_storage::ContentHash,
    ) -> myelin_storage::blob::Result<myelin_storage::blob::BlobMeta> {
        self.inner.head(tenant, hash)
    }
    fn delete(
        &self,
        tenant: &TenantId,
        hash: &myelin_storage::ContentHash,
    ) -> myelin_storage::blob::Result<()> {
        self.inner.delete(tenant, hash)
    }
}

#[test]
fn cold_blob_parity_verdict_is_load_bearing_per_conjunct() {
    let tenant = TenantId("acme".into());
    let batch = sample_cold_batch();
    let fs = myelin_storage::FsBlobStore::new();

    let honest = DivergentBlob {
        inner: myelin_storage::FsBlobStore::new(),
        bad_address: false,
        bad_bytes: false,
        last_real: std::sync::Mutex::new(None),
    };
    assert!(
        super::chat_cold_blob_store_parity(&fs, &honest, &tenant, &batch)
            .unwrap()
            .byte_identical,
        "an honest backing is byte-identical"
    );

    let bad_addr = DivergentBlob {
        inner: myelin_storage::FsBlobStore::new(),
        bad_address: true,
        bad_bytes: false,
        last_real: std::sync::Mutex::new(None),
    };
    assert!(
        !super::chat_cold_blob_store_parity(&fs, &bad_addr, &tenant, &batch)
            .unwrap()
            .byte_identical,
        "a divergent content address makes the swap NOT byte-identical (the conjunct is load-bearing)"
    );

    let bad_read = DivergentBlob {
        inner: myelin_storage::FsBlobStore::new(),
        bad_address: false,
        bad_bytes: true,
        last_real: std::sync::Mutex::new(None),
    };
    assert!(
        !super::chat_cold_blob_store_parity(&fs, &bad_read, &tenant, &batch)
            .unwrap()
            .byte_identical,
        "a corrupt read-back makes the swap NOT byte-identical (the conjunct is load-bearing)"
    );
}
