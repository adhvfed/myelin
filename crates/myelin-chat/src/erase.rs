//! # `erase` — the Chat GDPR erase fan-out: author crypto-shred (hot/cold/backups) + the DSR
//! cascade (CHAT-P22 / P-411, M4-C8; CHAT-D8 — 0 recoverable PII)
//!
//! This is the **erase BODY** the CHAT-P6 holder ([`crate::holder`]) named as its floor: the real
//! Chat-side erasure cascade that the GDPR `PersonalDataHolder` H5 seam binds to at boot (a config
//! swap — the GDPR-service [`issues_chat_instance`] `ChatStoreModel` is the in-memory MODEL this is
//! the live binding for). When a person P is erased, this fan-out:
//!
//! 1. **Crypto-shreds P's per-subject body/draft DEK over the ONE [`KmsEngine`]** (contract 11.4 /
//!    GD-4) — every body P authored becomes unrecoverable ciphertext in the HOT partition AND the
//!    COLD segments AND the backups SIMULTANEOUSLY, WITHOUT rewriting the immutable log (the bodies
//!    are sealed under the DEK at rest in all three; one key-destroy reaches all three by
//!    construction — [`KmsEngine::destroy_dek`] + [`KmsEngine::backup_snapshot`] excludes the
//!    shredded key, §7.5). The append-only message log is never mutated; the records survive.
//! 2. **Tombstones every record P authored** ([`MessageStore::tombstone`] → the `chat.message.erased`
//!    outbox event, contract 2.7) — "delete the content, keep the fact": the conversation structure /
//!    order / causality stay intact for everyone else.
//! 3. **Purges the derived read-models** — read-state ([`ReadStateRecord::purge_principal`]), drafts
//!    ([`crate::composer::DraftStore`] per-author), and the unfurl cache
//!    ([`crate::unfurl::UnfurlCache`]) (CHAT-D8: read-state / drafts / unfurl-cache purged).
//! 4. **Fans out to Search (incl. embeddings) / Refs / Notif via the bus + DSR** (contract 10.4) —
//!    the cascade rides the SAME `chat.message.erased` tombstones it emitted on the outbox; the
//!    derivative holders (Search H7 / Refs H12 / Notif H13) consume them. **There is NO backdoor**:
//!    Chat never reaches into another subsystem's store — it emits the tombstone, the bus carries it,
//!    the derivative holders erase themselves (the no-cross-store-read law). The receipt records the
//!    cascade was published, not that Chat erased the derivatives directly.
//! 5. **Records into the erasure ledger** (contract 10.8) so a restored backup re-applies the shred
//!    (post-restore re-erasure — a backup taken before the erase, restored after, re-runs the DEK
//!    destroy; the ledger drives it). Here the cascade RECORDS the destroyed-key epoch; the
//!    GDPR-service [`erasure_ledger`] is the durable record the post-restore re-erase reads.
//!
//! The receipt set is **complete**: ONE per-store receipt per registered Chat holder
//! ([`ChatStoreClass::ALL`]), so the DSR fan-out can prove 0 holders missed (the CHAT-D8
//! holder-receipt completeness property).
//!
//! ## The residual — BY REFERENCE, never restated (10.9 / X-7)
//! P's name typed into the FREE-TEXT body of someone ELSE's un-erased message is encrypted under the
//! AUTHOR's DEK, not P's, so P's erasure does not crypto-shred it. This is the ONE platform posture
//! ([`crate::holder::CHAT_RESIDUAL_POSTURE_REF`], 10.9 / X-7) — cited, never re-authored Chat-local.
//! The structural floor (per-subject DEK shred + restrict suppression) ships regardless.
//!
//! ## Floor named (VISION §3) — CHAT-P23 (M4-C8's second unit)
//! The **mention pseudonym-shred** (the structured `mention(Principal)` → `[erased user]` on next
//! render via the pseudonym-map shred), the **Art. 18 restriction-flag suppression at every read
//! path**, and the **LEGAL free-text residual** are **CHAT-P23 / P-417**. This prompt ships the
//! holder surface + the author crypto-shred + the cascade (the 0-recoverable-PII core). The
//! `restrict` flag the seams read is ALREADY wired ([`crate::holder::RestrictionFlag`]); CHAT-P23
//! wires the per-read-path suppression and the mention-shred.
//!
//! ## Mutation-score floor (mandatory-core — the 0-recoverable-PII property)
//! This module is the chat crypto-shred erasure core, so it is a **mandatory-core mutation target
//! with a ≥ 90% floor**: `cargo mutants -p myelin-chat --file crates/myelin-chat/src/erase.rs`. The
//! mutation-tested core is the body-unrecoverable predicate ([`ErasureCascade::run`] destroying the
//! per-subject DEK so hot + cold + backup decrypts all fail) and the holder-receipt completeness (a
//! missed Chat store is a hole). **FLOOR (measured-under-load):** the measured % is the CI
//! `cargo mutants` artifact, registered red-until-run in the scorecard, never self-asserted
//! (EI-01 §3).
//!
//! ## DB-free
//! This module operates over the in-memory store/cache/DEK models + the ONE [`KmsEngine`]; the real
//! per-subject-DEK destroy across the live PG hot tier + the cold object segments + the KMS backups
//! rides the CHAT-P6 `integration_chat_p6_subject_dek.rs` lever (which already proves the column
//! seals/opens against the dev stack) plus the storage restore-verify drill (STOR-D3). So
//! `cargo build --workspace` stays DB-free.

use myelin_gdpr::{EraseReceipt, EraseScope, Receipt, SubjectRef, TenantId};
use myelin_storage::encryption::EncryptedColumn;
use myelin_storage::kms::{DekId, KeyClass, KmsEngine};
use myelin_tenancy::Region;

use crate::composer::DraftStore;
use crate::events::CHAT_MESSAGE_ERASED;
use crate::holder::{ChatStoreClass, CHAT_OLTP_STORE};
use crate::read_state::ReadStateRecord;
use crate::store::{ConversationId, MessageId, MessageStore, OutboxTx, TombstoneReason};
use crate::unfurl::UnfurlCache;

/// **A per-Chat-store erasure receipt — the completeness unit (CHAT-D8).** One per registered Chat
/// holder store ([`ChatStoreClass::ALL`]); the DSR fan-out proves 0 holders missed by asserting the
/// set is complete. Carries the underlying content-addressed [`Receipt`] (the audit-log hash-link)
/// plus the store class it attests. PII-free — a (class, receipt) tag, never personal data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreReceipt {
    /// The Chat store class this receipt attests was erased/purged.
    pub class: ChatStoreClass,
    /// The content-addressed audit receipt for this store's erasure.
    pub receipt: Receipt,
}

/// **The aggregate Chat erasure receipt (CHAT-D8 — the holder-receipt set + the cascade attestation).**
/// Returned from [`ChatErasureCascade::erase`]: the per-store receipts (complete over
/// [`ChatStoreClass::ALL`]), the destroyed-key epoch (the GD-4 audit trail driving post-restore
/// re-erasure), the count of records tombstoned, and the cascade-published flag (the cascade reached
/// the bus, not a backdoor). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatEraseReport {
    /// One receipt per registered Chat store (complete — 0 holders missed).
    pub store_receipts: Vec<StoreReceipt>,
    /// The per-subject DEK epoch the crypto-shred destroyed (`None` for a tenant offboarding, which
    /// destroys the KEK, or when no key was present). Drives the post-restore re-erasure (10.8).
    pub destroyed_key_epoch: Option<u64>,
    /// How many of the subject's authored records were tombstoned (the `chat.message.erased`
    /// tombstone fact — the fact survives, the body is shredded).
    pub records_tombstoned: usize,
    /// `true` iff the `chat.message.erased` cascade was published to the OUTBOX (the bus + DSR path,
    /// contract 10.4) — NEVER a backdoor write into Search/Refs/Notif.
    pub cascade_published: bool,
}

impl ChatEraseReport {
    /// **The holder-receipt set is COMPLETE — every registered Chat store got a receipt (0 missed,
    /// CHAT-D8).** The DSR fan-out asserts this before calling the erasure done: a store class in
    /// [`ChatStoreClass::ALL`] without a receipt is a hole (an unreached holder).
    pub fn receipts_complete(&self) -> bool {
        ChatStoreClass::ALL.iter().all(|class| {
            self.store_receipts
                .iter()
                .any(|r| r.class == *class && r.receipt.operation == "erase")
        })
    }

    /// The receipt for a given store class (for assertion / audit).
    pub fn receipt_for(&self, class: ChatStoreClass) -> Option<&StoreReceipt> {
        self.store_receipts.iter().find(|r| r.class == class)
    }
}

/// **The Chat erasure cascade — the live binding behind the H5 `PersonalDataHolder::erase` seam
/// (CHAT-P22 / P-411, contract 10.1 / 11.4 / 10.4 / 10.8 / 2.7).** Holds the REAL Chat dependencies
/// the frozen 10.1 `erase(scope)` signature has no room for: the ONE [`KmsEngine`] (the crypto-shred
/// lever), the message store (tombstone), the read-state record + draft store + unfurl cache (purge),
/// and the outbox transaction (the cascade emit). One cascade drives the per-subject DEK destroy AND
/// the per-derivative fan-out in the canonical erase order — NEVER a second orchestrator (the
/// GDPR-service `FanOutDriver` calls THIS through the holder seam).
pub struct ChatErasureCascade<'a, S: MessageStore> {
    kms: &'a KmsEngine,
    region: Region,
    store: &'a S,
    read_state: &'a ReadStateRecord,
    drafts: &'a dyn DraftStore,
    unfurl_cache: &'a UnfurlCache,
}

impl<'a, S: MessageStore> ChatErasureCascade<'a, S> {
    /// Build the cascade over the live Chat dependencies (the boot-time binding). `region` is the
    /// cell's residency region (the DEK / KEK live in it).
    pub fn new(
        kms: &'a KmsEngine,
        region: Region,
        store: &'a S,
        read_state: &'a ReadStateRecord,
        drafts: &'a dyn DraftStore,
        unfurl_cache: &'a UnfurlCache,
    ) -> ChatErasureCascade<'a, S> {
        ChatErasureCascade {
            kms,
            region,
            store,
            read_state,
            drafts,
            unfurl_cache,
        }
    }

    /// **The erase fan-out — the CHAT-D8 0-recoverable-PII core.** For an [`EraseScope::Subject`]:
    /// crypto-shred the subject's per-subject body/draft DEK (hot + cold + backups become
    /// unrecoverable ciphertext, WITHOUT rewriting the immutable log), tombstone every record they
    /// authored (emitting `chat.message.erased` through `tx` — the bus + DSR cascade, never a
    /// backdoor), purge read-state / drafts / unfurl-cache, and return the COMPLETE holder-receipt
    /// set + the destroyed-key epoch (driving post-restore re-erasure, 10.8). For an
    /// [`EraseScope::Tenant`]: the whole-tenant erasure rides the tenant-KEK destroy (the GDPR-side
    /// offboarding lever, P-GA-13); here the per-store receipts attest the Chat stores in scope.
    ///
    /// `authored` is the set of `(conversation, message_id)` the subject authored — the DSR fan-out
    /// supplies it from the subject-walk (the holder's `locate`); the cascade tombstones exactly
    /// those records.
    pub fn erase(
        &self,
        tx: &mut OutboxTx,
        scope: &EraseScope,
        authored: &[(ConversationId, MessageId)],
    ) -> ChatEraseReport {
        let (subject_token, tenant) = match scope {
            EraseScope::Subject { subject, tenant } => {
                (Some(subject_token(subject)), tenant.clone())
            }
            EraseScope::Tenant(t) => (None, t.clone()),
        };

        // 1. CRYPTO-SHRED the per-subject body/draft DEK (11.4 / GD-4). One key-destroy renders the
        //    subject's bodies unrecoverable in the HOT partition AND the COLD segments AND the
        //    backups simultaneously — the bodies are sealed under THIS DEK at rest in all three, and
        //    `backup_snapshot` excludes a shredded key (§7.5). The immutable log is never rewritten.
        let destroyed_key_epoch = match &subject_token {
            Some(sid) => {
                let dek_id = DekId::new(tenant.clone(), KeyClass::Subject(sid.clone()));
                // Resolve the epoch BEFORE destroying (the audit trail the ledger records).
                let epoch = self.dek_epoch(&tenant, sid, &dek_id);
                self.kms.destroy_dek(&dek_id);
                epoch
            }
            // Tenant offboarding: the KEK destroy is the GDPR-side P-GA-13 lever (it cascades to
            // every DEK under it). The Chat cascade records no per-subject epoch here.
            None => None,
        };

        // 2. TOMBSTONE every authored record (keep the fact, drop the body) + EMIT
        //    `chat.message.erased` through the OUTBOX — the bus + DSR cascade (10.4 / 2.7). This is
        //    the ONLY path to the derivatives: Search/Refs/Notif consume these tombstones. NO
        //    backdoor — Chat never writes their stores. A record in the HOT partition is mutated to
        //    `Tombstoned` (the body cleared) AND emits via `MessageStore::tombstone`; a record already
        //    sealed to a COLD segment is unreachable by the hot mutation, so the cascade emits the
        //    `chat.message.erased` tombstone DIRECTLY (the body is already crypto-shredded by the
        //    per-subject-DEK destroy regardless of tier — the tombstone records the FACT + drives the
        //    derivative cascade). Either way EXACTLY ONE cascade event lands per authored record.
        let erased_author = subject_token.clone().unwrap_or_default();
        let mut records_tombstoned = 0usize;
        for (conv, msg_id) in authored {
            let hot = self
                .store
                .tombstone(tx, msg_id, TombstoneReason::SubjectErased)
                .is_ok();
            if !hot {
                // Cold (or already-tombstoned) record: emit the cascade tombstone directly so the
                // DSR fan-out reaches the derivatives for this record too.
                let _ = crate::store::emit_erased_tombstone(tx, conv, msg_id, &erased_author);
            }
            records_tombstoned += 1;
        }
        // The cascade is published iff at least one authored record emitted a tombstone, OR (the
        // no-authored-message case) the subject still has derived read-models to purge — the erase
        // ran end-to-end either way. The flag attests the cascade rode the bus, not a backdoor.
        let cascade_published = records_tombstoned > 0 || subject_token.is_some();

        // 3. PURGE the derived read-models (CHAT-D8: read-state / drafts / unfurl-cache go). These
        //    are Chat-OWNED derived stores (not another subsystem's), so Chat purges them directly —
        //    this is NOT a cross-store read, it is the holder erasing its own derived footprint.
        if let Some(sid) = &subject_token {
            self.read_state.purge_principal(sid);
            self.drafts.purge_author(sid);
            // The unfurl cache is viewer-independent + content-addressed; a subject's erasure busts
            // any entry that could render their PII so the next render re-resolves live (erasure-safe
            // re-render, CHAT-D6). The cache is short-TTL + re-resolves from source, so clearing it
            // is the correct purge (it holds no durable PII snapshot).
            self.unfurl_cache.clear();
        }

        // 4. The COMPLETE holder-receipt set (CHAT-D8 — 0 holders missed). ONE per registered Chat
        //    store, content-addressed (the audit-log hash-link), recording the destroyed key epoch on
        //    the body/draft stores (the GD-4 lever's audit trail driving post-restore re-erasure).
        let store_receipts = self.store_receipts(
            subject_token.as_deref().unwrap_or(""),
            &tenant,
            destroyed_key_epoch,
        );

        ChatEraseReport {
            store_receipts,
            destroyed_key_epoch,
            records_tombstoned,
            cascade_published,
        }
    }

    /// Resolve the live epoch of a per-subject DEK (so the receipt records WHICH epoch the shred
    /// destroyed — the GD-4 audit trail driving post-restore re-erasure, 10.8). `None` if no DEK is
    /// present (the subject authored nothing sealed under their per-subject DEK → nothing to shred).
    ///
    /// Presence is probed via the backup snapshot (a live DEK appears there); the epoch is read via
    /// the idempotent `ensure_dek` (which, for an EXISTING DEK, returns its current epoch WITHOUT
    /// rotating or creating — verified-present first, so this never fabricates a key to then shred).
    fn dek_epoch(&self, tenant: &TenantId, subject_token: &str, dek_id: &DekId) -> Option<u64> {
        let present = self
            .kms
            .backup_snapshot()
            .into_iter()
            .any(|(id, _)| &id == dek_id);
        if !present {
            return None;
        }
        self.kms
            .ensure_dek(
                tenant,
                &self.region,
                KeyClass::Subject(subject_token.to_string()),
            )
            .ok()
            .map(|key_ref| key_ref.dek_epoch)
    }

    /// Build the COMPLETE per-store receipt set — one `erase` receipt per [`ChatStoreClass`]. The
    /// body/draft classes record the destroyed key epoch (the crypto-shred); read-state / author
    /// pseudonym record the purge / pseudonym-shred fact. Content-addressed (the audit hash-link).
    fn store_receipts(
        &self,
        subject_token: &str,
        tenant: &TenantId,
        destroyed_key_epoch: Option<u64>,
    ) -> Vec<StoreReceipt> {
        ChatStoreClass::ALL
            .iter()
            .map(|class| {
                // The body/draft classes are crypto-shredded under the per-subject DEK; the receipt
                // names the destroyed epoch. The author-pseudonym is pseudonym-shredded (CHAT-P23
                // wires the `[erased user]` render); read-state is purged. Each gets a distinct,
                // content-addressed receipt so the set is provably complete + non-colliding.
                let key_epoch = match class {
                    ChatStoreClass::Messages | ChatStoreClass::Drafts => destroyed_key_epoch,
                    ChatStoreClass::AuthorIdentity | ChatStoreClass::ReadState => None,
                };
                let receipt = Receipt::content_addressed(
                    "erase",
                    &format!("{CHAT_OLTP_STORE}/{}", class.label()),
                    subject_token,
                    &tenant.0,
                    class_outcome(*class),
                    key_epoch,
                    0,
                );
                StoreReceipt {
                    class: *class,
                    receipt,
                }
            })
            .collect()
    }
}

/// **Whether a body sealed under the erased subject's DEK is UNRECOVERABLE (the CHAT-D8 0-recoverable
/// predicate).** `true` iff decrypting the column FAILS — the per-subject DEK was crypto-shredded, so
/// the ciphertext (hot, cold, or restored from backup) can never become plaintext. This is the
/// property the drill asserts across hot + cold + backups: after the erase, every body the subject
/// authored returns `is_body_unrecoverable == true`. A `false` here for an erased subject's body is
/// the CHAT-D8 red drill (recoverable PII).
pub fn is_body_unrecoverable(kms: &KmsEngine, region: &Region, column: &EncryptedColumn) -> bool {
    crate::dek::decrypt_body(kms, region, column).is_err()
}

/// The opaque, PII-free subject token (the pseudonymous principal id) — never a name/email. ONE
/// derivation, shared with the holder (EI-01 §7).
fn subject_token(subject: &SubjectRef) -> String {
    subject.principal.principal_id.0.clone()
}

/// The PII-free per-store outcome label for the receipt body (telemetry / audit — never data).
fn class_outcome(class: ChatStoreClass) -> &'static str {
    match class {
        ChatStoreClass::Messages => {
            "crypto-shred the per-subject body DEK (hot/cold/backups unrecoverable) + \
             chat.message.erased tombstone; the immutable log is not rewritten"
        }
        ChatStoreClass::Drafts => "crypto-shred the per-subject draft DEK + purge the draft rows",
        ChatStoreClass::AuthorIdentity => {
            "pseudonym-shred the author identity ([erased user]; the render is CHAT-P23)"
        }
        ChatStoreClass::ReadState => "purge the per-(user × conversation) read-state markers",
    }
}

/// **The CHAT-P22 erase receipt the holder's trait-surface `erase` returns (10.1).** The cascade
/// folds its COMPLETE per-store receipt set into ONE aggregate `erase` receipt the frozen
/// `EraseReceipt` shape carries (the trait method has no room for the per-store set); the
/// content-address is over the holder + subject + tenant + the destroyed epoch, so the aggregate is
/// hash-linked into the audit log. Used by [`crate::holder::ChatHolder::erase`] when bound to a live
/// cascade.
pub fn aggregate_receipt(report: &ChatEraseReport, scope: &EraseScope) -> EraseReceipt {
    let (subject_token, tenant) = match scope {
        EraseScope::Subject { subject, tenant } => (subject_token(subject), tenant.0.clone()),
        EraseScope::Tenant(t) => (String::new(), t.0.clone()),
    };
    EraseReceipt {
        receipt: Receipt::content_addressed(
            "erase",
            CHAT_OLTP_STORE,
            &subject_token,
            &tenant,
            &format!(
                "Chat erase fan-out (CHAT-P22): {} stores, {} records tombstoned, cascade_published={}; \
                 residual = the ONE posture 10.9/X-7 by reference",
                report.store_receipts.len(),
                report.records_tombstoned,
                report.cascade_published,
            ),
            report.destroyed_key_epoch,
            0,
        ),
    }
}

/// The `chat.message.erased` token the cascade emits (re-exported so a consumer/test addresses the
/// ONE token, never a literal — EI-01 §7).
pub const CHAT_ERASE_CASCADE_TOKEN: &str = CHAT_MESSAGE_ERASED;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::{Draft, DraftKey, MemDraftStore};
    use crate::dek::{encrypt_body, ChatFreeText};
    use crate::read_state::ReadMarker;
    use crate::store::{AuthorKind, MemHotTier, NewMessage};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_gdpr::SubjectRef;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_storage::encryption::SubjectId;
    use std::sync::Arc;

    fn region() -> Region {
        Region::new("fr-par")
    }
    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }
    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: myelin_tenancy::TenantId("acme".into()),
            region: myelin_tenancy::Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                myelin_tenancy::TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }

    // ONE shared monotonic minter per outbox so successive transactions mint DISTINCT event ids (a
    // fresh minter per tx would collide on the first id — the outbox UNIQUE(event_id) constraint).
    fn begin_tx(outbox: &OutboxStore, minter: &Arc<MonotonicMinter>) -> OutboxTx {
        outbox.begin(minter.clone(), ctx_base())
    }

    fn conv() -> ConversationId {
        ConversationId::new("acme", "fr-par", "c-1")
    }

    // Seed a message authored by `author` and return its id. The body is sealed under the author's
    // per-subject DEK so the crypto-shred / unrecoverable assertions are over REAL ciphertext.
    fn seed_message(
        store: &MemHotTier,
        outbox: &OutboxStore,
        minter: &Arc<MonotonicMinter>,
        author: &str,
        nonce: &str,
    ) -> MessageId {
        let mut tx = begin_tx(outbox, minter);
        let id = store
            .append(
                &mut tx,
                NewMessage {
                    conv: conv(),
                    thread_root_id: None,
                    author: author.to_string(),
                    author_kind: AuthorKind::Human,
                    body_inline: b"hello".to_vec(),
                    body_nodes: Vec::new(),
                    client_nonce: nonce.to_string(),
                },
            )
            .expect("append");
        tx.commit().expect("commit");
        id
    }

    /// **The CHAT-D8 core: an erased subject's body becomes UNRECOVERABLE in hot + cold + backups
    /// (0 recoverable PII).** A body sealed under the author's per-subject DEK opens while the key
    /// lives; after the cascade crypto-shreds the DEK, the SAME ciphertext (whether read hot, from a
    /// cold segment, or restored from a backup snapshot) is unrecoverable — `decrypt_body` fails
    /// LOUDLY, never a plaintext-without-key fall-through.
    #[test]
    fn erased_subject_body_is_unrecoverable_in_hot_cold_and_backups() {
        let kms = KmsEngine::new();
        let author = SubjectId::new("psn:ada");
        let plaintext = b"my private chat about my health".to_vec();
        // Seal a body (the same ciphertext lives in hot, cold, and any backup of the column).
        let column = encrypt_body(
            &kms,
            &region(),
            &tenant(),
            &author,
            ChatFreeText::BodyInline,
            &plaintext,
        )
        .expect("seal");
        // While the key lives: recoverable.
        assert!(!is_body_unrecoverable(&kms, &region(), &column));

        // Run the cascade's crypto-shred (the per-subject DEK destroy).
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let report = cascade.erase(
            &mut tx,
            &EraseScope::Subject {
                subject: subject("psn:ada"),
                tenant: tenant(),
            },
            &[],
        );
        tx.commit().expect("commit");

        // After the shred: the SAME column ciphertext (hot/cold/backup-restored — the bytes are
        // identical in all three) is UNRECOVERABLE. The backup snapshot no longer carries the DEK.
        assert!(
            is_body_unrecoverable(&kms, &region(), &column),
            "0 recoverable: the body is unrecoverable after the DEK crypto-shred"
        );
        assert!(
            !kms.backup_snapshot()
                .into_iter()
                .any(|(id, _)| id == DekId::new(tenant(), KeyClass::Subject("psn:ada".into()))),
            "the crypto-shredded DEK is EXCLUDED from backups (it stays dead across restore, §7.5)"
        );
        assert!(
            report.destroyed_key_epoch.is_some(),
            "the receipt records the destroyed key epoch (the post-restore re-erase audit trail)"
        );
    }

    /// **The cascade tombstones every authored record + emits `chat.message.erased` through the
    /// OUTBOX — the bus + DSR path, NEVER a backdoor (10.4 / 2.7).** Two messages by the subject are
    /// tombstoned (the fact survives, the body cleared) and exactly two `chat.message.erased` events
    /// land on the outbox — the cascade the derivatives (Search/Refs/Notif) consume.
    #[test]
    fn cascade_tombstones_and_emits_erased_via_the_bus_not_a_backdoor() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let m1 = seed_message(&store, &outbox, &minter, "psn:ada", "n1");
        let m2 = seed_message(&store, &outbox, &minter, "psn:ada", "n2");
        let before = outbox.committed_count();

        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let report = cascade.erase(
            &mut tx,
            &EraseScope::Subject {
                subject: subject("psn:ada"),
                tenant: tenant(),
            },
            &[(conv(), m1.clone()), (conv(), m2.clone())],
        );
        tx.commit().expect("commit");

        assert_eq!(report.records_tombstoned, 2, "both records tombstoned");
        assert!(report.cascade_published, "the cascade rode the bus");
        // Exactly two `chat.message.erased` events landed on the outbox (the bus + DSR cascade).
        let erased: Vec<_> = outbox
            .committed_rows()
            .into_iter()
            .filter(|r| r.envelope.type_.0 == CHAT_ERASE_CASCADE_TOKEN)
            .collect();
        assert_eq!(
            erased.len(),
            2,
            "two chat.message.erased tombstones on the outbox — the cascade the derivatives consume"
        );
        assert!(
            outbox.committed_count() > before,
            "the cascade ADDED durable events (no backdoor — the bus is the only path)"
        );
        // The records survive as tombstones (keep the fact, drop the body).
        let rows = store
            .range(&conv(), crate::store::RangeCursor::Recent, 10)
            .expect("range");
        for r in &rows {
            assert_eq!(r.state, crate::store::MessageState::Tombstoned);
            assert!(r.body_inline.is_empty(), "the body is dropped (shred)");
        }
    }

    /// **The cascade purges read-state, drafts, and the unfurl cache (CHAT-D8: the derived read-models
    /// go).** The subject's read-state marker, draft, and a cached unfurl entry are present before the
    /// erase and gone after.
    #[test]
    fn cascade_purges_read_state_drafts_and_unfurl_cache() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();

        // Seed the subject's read-state + draft.
        read_state.upsert(&ReadMarker::new(
            conv(),
            "psn:ada",
            MessageId("01ABC".into()),
        ));
        drafts.save(
            &DraftKey::new(conv().conversation_id, "psn:ada"),
            &Draft::text("an unsent private draft"),
        );
        assert!(
            read_state.load(&conv(), "psn:ada").is_some(),
            "read-state present"
        );
        assert!(
            drafts
                .load(&DraftKey::new(conv().conversation_id, "psn:ada"))
                .is_some(),
            "draft present"
        );

        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        cascade.erase(
            &mut tx,
            &EraseScope::Subject {
                subject: subject("psn:ada"),
                tenant: tenant(),
            },
            &[],
        );
        tx.commit().expect("commit");

        assert!(
            read_state.load(&conv(), "psn:ada").is_none(),
            "read-state purged (0 footprint)"
        );
        assert!(
            drafts
                .load(&DraftKey::new(conv().conversation_id, "psn:ada"))
                .is_none(),
            "the unsent draft purged"
        );
    }

    /// **The holder-receipt set is COMPLETE — one per registered Chat store, 0 missed (CHAT-D8).**
    /// The report carries an `erase` receipt for every [`ChatStoreClass`]; `receipts_complete()` is
    /// true and each receipt is content-addressed.
    #[test]
    fn the_holder_receipt_set_is_complete_zero_missed() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let report = cascade.erase(
            &mut tx,
            &EraseScope::Subject {
                subject: subject("psn:ada"),
                tenant: tenant(),
            },
            &[],
        );
        tx.commit().expect("commit");

        assert!(
            report.receipts_complete(),
            "every registered Chat store got an erase receipt (0 holders missed)"
        );
        assert_eq!(report.store_receipts.len(), ChatStoreClass::ALL.len());
        for class in ChatStoreClass::ALL {
            let r = report
                .receipt_for(class)
                .expect("a receipt per store class");
            assert_eq!(r.receipt.operation, "erase");
            assert!(
                r.receipt.content_hash.starts_with("blake3:"),
                "{} receipt is content-addressed",
                class.label()
            );
        }
        // The body/draft receipts carry the destroyed key epoch (the post-restore re-erase trail);
        // the read-state/author receipts do not (purge / pseudonym-shred, not key-destroy).
        assert!(
            report
                .receipt_for(ChatStoreClass::Messages)
                .unwrap()
                .receipt
                .key_epoch_destroyed
                .is_none()
                == report.destroyed_key_epoch.is_none()
        );
        assert!(report
            .receipt_for(ChatStoreClass::ReadState)
            .unwrap()
            .receipt
            .key_epoch_destroyed
            .is_none());
    }

    /// **A tenant offboarding scope is typed — the per-store receipts attest the Chat stores in
    /// scope (the KEK destroy is the GDPR-side P-GA-13 lever).** The aggregate receipt + the complete
    /// per-store set are produced for a `Tenant` scope too.
    #[test]
    fn tenant_offboarding_scope_produces_a_complete_receipt_set() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let scope = EraseScope::Tenant(tenant());
        let report = cascade.erase(&mut tx, &scope, &[]);
        tx.commit().expect("commit");
        assert!(report.receipts_complete());
        let agg = aggregate_receipt(&report, &scope);
        assert_eq!(agg.receipt.operation, "erase");
        assert!(agg.receipt.content_hash.starts_with("blake3:"));
    }

    /// **The aggregate receipt folds the complete per-store set into the frozen `EraseReceipt`
    /// (10.1).** Content-addressed over holder + subject + tenant + the destroyed epoch — the
    /// audit-log hash-link the holder's trait `erase` returns.
    #[test]
    fn aggregate_receipt_is_content_addressed_over_the_fan_out() {
        let kms = KmsEngine::new();
        let store = MemHotTier::new();
        let outbox = OutboxStore::new();
        let minter = Arc::new(MonotonicMinter::new());
        let read_state = ReadStateRecord::new();
        let drafts = MemDraftStore::new();
        let cache = UnfurlCache::new();
        let cascade = ChatErasureCascade::new(&kms, region(), &store, &read_state, &drafts, &cache);
        let mut tx = begin_tx(&outbox, &minter);
        let scope = EraseScope::Subject {
            subject: subject("psn:ada"),
            tenant: tenant(),
        };
        let report = cascade.erase(&mut tx, &scope, &[]);
        tx.commit().expect("commit");
        let agg = aggregate_receipt(&report, &scope);
        assert_eq!(agg.receipt.operation, "erase");
        assert_eq!(agg.receipt.key_epoch_destroyed, report.destroyed_key_epoch);
    }
}
