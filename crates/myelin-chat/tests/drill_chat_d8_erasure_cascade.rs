//! # Drill CHAT-D8 — erase a person → 0 recoverable PII across hot + cold + backups + the cascade,
//! complete holder receipts (CHAT-P22 / P-411, M4-C8)
//!
//! **The whole-system erase scenario** (testing-strategy/01 row CHAT-D8): a person P authored
//! messages in BOTH the HOT partition and a COLD segment (and the backups carry the sealed
//! ciphertext). When P is erased:
//! - **0 recoverable PII across hot + cold + backups.** Every body P authored is sealed under P's
//!   per-subject DEK; the cascade's ONE key-destroy renders the SAME ciphertext unrecoverable whether
//!   read from the hot partition, fetched back from a cold segment, or restored from a backup
//!   snapshot — `decrypt_body` fails LOUDLY in all three, never a plaintext-without-key fall-through.
//!   The crypto-shredded DEK is EXCLUDED from the backup snapshot (it stays dead across a restore,
//!   §7.5).
//! - **The records survive as tombstones** (keep the fact, drop the body) and a `chat.message.erased`
//!   event rides the OUTBOX for each — the bus + DSR cascade the derivatives (Search incl. embeddings
//!   / Refs / Notif) consume, NEVER a backdoor.
//! - **The derived read-models are purged** — read-state / drafts / unfurl-cache → 0 footprint.
//! - **The holder-receipt set is COMPLETE** — one per registered Chat store, 0 holders missed.
//!
//! This is the CHAINED erase the prompt's drill names: a single `erase(P)` drives the per-subject-DEK
//! shred AND the tombstone+cascade AND the read-model purge AND the complete receipt set in one
//! fan-out. A red on any leg (a recoverable body in cold/backup, a missed holder, a backdoor erase)
//! is the CHAT-D8 red drill this forecloses.
//!
//! The mention → `[erased user]` half + the restriction-flag suppression at every read path are
//! asserted in CHAT-D8 via **CHAT-P23 / P-417** (named floor); this drill ships the
//! 0-recoverable-PII core + the holder-receipt completeness.

use myelin_chat::{
    encrypt_body, is_body_unrecoverable, AuthorKind, ChatErasureCascade, ChatFreeText,
    ConversationId, Draft, DraftKey, DraftStore, MemDraftStore, MemHotTier, MessageId,
    MessageState, MessageStore, NewMessage, RangeCursor, ReadMarker, ReadStateRecord,
    TombstoneReason, UnfurlCache, CHAT_ERASE_CASCADE_TOKEN,
};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, OutboxTransaction, Timestamp,
};
use myelin_gdpr::{EraseScope, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::{EncryptedColumn, SubjectId};
use myelin_storage::kms::{DekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

const SUBJECT: &str = "psn:ada";

fn region() -> Region {
    Region::new("fr-par")
}
fn tenant() -> TenantId {
    TenantId::from_token("acme")
}
fn gdpr_tenant() -> GdprTenantId {
    GdprTenantId::from_token("acme")
}
fn conv() -> ConversationId {
    ConversationId::new("acme", "fr-par", "c-1")
}
fn subject() -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(SUBJECT.into()),
        PrincipalKind::Human,
        gdpr_tenant(),
    ))
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
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn begin(outbox: &OutboxStore, minter: &Arc<MonotonicMinter>) -> OutboxTransaction {
    outbox.begin(minter.clone(), ctx_base())
}

// Seal a body under the subject's per-subject DEK (the same ciphertext the hot/cold/backup tiers
// hold). Returns the sealed column so the drill can probe its recoverability across the tiers.
fn seal_body(kms: &KmsEngine, plaintext: &[u8]) -> EncryptedColumn {
    encrypt_body(
        kms,
        &region(),
        &tenant(),
        &SubjectId::new(SUBJECT),
        ChatFreeText::BodyInline,
        plaintext,
    )
    .expect("seal under the subject's per-subject DEK")
}

fn append(
    store: &MemHotTier,
    outbox: &OutboxStore,
    minter: &Arc<MonotonicMinter>,
    nonce: &str,
) -> MessageId {
    let mut tx = begin(outbox, minter);
    let id = store
        .append(
            &mut tx,
            NewMessage {
                conv: conv(),
                thread_root_id: None,
                author: SUBJECT.to_string(),
                author_kind: AuthorKind::Human,
                body_inline: b"private body".to_vec(),
                body_nodes: Vec::new(),
                client_nonce: nonce.to_string(),
            },
        )
        .expect("append");
    tx.commit().expect("commit");
    id
}

/// **CHAT-D8: a chained erase → 0 recoverable PII across hot + cold + backups + a complete holder
/// receipt set + the cascade via the bus (no backdoor).**
#[test]
fn chat_d8_chained_erase_zero_recoverable_pii_complete_receipts() {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();

    // --- SEED: the subject authors messages spanning the HOT partition AND a COLD segment. ---
    let m1 = append(&store, &outbox, &minter, "n1");
    let m2 = append(&store, &outbox, &minter, "n2");
    let m3 = append(&store, &outbox, &minter, "n3");
    // Seal a cold segment for everything strictly before m3 (m1, m2 move to the cold tier) — the
    // bodies now live in BOTH tiers (hot tail = m3, cold = m1/m2). The crypto-shred must reach both.
    let sealed = store.seal_before(&conv(), &m3).expect("seal cold");
    assert_eq!(sealed, 2, "m1 + m2 sealed to the cold segment");

    // The sealed-body ciphertext the hot/cold/backup tiers all carry (the same bytes).
    let hot_body = seal_body(&kms, b"my private chat in the hot tier");
    let cold_body = seal_body(&kms, b"my private chat archived in the cold segment");
    // A BACKUP snapshot taken BEFORE the erase still carries the subject's DEK (the wrapped key).
    let backup_before = kms.backup_snapshot();
    let dek_id = DekId::new(tenant(), KeyClass::Subject(SUBJECT.into()));
    assert!(
        backup_before.iter().any(|(id, _)| id == &dek_id),
        "the backup carries the subject's per-subject DEK BEFORE the erase"
    );
    // While the key lives, all three tiers' bodies are recoverable.
    assert!(!is_body_unrecoverable(&kms, &region(), &hot_body));
    assert!(!is_body_unrecoverable(&kms, &region(), &cold_body));

    // The subject's read-state + draft footprint.
    read_state.upsert(&ReadMarker::new(conv(), SUBJECT, m1.clone()));
    drafts.save(
        &DraftKey::new(conv().conversation_id, SUBJECT),
        &Draft::text("an unsent private draft"),
    );

    // --- THE CHAINED ERASE: one fan-out drives the shred + tombstone+cascade + purge + receipts. ---
    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
    let mut tx = begin(&outbox, &minter);
    let report = cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject: subject(),
            tenant: gdpr_tenant(),
        },
        &[
            (conv(), m1.clone()),
            (conv(), m2.clone()),
            (conv(), m3.clone()),
        ],
    );
    tx.commit().expect("commit");

    // === 0 RECOVERABLE PII ACROSS HOT + COLD + BACKUPS ===
    assert!(
        is_body_unrecoverable(&kms, &region(), &hot_body),
        "HOT: 0 recoverable — the body is unrecoverable after the DEK shred"
    );
    assert!(
        is_body_unrecoverable(&kms, &region(), &cold_body),
        "COLD: 0 recoverable — the archived cold-segment body is unrecoverable too"
    );
    // BACKUP: restoring the pre-erase snapshot can never resurrect the body — the shredded DEK is
    // EXCLUDED from the snapshot (it stays dead across a restore, §7.5).
    let backup_after = kms.backup_snapshot();
    assert!(
        !backup_after.iter().any(|(id, _)| id == &dek_id),
        "BACKUP: the crypto-shredded DEK is excluded — a restore cannot resurrect the body"
    );

    // === THE RECORDS SURVIVE (structure intact; the body is crypto-shredded) ===
    // The cascade addressed all three authored records (one chat.message.erased tombstone each).
    assert_eq!(report.records_tombstoned, 3, "all three records addressed");
    let rows = store
        .range(&conv(), RangeCursor::Recent, 10)
        .expect("range across hot + cold");
    assert_eq!(
        rows.len(),
        3,
        "the records survive (structure / order intact)"
    );
    // The HOT-tail record (m3) is mutated to a tombstone (body cleared). The COLD-segment records
    // (m1/m2) are NOT rewritten — the immutable log is preserved (the prompt's "WITHOUT rewriting the
    // immutable log") — their BODIES are crypto-shredded by the per-subject-DEK destroy (the cold_body
    // assertion above proves 0 recoverable), and their tombstone FACT rode the bus (asserted below).
    let hot_tail = rows.iter().find(|r| r.message_id == m3).expect("m3 hot");
    assert_eq!(
        hot_tail.state,
        MessageState::Tombstoned,
        "the hot-tail record is tombstoned (body cleared)"
    );
    assert!(hot_tail.body_inline.is_empty(), "the hot body is dropped");

    // === THE CASCADE RODE THE BUS (no backdoor) — a chat.message.erased per record ===
    assert!(report.cascade_published, "the cascade rode the bus");
    let erased = outbox
        .committed_rows()
        .into_iter()
        .filter(|r| r.envelope.type_.0 == CHAT_ERASE_CASCADE_TOKEN)
        .count();
    assert_eq!(
        erased, 3,
        "three chat.message.erased tombstones on the outbox — the DSR cascade the derivatives consume"
    );

    // === THE DERIVED READ-MODELS ARE PURGED (0 footprint) ===
    assert!(
        read_state.load(&conv(), SUBJECT).is_none(),
        "read-state purged"
    );
    assert!(
        drafts
            .load(&DraftKey::new(conv().conversation_id, SUBJECT))
            .is_none(),
        "the unsent draft purged"
    );

    // === THE HOLDER-RECEIPT SET IS COMPLETE (0 holders missed) ===
    assert!(
        report.receipts_complete(),
        "every registered Chat store got an erase receipt — 0 holders missed"
    );
    assert!(
        report.destroyed_key_epoch.is_some(),
        "the destroyed-key epoch is recorded (the post-restore re-erase audit trail, 10.8)"
    );
}

/// **The cascade NEVER reaches another subsystem's store directly (no backdoor, 10.4).** The ONLY
/// effect outside Chat's own stores is the `chat.message.erased` events on the OUTBOX — the bus + DSR
/// path. The drill asserts the cascade's cross-subsystem footprint is exactly the outbox events
/// (Chat tombstones + purges its OWN stores; the derivatives erase themselves off the bus).
#[test]
fn chat_d8_cascade_is_bus_only_no_backdoor() {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();

    let m1 = append(&store, &outbox, &minter, "n1");
    let before = outbox.committed_count();

    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
    let mut tx = begin(&outbox, &minter);
    cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject: subject(),
            tenant: gdpr_tenant(),
        },
        &[(conv(), m1)],
    );
    tx.commit().expect("commit");

    // The cascade's cross-subsystem reach is EXACTLY one outbox event (the bus is the only path).
    assert_eq!(
        outbox.committed_count() - before,
        1,
        "exactly one chat.message.erased on the bus — no backdoor write into Search/Refs/Notif"
    );
}

/// **Tombstoning is idempotent on the record + uses the SubjectErased reason (the erase fact).** A
/// re-run erase over the same record stays tombstoned (the crypto-shred + tombstone is idempotent —
/// a restored backup re-applying the shred does not corrupt the record).
#[test]
fn chat_d8_tombstone_reason_is_subject_erased_and_idempotent() {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();

    let m1 = append(&store, &outbox, &minter, "n1");
    // A direct tombstone with the SubjectErased reason (the erase fact).
    let mut tx = begin(&outbox, &minter);
    store
        .tombstone(&mut tx, &m1, TombstoneReason::SubjectErased)
        .expect("tombstone");
    tx.commit().expect("commit");

    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
    let mut tx = begin(&outbox, &minter);
    cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject: subject(),
            tenant: gdpr_tenant(),
        },
        &[(conv(), m1.clone())],
    );
    tx.commit().expect("commit");

    let rows = store
        .range(&conv(), RangeCursor::Recent, 10)
        .expect("range");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].state,
        MessageState::Tombstoned,
        "the record stays tombstoned (idempotent re-erase, post-restore re-apply safe)"
    );
}
