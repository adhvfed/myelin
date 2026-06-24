//! # CDC pair — contract 10.1 (the Chat holder `erase`) + 11.4 (per-subject-DEK crypto-shred) +
//! 10.4 (the DSR cascade via the bus) for the Chat erase fan-out (CHAT-P22 / P-411, M4-C8)
//!
//! **The three contract halves this artifact proves (the prompt's GATE):**
//! - **10.1 — the Chat `PersonalDataHolder` erase path.** PROVIDER: the Chat erase fan-out
//!   ([`myelin_chat::ChatErasureCascade`]) produces a COMPLETE per-store receipt set (one per
//!   registered Chat holder store) + the aggregate [`myelin_gdpr::EraseReceipt`] the frozen 10.1
//!   `erase(scope)` returns. CONSUMER: the DSR-orchestrator-facing surface asserts the receipt set is
//!   complete (0 holders missed) and content-addressed.
//! - **11.4 — per-subject-DEK crypto-shred.** PROVIDER: the cascade destroys the subject's per-subject
//!   body/draft DEK over the ONE [`myelin_storage::kms::KmsEngine`]. CONSUMER: a read-side that finds
//!   every body the subject authored is now unrecoverable ciphertext (the GD-4 individual lever — the
//!   tenant's other subjects untouched).
//! - **10.4 — the DSR cascade via the bus (never a backdoor).** PROVIDER: the cascade emits a
//!   `chat.message.erased` event per record through the OUTBOX. CONSUMER: a derivative-facing reader
//!   sees the tombstones on the bus (the cascade Search/Refs/Notif consume) — Chat never writes their
//!   stores.
//!
//! The provider + consumer are the SAME frozen shapes (one KmsEngine, one holder trait, one outbox —
//! EI-01 §7), proven DB-free. The live-Postgres / live-bus round-trip is the `integration`-feature
//! artifact (the CHAT-P6 subject-DEK integration lever + the bus relay drills).

use myelin_chat::{
    aggregate_receipt, encrypt_body, is_body_unrecoverable, AuthorKind, ChatErasureCascade,
    ChatFreeText, ConversationId, MemDraftStore, MemHotTier, MessageId, MessageStore, NewMessage,
    ReadStateRecord, UnfurlCache, CHAT_ERASE_CASCADE_TOKEN,
};
use myelin_events::{
    Actor, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, OutboxTransaction, Timestamp,
};
use myelin_gdpr::{EraseScope, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::SubjectId;
use myelin_storage::kms::{DekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

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
fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
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
fn append(
    store: &MemHotTier,
    outbox: &OutboxStore,
    minter: &Arc<MonotonicMinter>,
    author: &str,
    nonce: &str,
) -> MessageId {
    let mut tx = begin(outbox, minter);
    let id = store
        .append(
            &mut tx,
            NewMessage {
                conv: conv(),
                thread_root_id: None,
                author: author.to_string(),
                author_kind: AuthorKind::Human,
                body_inline: b"body".to_vec(),
                body_nodes: Vec::new(),
                client_nonce: nonce.to_string(),
            },
        )
        .expect("append");
    tx.commit().expect("commit");
    id
}

// ───────────────────────────── 11.4 — per-subject-DEK crypto-shred ─────────────────────────────

/// **PROVIDER (11.4) + CONSUMER: the cascade crypto-shreds the subject's DEK so their body is
/// unrecoverable, while ANOTHER subject's body (distinct DEK) stays recoverable (the GD-4 individual
/// lever).**
#[test]
fn cdc_11_4_provider_consumer_per_subject_dek_individual_lever() {
    let kms = KmsEngine::new();
    // Two subjects' bodies, each under their OWN per-subject DEK.
    let ada = encrypt_body(
        &kms,
        &region(),
        &tenant(),
        &SubjectId::new("psn:ada"),
        ChatFreeText::BodyInline,
        b"ada's private body",
    )
    .expect("seal ada");
    let bo = encrypt_body(
        &kms,
        &region(),
        &tenant(),
        &SubjectId::new("psn:bo"),
        ChatFreeText::BodyInline,
        b"bo's private body",
    )
    .expect("seal bo");

    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();
    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);

    let mut tx = begin(&outbox, &minter);
    cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject: subject("psn:ada"),
            tenant: gdpr_tenant(),
        },
        &[],
    );
    tx.commit().expect("commit");

    // CONSUMER: ada's body is unrecoverable; bo's (distinct DEK) is untouched (individual lever).
    assert!(
        is_body_unrecoverable(&kms, &region(), &ada),
        "11.4: ada's per-subject DEK is shredded — 0 recoverable"
    );
    assert!(
        !is_body_unrecoverable(&kms, &region(), &bo),
        "11.4 GD-4: bo's distinct per-subject DEK is untouched — only ada was erased"
    );
}

// ───────────────────────────── 10.4 — the DSR cascade via the bus ─────────────────────────────

/// **PROVIDER (10.4): the cascade emits a `chat.message.erased` per record through the OUTBOX (the
/// bus + DSR path).** CONSUMER: a derivative-facing reader sees exactly those tombstones on the bus —
/// the only cross-subsystem effect (no backdoor).
#[test]
fn cdc_10_4_provider_consumer_cascade_rides_the_bus_not_a_backdoor() {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let m1 = append(&store, &outbox, &minter, "psn:ada", "n1");
    let m2 = append(&store, &outbox, &minter, "psn:ada", "n2");
    let before = outbox.committed_count();

    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();
    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
    let mut tx = begin(&outbox, &minter);
    let report = cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject: subject("psn:ada"),
            tenant: gdpr_tenant(),
        },
        &[(conv(), m1), (conv(), m2)],
    );
    tx.commit().expect("commit");

    // CONSUMER: exactly two chat.message.erased on the bus — the cascade the derivatives consume.
    let erased = outbox
        .committed_rows()
        .into_iter()
        .filter(|r| r.envelope.type_.0 == CHAT_ERASE_CASCADE_TOKEN)
        .count();
    assert_eq!(
        erased, 2,
        "10.4: two tombstones on the bus (the DSR cascade)"
    );
    assert_eq!(
        outbox.committed_count() - before,
        2,
        "10.4: the ONLY cross-subsystem effect is the bus events (no backdoor)"
    );
    assert!(report.cascade_published);
}

// ───────────────────────────── 10.1 — the holder erase path ─────────────────────────────

/// **PROVIDER (10.1): the cascade produces a COMPLETE per-store receipt set + the aggregate
/// `EraseReceipt`.** CONSUMER: the DSR-orchestrator asserts the set is complete (0 holders missed)
/// and the aggregate is content-addressed (the audit-log hash-link).
#[test]
fn cdc_10_1_provider_consumer_complete_holder_receipt_set() {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();
    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);

    let scope = EraseScope::Subject {
        subject: subject("psn:ada"),
        tenant: gdpr_tenant(),
    };
    let mut tx = begin(&outbox, &minter);
    let report = cascade.erase(&mut tx, &scope, &[]);
    tx.commit().expect("commit");

    // CONSUMER: the receipt set is complete + content-addressed; the aggregate folds it (10.1).
    assert!(
        report.receipts_complete(),
        "10.1: every registered Chat store got an erase receipt — 0 holders missed"
    );
    let agg = aggregate_receipt(&report, &scope);
    assert_eq!(agg.receipt.operation, "erase");
    assert!(
        agg.receipt.content_hash.starts_with("blake3:"),
        "10.1: the aggregate erase receipt is content-addressed (the audit hash-link)"
    );
    // The destroyed-key epoch is folded into the content-address (a receipt cannot claim an epoch it
    // did not address) — the post-restore re-erase audit trail (10.8).
    assert_eq!(agg.receipt.key_epoch_destroyed, report.destroyed_key_epoch);
}

/// **The crypto-shredded DEK is excluded from the backup (11.4 / §7.5) — the receipt's destroyed
/// epoch matches the DEK that no longer round-trips.** Ties the 10.1 receipt to the 11.4 shred: the
/// epoch the receipt records is the epoch of the DEK now absent from the backup snapshot.
#[test]
fn cdc_11_4_10_1_destroyed_epoch_matches_the_excluded_backup_dek() {
    let kms = KmsEngine::new();
    // Seal a body so the subject's DEK exists (with an epoch).
    let _col = encrypt_body(
        &kms,
        &region(),
        &tenant(),
        &SubjectId::new("psn:ada"),
        ChatFreeText::BodyInline,
        b"body",
    )
    .expect("seal");
    let dek_id = DekId::new(tenant(), KeyClass::Subject("psn:ada".into()));
    assert!(
        kms.backup_snapshot().iter().any(|(id, _)| id == &dek_id),
        "the DEK is in the backup before the erase"
    );

    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();
    let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
    let mut tx = begin(&outbox, &minter);
    let report = cascade.erase(
        &mut tx,
        &EraseScope::Subject {
            subject: subject("psn:ada"),
            tenant: gdpr_tenant(),
        },
        &[],
    );
    tx.commit().expect("commit");

    assert!(
        report.destroyed_key_epoch.is_some(),
        "the receipt records the destroyed epoch"
    );
    assert!(
        !kms.backup_snapshot().iter().any(|(id, _)| id == &dek_id),
        "the shredded DEK is excluded from the backup (stays dead across restore)"
    );
}
