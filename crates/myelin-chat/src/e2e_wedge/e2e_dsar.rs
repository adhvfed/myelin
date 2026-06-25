//! # `e2e_wedge::e2e_dsar` — Chat's E2E-4 leg: the CHAT-D8 erasure as a NAMED HOLDER in the
//! 0-holders-missed DSAR certificate (CHAT-P27 / P-501, M5)
//!
//! Chat's contribution to the whole-system **E2E-4 — a DSAR fan-out (the GDPR-by-construction proof)**
//! (testing-strategy §E2E-4). E2E-4 proves a single `dsr_submit` reaches **every** H1–H18 holder, erases
//! reliably (crypto-shred + pseudonym-shred + restrict), survives a backup-restore, and emits a
//! Merkle-proven certificate. Chat's leg is the **H5 holder**: chat's CHAT-D8 erasure appears as a
//! **named holder in the 0-holders-missed certificate** — chat erases with **0 recoverable PII** (incl.
//! backups) and a **complete holder-receipt set** (0 holders missed), and its tombstone cascade rides the
//! bus (no backdoor).
//!
//! The chained leg (chat's holder, as the whole-system fan-out reaches it):
//! 1. Seed the subject's PII into chat's holder (an authored message in the HOT partition + a COLD
//!    segment + the backup-carried sealed ciphertext + the read-state/draft footprint).
//! 2. The DSR fan-out reaches chat's H5 holder → [`ChatErasureCascade::erase`]: crypto-shred the
//!    per-subject DEK (hot + cold + backup unrecoverable), tombstone every authored record (the bus
//!    cascade the derivatives consume — no backdoor), purge the derived read-models, and return the
//!    **COMPLETE** holder-receipt set + the destroyed-key epoch (driving post-restore re-erasure, 10.8).
//! 3. Assert the gate: **0 recoverable PII** (hot + cold + backup) + **0 holders missed** (the receipt
//!    set is complete) + the cascade rode the bus → chat appears in the whole-system certificate green.
//!
//! This drives the SAME [`ChatErasureCascade::erase`] CHAT-D8 cascade — no second erase orchestrator
//! (EI-01 §7). The whole-system DSAR certificate (the storage/GDPR-service spine,
//! `myelin-storage::holder_fanout` / `myelin-gdpr-service::full_fanout`) calls chat's holder through the
//! H5 [`crate::holder::ChatHolder`] seam; this leg proves chat's holder is green within that fan-out.

use std::sync::Arc;

use myelin_events::{
    Actor, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, OutboxTransaction, Timestamp,
};
use myelin_gdpr::{EraseScope, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::{EncryptedColumn, SubjectId};
use myelin_storage::kms::{DekId, KeyClass, KmsEngine};
use myelin_tenancy::{Region, TenantId};

use crate::dek::{encrypt_body, ChatFreeText};
use crate::erase::{is_body_unrecoverable, ChatErasureCascade};
use crate::read_state::{ReadMarker, ReadStateRecord};
use crate::store::{
    AuthorKind, ConversationId, MemHotTier, MessageId, MessageStore, NewMessage, RangeCursor,
};
use crate::{Draft, DraftKey, DraftStore, MemDraftStore, UnfurlCache, CHAT_ERASE_CASCADE_TOKEN};

use super::{dsar_holder_green, ChatE2eArtifact};

/// The E2E scenario token chat's DSAR holder leg attests (chat is the H5 named holder of E2E-4).
pub const E2E_SCENARIO: &str = "E2E-4";

/// The subject whose PII the DSAR fan-out erases from chat's holder.
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
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:dsar".into())),
    }
}

fn begin(outbox: &OutboxStore, minter: &Arc<MonotonicMinter>) -> OutboxTransaction {
    outbox.begin(minter.clone(), ctx_base())
}

/// Seal a body under the subject's per-subject DEK (the same ciphertext the hot/cold/backup tiers hold).
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

/// **E2E-4 — drive chat's H5 holder leg of the whole-system DSAR fan-out end-to-end (CHAT-D8).** Seeds
/// the subject's PII into chat's holder across hot + cold + backups + derived read-models, runs the
/// CHAT-D8 erasure cascade, and asserts the gate: 0 recoverable PII (hot + cold + backup) + 0 holders
/// missed (complete receipt set) + the cascade rode the bus (no backdoor). Returns the named green
/// artifact: chat appears in the 0-holders-missed certificate green. Drives the SAME
/// [`ChatErasureCascade::erase`] cascade — no second erase orchestrator.
pub fn run_e2e_4_chat_dsar_holder() -> ChatE2eArtifact {
    let kms = KmsEngine::new();
    let store = MemHotTier::new();
    let outbox = OutboxStore::new();
    let minter = Arc::new(MonotonicMinter::new());
    let read_state = ReadStateRecord::new();
    let drafts = MemDraftStore::new();
    let cache = UnfurlCache::new();

    let mut leaks: u64 = 0;

    // ── SEED: the subject authors messages spanning the HOT partition AND a COLD segment. ──
    let m1 = append(&store, &outbox, &minter, "n1");
    let m2 = append(&store, &outbox, &minter, "n2");
    let m3 = append(&store, &outbox, &minter, "n3");
    let sealed = store.seal_before(&conv(), &m3).expect("seal cold");
    let cold_seeded = sealed == 2; // m1 + m2 sealed to the cold segment.

    // The sealed-body ciphertext the hot/cold/backup tiers all carry.
    let hot_body = seal_body(&kms, b"my private chat in the hot tier");
    let cold_body = seal_body(&kms, b"my private chat archived in the cold segment");
    let dek_id = DekId::new(tenant(), KeyClass::Subject(SUBJECT.into()));
    // The BACKUP carries the subject's DEK BEFORE the erase (so a naive restore could resurrect PII).
    let backup_before = kms.backup_snapshot();
    let backup_seeded = backup_before.iter().any(|(id, _)| id == &dek_id);
    // While the key lives, all three tiers' bodies are recoverable (the seed is real PII).
    let pii_recoverable_before = !is_body_unrecoverable(&kms, &region(), &hot_body)
        && !is_body_unrecoverable(&kms, &region(), &cold_body);

    // The subject's read-state + draft footprint.
    read_state.upsert(&ReadMarker::new(conv(), SUBJECT, m1.clone()));
    drafts.save(
        &DraftKey::new(conv().conversation_id, SUBJECT),
        &Draft::text("an unsent private draft"),
    );

    // ── THE DSR FAN-OUT REACHES CHAT'S H5 HOLDER: one cascade drives shred + tombstone + purge. ──
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

    // ── GATE (a): 0 RECOVERABLE PII ACROSS HOT + COLD + BACKUPS. ──
    let hot_unrecoverable = is_body_unrecoverable(&kms, &region(), &hot_body);
    let cold_unrecoverable = is_body_unrecoverable(&kms, &region(), &cold_body);
    // The crypto-shredded DEK is EXCLUDED from the backup snapshot → a restore cannot resurrect the body.
    let backup_after = kms.backup_snapshot();
    let backup_unrecoverable = !backup_after.iter().any(|(id, _)| id == &dek_id);
    if !hot_unrecoverable {
        leaks += 1;
    }
    if !cold_unrecoverable {
        leaks += 1;
    }
    if !backup_unrecoverable {
        leaks += 1;
    }

    // ── GATE (b): the records survive as tombstones (the fact survives, the body is shredded). ──
    let records_addressed = report.records_tombstoned == 3;
    let rows = store
        .range(&conv(), RangeCursor::Recent, 10)
        .expect("range across hot + cold");
    let records_survive = rows.len() == 3;

    // ── GATE (c): the cascade rode the BUS (no backdoor) — a chat.message.erased per record. ──
    let cascade_count = outbox
        .committed_rows()
        .into_iter()
        .filter(|r| r.envelope.type_.0 == CHAT_ERASE_CASCADE_TOKEN)
        .count();
    let cascade_bus_only = cascade_count == 3 && report.cascade_published;

    // ── GATE (d): the derived read-models are purged (0 footprint). ──
    let read_models_purged = read_state.load(&conv(), SUBJECT).is_none()
        && drafts
            .load(&DraftKey::new(conv().conversation_id, SUBJECT))
            .is_none();

    // ── GATE (e): the holder-receipt set is COMPLETE — 0 holders missed (chat is named in the
    //             certificate with 0 holders missed). The destroyed-key epoch is recorded (the
    //             post-restore re-erase audit trail, 10.8). ──
    let holder_green = dsar_holder_green(&report);

    let green = cold_seeded
        && backup_seeded
        && pii_recoverable_before
        && hot_unrecoverable
        && cold_unrecoverable
        && backup_unrecoverable
        && records_addressed
        && records_survive
        && cascade_bus_only
        && read_models_purged
        && holder_green;

    ChatE2eArtifact {
        scenario: E2E_SCENARIO,
        green,
        evidence: format!(
            "Chat H5 DSAR holder (CHAT-D8 named in the 0-holders-missed certificate): \
             0 recoverable PII (hot={hot_unrecoverable}, cold={cold_unrecoverable}, \
             backup={backup_unrecoverable}); records survive as tombstones \
             (addressed={records_addressed}, survive={records_survive}); cascade bus-only no-backdoor \
             ({cascade_count} chat.message.erased)={cascade_bus_only}; read-models purged \
             ={read_models_purged}; 0 holders missed (complete receipt set + destroyed-key epoch \
             recorded)={holder_green}; leaks={leaks}",
        ),
        leaks,
    }
}
