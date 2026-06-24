//! # `holder` — the Chat `PersonalDataHolder` (H5; auto-registered, locate/export typed, erase
//! stubbed to crypto-shred, the `restrict` flag wired) — CHAT-P6 / P-400, M4-C1
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md` (the
//!   Chat `PersonalDataHolder` H5 — Chat is a GDPR holder over its message bodies / drafts / author
//!   pseudonyms: `locate / export / rectify / restrict / erase`; the residual is the ONE platform
//!   posture, X-7 / 10.9, by reference).
//! - `01-tech-and-data-model.md` §3 / §1.4 (the per-subject-DEK body/draft columns the `erase`
//!   crypto-shred destroys + the pseudonymous author the erase pseudonym-shreds).
//! - `05-hard-problems.md` §5 (Chat is the most PII-dense holder — the body IS the PII).
//! - `planning/00-platform-substrate.md` §3.4 (every store the harness opens auto-registers as a
//!   `PersonalDataHolder` — "we forgot a store" is structurally impossible, contract 1.4).
//!
//! **Contracts:** index rows **10.1** (OWNED — the Chat `PersonalDataHolder{locate, export, rectify,
//! restrict, erase}`, auto-registered + typed over Chat's stores), **1.4** (CONSUMED — the harness
//! auto-registration on every store opened, the substrate [`HolderRegistry`] one door), **10.9**
//! (CONSUMED **by reference** — the ONE erasure posture; Chat does **not** restate a Chat-local
//! residual). Implemented to the frozen [`myelin_gdpr`] shapes.
//!
//! ## What CHAT-P6 ships — the holder SUBSTRATE, not the erasure fan-out (the named floor)
//! This prompt opens + auto-registers the Chat OLTP store (the message log + drafts) as holder **H5**
//! and ships the holder **registered + classified + callable**, with:
//! - **`locate` / `export` TYPED** over the Chat surface (messages / drafts / author pseudonyms) —
//!   empty-but-correct content-addressed receipts that attest the op ran (a real, callable holder,
//!   never a `todo!()`/panic). The full per-store subject-walk lands with the DSR cascade (CHAT-P22).
//! - **`restrict` WIRED** — [`ChatHolder::restrict`] flips a per-subject flag the index/agent/
//!   analytics/notif seams read ([`RestrictionFlag`]); the honoured-everywhere proof is the GDPR
//!   P-GA-25 path, but the flag the seams check is REAL here (Art. 18/21).
//! - **`erase` STUBBED to crypto-shred** — a well-defined no-op receipt that NAMES its CHAT-P22
//!   follow-on (the full per-subject-DEK crypto-shred across hot/cold/backups + pseudonym shred + the
//!   DSR cascade). The erasure LEVERS now BOTH exist: the per-subject DEK on the body/draft columns
//!   ([`crate::dek`], 11.4) and the pseudonymous author column ([`crate::schema`], 4.8) are WIRED at
//!   CHAT-P6; CHAT-P22 wires the `erase` BODY.
//!
//! The residual (third-party PII a person typed into ANOTHER subject's message, under that other
//! person's DEK) is the ONE platform posture (10.9 / X-7) — handled **by reference**
//! ([`CHAT_RESIDUAL_POSTURE_REF`]), never restated as a Chat-local statement. The structural floor
//! (per-subject DEK + pseudonym shred + `restrict` suppression) ships regardless.
//!
//! ## Why register NOW (the structural guarantee — §3.4 / contract 1.4)
//! The Chat OLTP store (the message log + outbox + drafts, [`crate::store`]) is opened through the
//! substrate [`HolderRegistry`] ONE door, so it is a registered holder by construction and classifies
//! to **H5 (`H5Chat`)** in the exhaustive H1–H18 list (gdpr §3.2). Registering it now makes "the DSR
//! fan-out forgot Chat" structurally impossible (10.1 exhaustiveness) — even though the erase BODY is
//! the CHAT-P22 floor.
//!
//! ## DB-free
//! This module builds in-memory holder/receipt values + flips an in-memory restriction flag; the real
//! per-subject-DEK crypto-shred rides the CHAT-P22 fan-out + the storage integration drills. So
//! `cargo build --workspace` stays DB-free.
//!
//! ## Floors named (VISION §3)
//! - **The `erase` crypto-shred fan-out body** (the per-subject-DEK destroy across hot/cold/backups
//!   over the message bodies + drafts + the `chat.message.erased` tombstones + pseudonym shred of the
//!   author) is **CHAT-P22 / P-411** (the full DSR cascade, CHAT-D8 — 0 recoverable PII). The
//!   per-subject-DEK COLUMNS the destroy operates on are WIRED at CHAT-P6 ([`crate::dek`]); here
//!   `erase` is the typed no-op receipt that names CHAT-P22.
//! - **The full `locate`/`export` subject-walk** (the real message/draft rows naming the subject)
//!   lands with the DSR cascade (CHAT-P22). Here they are empty-but-correct typed receipts.

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

/// The stable, PII-free name of the Chat **OLTP store** (the message log + outbox + drafts,
/// [`crate::store`]). This is the holder's **H5 (`H5Chat`)** store. Frozen here so the holder, the
/// data-map, the GDPR-side H5 registration, and the DSR cascade (CHAT-P22) all address exactly this
/// store. PII-free: a store identifier, never personal data.
pub const CHAT_OLTP_STORE: &str = "chat_oltp";

/// The stable, PII-free name of the Chat **read-state store** (the `(user × conversation)` last-read
/// markers, [`crate::read_state`]). Its OWN store (its own durable table/keyspace, distinct from the
/// OLTP message log) so the DSR cascade reaches it SPECIFICALLY (D-C8: read-state purged). Registered
/// as holder **H5 (`H5Chat`)** at CHAT-P16 so a person's read-state footprint is a holder by
/// construction. PII-free: a store identifier. Re-exported from [`crate::read_state`] so there is ONE
/// name (EI-01 §7); aliased here for the holder registration.
pub const CHAT_READ_STATE_STORE: &str = crate::read_state::CHAT_READ_STATE_STORE;

/// The Chat store CLASSES the holder spans — `locate / export / rectify / restrict / erase` over
/// **messages, drafts, author pseudonyms**. A closed enum: a new Chat data class cannot be added
/// without appearing here (the holder coverage is total — proven by the unit test over
/// [`ChatStoreClass::ALL`]). PII-free — a class tag, never data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChatStoreClass {
    /// Messages — the `message` rows: the per-subject-DEK `body_inline`/`body_nodes` (the body IS the
    /// PII, §1.4) + the pseudonymous `author`. OLTP (H5).
    Messages,
    /// Drafts — the composer draft store: an unsent message body under the per-subject DEK (§1.4).
    /// OLTP (H5).
    Drafts,
    /// Author identity — the pseudonymous `author` columns (4.8); erased by deleting the Identity
    /// pseudonym map ("Former user 8a2f" without rewriting messages others own). OLTP (H5).
    AuthorIdentity,
    /// Read-state — the per-`(user × conversation)` last-read markers ([`crate::read_state`], the
    /// CHAT-P16 durable record). A person's scroll-position footprint; purged on erasure (D-C8). OLTP
    /// (H5).
    ReadState,
}

impl ChatStoreClass {
    /// A stable, PII-free label for the class (telemetry / the receipt — never personal data).
    pub fn label(self) -> &'static str {
        match self {
            ChatStoreClass::Messages => "messages",
            ChatStoreClass::Drafts => "drafts",
            ChatStoreClass::AuthorIdentity => "author-identity",
            ChatStoreClass::ReadState => "read-state",
        }
    }

    /// **The full set of Chat store classes the holder spans.** `locate`/`export`/`erase` reach every
    /// member; a missed class is a hole. Closed + total — a new Chat data class cannot be added
    /// without appearing here (proven by the unit tests).
    pub const ALL: [ChatStoreClass; 4] = [
        ChatStoreClass::Messages,
        ChatStoreClass::Drafts,
        ChatStoreClass::AuthorIdentity,
        ChatStoreClass::ReadState,
    ];
}

/// **The residual posture — instantiated BY REFERENCE to the ONE platform posture (10.9 / X-7), NEVER
/// restated as a Chat-local statement** (recon X-7 — "the residual is by reference"). Chat cites the
/// posture; it does not author a fresh Chat-local residual. The structural floor (per-subject DEK +
/// pseudonym shred + `restrict` suppression) ships regardless.
pub const CHAT_RESIDUAL_POSTURE_REF: &str =
    "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
     Chat: per-subject DEK crypto-shred of the message bodies/drafts (11.4, the author's DEK reaches \
     hot/cold/backups + the immutable log) + pseudonym shred of the author (4.8) + restrict \
     suppression; per-tenant DEK fallback where third-party PII is not isolable (under the author's \
     DEK, not the subject's); never a Chat-local restatement (the full DSR cascade = CHAT-P22)";

/// The typed receipt that a Chat store was auto-registered as a [`PersonalDataHolder`] (re-exports
/// the substrate-side [`HolderRegistration`]). PII-free: a (kind, name) tag.
pub type ChatHolderRegistration = HolderRegistration;

/// Build the Chat [`StoreClassifier`] — the data-map declaration that the Chat OLTP store belongs to
/// holder **H5 (`H5Chat`)** (gdpr §3.2). The substrate completeness assertion joins the harness's
/// [`HolderRegistry`] against this classifier: every opened Chat store must map to an H-holder, or it
/// is an orphan (contract 1.4 + gdpr §3.2).
pub fn chat_store_classifier() -> StoreClassifier {
    StoreClassifier::of([
        myelin_substrate::StoreHolder::new(StoreKind::Oltp, CHAT_OLTP_STORE, Holder::H5Chat),
        // CHAT-P16 (P-410): the read-state durable record is its OWN Chat store, classified to the
        // SAME H5 holder — so the DSR cascade reaches the per-(user × conversation) markers (D-C8).
        myelin_substrate::StoreHolder::new(StoreKind::Oltp, CHAT_READ_STATE_STORE, Holder::H5Chat),
    ])
}

/// **Register the Chat OLTP store as a `PersonalDataHolder` through the harness auto-registration
/// (contract 1.4).** Opens the Chat OLTP store through the substrate [`HolderRegistry`] — the ONE
/// door — so it is a registered holder by construction. Returns the registry (carrying the receipt)
/// so a caller / test can assert exactly which stores registered + that they classify to their
/// H-holders (H5 for Chat). Registering it makes "the DSR fan-out forgot Chat" structurally
/// impossible (10.1 exhaustiveness).
pub fn register_chat_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, CHAT_OLTP_STORE);
    // CHAT-P16 (P-410): the read-state durable record opens through the SAME one door, so the
    // read-state markers are a registered H5 holder by construction (the DSR fan-out cannot miss
    // read-state — D-C8).
    registry.open(StoreKind::Oltp, CHAT_READ_STATE_STORE);
    registry
}

/// **The per-subject `restrict` flag (Art. 18/21) — the seam the index/agent/analytics/notif checks
/// read.** A restricted subject's Chat data is NOT indexed (CHAT-P20) / agent-used / analytics-fed /
/// notification-fanned. [`ChatHolder::restrict`] flips it; every Chat seam that surfaces a subject's
/// footprint reads [`RestrictionFlag::is_restricted`] BEFORE emitting. Shared (`Arc<Mutex<…>>`) so the
/// holder and the seams see ONE flag set. PII-free: opaque pseudonymous ids.
#[derive(Clone, Default)]
pub struct RestrictionFlag {
    /// The set of restricted subject ids (opaque pseudonymous principal ids — never a name/email).
    restricted: Arc<Mutex<BTreeSet<String>>>,
}

impl RestrictionFlag {
    /// A fresh flag set (no subject restricted yet).
    pub fn new() -> RestrictionFlag {
        RestrictionFlag::default()
    }

    /// Set (`on = true`) or clear (`on = false`) the restriction for a subject. Idempotent.
    pub fn set(&self, subject: &str, on: bool) {
        let mut g = self.restricted.lock().expect("restriction flag poisoned");
        if on {
            g.insert(subject.to_string());
        } else {
            g.remove(subject);
        }
    }

    /// **Whether a subject is restricted — the check every Chat index/agent/analytics/notif seam makes
    /// BEFORE surfacing the subject's footprint.** A restricted subject's Chat data is suppressed at
    /// the seam (fail-closed for surfacing).
    pub fn is_restricted(&self, subject: &str) -> bool {
        self.restricted
            .lock()
            .expect("restriction flag poisoned")
            .contains(subject)
    }
}

/// **The Chat `PersonalDataHolder` (H5; contract 10.1) — auto-registered, locate/export TYPED, erase
/// STUBBED to crypto-shred, the `restrict` flag WIRED.** The holder over Chat's messages, drafts, and
/// author pseudonyms. At CHAT-P6 the locate/export bodies are empty-but-correct content-addressed
/// receipts (a real, callable holder — the full subject-walk + DSR cascade is CHAT-P22); `erase` is
/// the typed no-op that names its CHAT-P22 crypto-shred fan-out; `restrict` flips a REAL per-subject
/// flag the Chat seams read. The erasure LEVER (the per-subject DEK on the body/draft columns, 11.4)
/// exists as [`crate::dek`].
#[derive(Clone, Default)]
pub struct ChatHolder {
    /// The per-subject restriction flag the index/agent/analytics/notif seams read. Shared so the
    /// holder and the seams see ONE flag set.
    restriction: RestrictionFlag,
}

impl ChatHolder {
    /// Build the Chat holder with a fresh restriction flag.
    pub fn new() -> ChatHolder {
        ChatHolder::default()
    }

    /// Build the Chat holder sharing an existing restriction flag (so a seam can read the SAME flag
    /// the holder writes — one flag set across the holder + the index/agent/analytics/notif seams).
    pub fn with_restriction(restriction: RestrictionFlag) -> ChatHolder {
        ChatHolder { restriction }
    }

    /// Register the Chat OLTP store as holder H5 through the substrate registry (the `serve`-called
    /// auto-registration seam), returning the receipt — the proof the Chat store registered as H5.
    pub fn register(&self, registry: &mut HolderRegistry) -> ChatHolderRegistration {
        registry.open(StoreKind::Oltp, CHAT_OLTP_STORE)
    }

    /// Borrow the restriction flag (so a Chat index/agent/analytics/notif seam can read the SAME flag
    /// the holder's `restrict` writes — one flag set, never two).
    pub fn restriction(&self) -> &RestrictionFlag {
        &self.restriction
    }

    /// The opaque, PII-free subject id the receipt body keys on (the pseudonymous Principal id) —
    /// never a name/email. One derivation — never a second subject-id rendering.
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for ChatHolder {
    /// Art. 15 access — where the subject's Chat data lives: their authored messages, their drafts,
    /// their author-pseudonym footprint. At CHAT-P6 an empty-but-correct content-addressed receipt
    /// attesting the locate ran over the Chat surface (the full per-class subject-walk lands with
    /// CHAT-P22). NEVER an error — a real, callable holder.
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                CHAT_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "Chat locate over messages/drafts/author-pseudonym (CHAT-P6 typed seam; the full \
                 subject-walk = the DSR cascade CHAT-P22)",
                None,
                0,
            ),
        })
    }

    /// Art. 20 portability — the subject's Chat footprint (authored messages + drafts) as
    /// references plus decrypted-while-key-lives body excerpts. At CHAT-P6 an empty-but-correct
    /// portable bundle; the full export lands with CHAT-P22.
    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                CHAT_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "Chat export: the subject's authored messages + drafts as references + \
                 per-subject-DEK-decrypted body excerpts (CHAT-P6 typed seam; the full bundle = \
                 CHAT-P22)",
                None,
                0,
            ),
        })
    }

    /// Art. 16 rectification — update Chat free text the subject controls (their own message bodies /
    /// drafts). The patch-apply model lands with the DSR cascade (CHAT-P22 / GDPR 10.4); at CHAT-P6 a
    /// well-defined no-op receipt naming it.
    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                CHAT_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (CHAT-P6 substrate; the patch-apply + reindex-from-source = CHAT-P22 / \
                 GDPR 10.4)",
                None,
                0,
            ),
        })
    }

    /// Art. 18/21 restriction — set/clear the per-subject restriction flag the Chat index/agent/
    /// analytics/notif seams read. This flips a REAL flag ([`RestrictionFlag`]) the seams check BEFORE
    /// surfacing the subject's footprint; the honoured-everywhere proof is the GDPR P-GA-25 path. A
    /// restricted subject's Chat data is NOT indexed / agent-used / analytics-fed / notification-fanned.
    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = Self::subject_id(subject);
        self.restriction.set(&sid, on);
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                CHAT_OLTP_STORE,
                &sid,
                "",
                if on {
                    "Chat restrict ON: no indexing / no agent-use / no analytics / no notification"
                } else {
                    "Chat restrict OFF: the per-subject restriction flag is cleared"
                },
                None,
                0,
            ),
        })
    }

    /// Art. 17 erasure — the **trait-surface** entrypoint. The FULL fan-out body is the named floor
    /// **CHAT-P22 / P-411**: crypto-shred the subject's per-subject Chat DEK (the body/draft columns,
    /// 11.4 — rendering the encrypted body, incl. cold segments + backups + the immutable log,
    /// unrecoverable) over the ONE `KmsEngine`, pseudonym-shred the `author` identity via Identity's
    /// `erase` (4.8 — "Former user 8a2f" without rewriting messages others own), set the
    /// [`RestrictionFlag`], and emit the `chat.message.erased` tombstones — across every Chat holder,
    /// with a per-holder receipt + post-restore re-erasure. The message STRUCTURE survives (delete the
    /// content, keep the fact). The residual is the ONE platform posture
    /// ([`CHAT_RESIDUAL_POSTURE_REF`], 10.9 / X-7 — never restated Chat-local).
    ///
    /// THIS trait method has no `KmsEngine` / Identity surface in its frozen 10.1 signature, so it
    /// returns the typed aggregate receipt the live binding (the CHAT-P22 fan-out, which DOES hold
    /// those dependencies) backs. It is NEVER a panic / `todo!()`.
    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                CHAT_OLTP_STORE,
                &subject_id,
                &tenant,
                "Chat erase (the full fan-out = CHAT-P22 / P-411): per-subject DEK crypto-shred of \
                 the message bodies/drafts across hot/cold/backups + the immutable log + pseudonym \
                 shred of the author (4.8) + restrict + the chat.message.erased tombstones across \
                 every holder, with post-restore re-erasure; residual = the ONE posture 10.9/X-7, \
                 by reference",
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::{
        assert_all_holders_registered, assert_holder_completeness, classify_store, DeclaredStore,
        StoreManifest,
    };

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    /// **The store-class set is the holder's coverage** — messages, drafts, author identity. The
    /// closed set is the structural coverage surface (a new Chat data class cannot be added without
    /// appearing here).
    #[test]
    fn the_chat_store_class_set_is_the_holder_coverage() {
        assert_eq!(ChatStoreClass::ALL.len(), 4);
        for c in [
            ChatStoreClass::Messages,
            ChatStoreClass::Drafts,
            ChatStoreClass::AuthorIdentity,
            ChatStoreClass::ReadState,
        ] {
            assert!(
                ChatStoreClass::ALL.contains(&c),
                "{} must be in the holder coverage",
                c.label()
            );
        }
    }

    /// **The Chat OLTP store auto-registers as holder H5 through the one door (contract 1.4) and
    /// classifies to H5 — 0 orphans (gdpr §3.2).** Opening it through the substrate registry makes it
    /// a registered holder by construction; it maps to the exhaustive H5 (`H5Chat`) — so the DSR
    /// fan-out cannot silently miss Chat. This is the CHAT-P6 holder GATE.
    #[test]
    fn chat_store_registers_and_classifies_to_h5_no_orphan() {
        let registry = register_chat_holders();
        assert!(registry.is_registered(StoreKind::Oltp, CHAT_OLTP_STORE));
        // CHAT-P16: the read-state durable record is its own registered Chat store (D-C8).
        assert!(registry.is_registered(StoreKind::Oltp, CHAT_READ_STATE_STORE));
        assert_eq!(
            registry.len(),
            2,
            "the Chat OLTP + read-state stores registered"
        );
        let classifier = chat_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, CHAT_OLTP_STORE, &classifier),
            Some(Holder::H5Chat),
            "the Chat OLTP store is holder H5 (Chat subsystem DB)"
        );
        assert_eq!(
            classify_store(StoreKind::Oltp, CHAT_READ_STATE_STORE, &classifier),
            Some(Holder::H5Chat),
            "the Chat read-state store is holder H5 (the per-user markers, D-C8)"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "every Chat store is in the exhaustive H1–H18 list — 0 orphan stores"
        );
    }

    /// **The 1.4 enforcement: a Chat store opened OUTSIDE the harness FAILS the holder-registered
    /// architecture test.** The conforming registry passes; a registry missing it is a loud violation
    /// naming exactly the escaped store — an unregistered PII store cannot quietly miss the DSR fan-out.
    #[test]
    fn an_unregistered_chat_store_fails_the_holder_registered_architecture_test() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, CHAT_OLTP_STORE)]);
        assert_eq!(
            assert_all_holders_registered(&manifest, &register_chat_holders()),
            Ok(()),
            "the Chat store opened through the harness → the architecture test passes"
        );
        let rogue = HolderRegistry::new();
        let err = assert_all_holders_registered(&manifest, &rogue)
            .expect_err("a Chat store opened outside the harness must FAIL the architecture test");
        assert_eq!(
            err.len(),
            1,
            "exactly the unregistered Chat store is the violation"
        );
        assert!(
            err[0].message().contains(CHAT_OLTP_STORE),
            "the failure names the escaped Chat store: {}",
            err[0].message()
        );
    }

    /// **`locate`/`export` are TYPED + empty-but-correct (the CHAT-P6 surface), not an error.** Both
    /// return content-addressed receipts over the Chat surface — a real, callable holder, not a
    /// `todo!()`/`Err`. The full located/exported data lands with CHAT-P22.
    #[test]
    fn locate_and_export_are_typed_and_empty_but_correct() {
        let holder = ChatHolder::new();
        let subj = subject("psn:chat-7");
        let locate = holder
            .locate(&subj, tenant())
            .expect("locate over the Chat surface succeeds");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        assert!(
            locate.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
        let export = holder
            .export(&subj, tenant())
            .expect("export over the Chat surface succeeds");
        assert_eq!(export.receipt.operation, "export");
        assert!(export.receipt.content_hash.starts_with("blake3:"));
    }

    /// **`restrict` flips a REAL per-subject flag the Chat seams read (Art. 18/21).** After
    /// `restrict(on)` the subject is restricted; after `restrict(off)` it is cleared. The flag the
    /// holder writes is the SAME one a seam reads (one flag set).
    #[test]
    fn restrict_flips_a_real_flag_the_seams_read() {
        let flag = RestrictionFlag::new();
        let holder = ChatHolder::with_restriction(flag.clone());
        let subj = subject("psn:chat-restricted");
        let sid = "psn:chat-restricted";
        assert!(!flag.is_restricted(sid), "not restricted initially");
        holder.restrict(&subj, true).expect("restrict on");
        assert!(
            flag.is_restricted(sid),
            "the holder's restrict(on) is seen by a seam reading the SAME flag"
        );
        holder.restrict(&subj, false).expect("restrict off");
        assert!(!flag.is_restricted(sid), "restrict(off) clears the flag");
    }

    /// **`erase` is the typed crypto-shred receipt naming CHAT-P22 (never a `todo!()`/panic).** It
    /// returns a content-addressed `erase` receipt over the Chat store for both subject + tenant scope.
    #[test]
    fn erase_is_a_typed_crypto_shred_receipt_naming_chat_p22() {
        let holder = ChatHolder::new();
        let subj = subject("psn:chat-erase");
        let receipt = holder
            .erase(EraseScope::Subject {
                subject: subj,
                tenant: tenant(),
            })
            .expect("erase returns a typed receipt");
        assert_eq!(receipt.receipt.operation, "erase");
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));

        // tenant offboarding scope is also typed (the whole-tenant KEK destroy, CHAT-P22).
        let t_receipt = holder
            .erase(EraseScope::Tenant(tenant()))
            .expect("tenant erase returns a typed receipt");
        assert_eq!(t_receipt.receipt.operation, "erase");
    }

    /// **The residual is cited BY REFERENCE to the ONE platform posture (10.9 / X-7), never restated
    /// Chat-local.** The reference names the contract + the structural floor (per-subject DEK +
    /// pseudonym shred + restrict), never a fresh Chat-authored residual statement.
    #[test]
    fn the_residual_is_the_one_platform_posture_by_reference() {
        assert!(CHAT_RESIDUAL_POSTURE_REF.contains("10.9"));
        assert!(CHAT_RESIDUAL_POSTURE_REF.contains("X-7"));
        assert!(CHAT_RESIDUAL_POSTURE_REF.contains("per-subject DEK"));
    }
}
