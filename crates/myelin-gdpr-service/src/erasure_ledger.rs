//! # The erasure ledger (10.8) — PII-free, non-shred-erasable + post-restore re-erasure
//! (P-GA-15 → P-115)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§3.2** (H10 backups —
//! crypto-shred by construction: key destroyed ⇒ ciphertext unrecoverable, + bounded retention +
//! **post-restore re-erasure**), **§4** (the canonical erase order records the **destroyed key
//! epoch** — §4.2), and the **§1.2 ownership split** (Storage owns the restore MECHANISM
//! `post_restore_reerase`, contract 11.5; GDPR owns the **LEDGER** that drives it, contract 10.8).
//! The §2.3 register names the erasure ledger as *"a recursive holder with the audit carve-out"* —
//! a holder that does **NOT** crypto-shred away (it must survive to drive re-erasure).
//!
//! **Contract-index:** row **10.8** (OWNED here — the erasure ledger: PII-free opaque-subject +
//! holders/keys shredded record; drives post-restore re-erasure GD-14). Consumed: row **11.5**
//! (Storage's `post_restore_reerase` reads the records this ledger exposes — the GDPR/Storage seam),
//! 11.3/11.4 (the destroyed key epochs the ledger records), 10.4 (the DSR completion that writes the
//! ledger entry).
//!
//! ## The load-bearing idea (§3.2 / §4.4 — why a restore can RESURRECT an erased person)
//! A backup is a point-in-time T. A subject erased (crypto-shredded) at a cross-seam offset `e > T`
//! — AFTER the backup was taken — still has its *pre-erasure* per-subject DEK alive in the backup
//! (the key was live at T). Restoring T would bring that DEK back to life and **un-erase the
//! person** — the gravest data-handling failure (a restore that resurrects an erased subject IS a
//! data-loss-class failure, EI-01 §2). The fix (ADR-18 / GD-14): every restore reads the **erasure
//! ledger** and **re-erases** every subject the ledger records as erased AFTER the restore's PIT, so
//! a restore never resurrects erased PII. Storage owns the restore MECHANISM (`post_restore_reerase`,
//! [`crate::erasure_ledger`] is read by `myelin-storage::reerase::PostRestoreErasureLedger`); GDPR
//! owns the **ledger** — *this module*.
//!
//! ## Why the ledger is itself a NON-shred-erasable recursive holder (the §2.3 carve-out)
//! The ledger records every completed erasure. If it were itself crypto-shred-erasable, an erasure of
//! the subject would destroy the very record that drives the subject's re-erasure on a restore — and
//! a restore WOULD then resurrect them. So the ledger is a **recursive holder** (it implements
//! [`PersonalDataHolder`]) whose `erase` **RETAINS** the PII-free record (exactly like the audit
//! carve-out H16, gdpr §6.4) — it holds **no PII** (only opaque tokens + offsets + epochs), so
//! retaining it leaks nothing, and it MUST survive to drive re-erasure. The architecture test
//! [`tests::the_ledger_schema_is_pii_free`] asserts no PII-typed field appears in the schema.
//!
//! ## What THIS prompt (P-GA-15) ships — and what it REUSES (coherence, EI-01 §7)
//! Storage already shipped (P-100) the re-erasure **MECHANISM**: the
//! `myelin-storage::reerase::PostRestoreErasureLedger` **seam** (a trait keyed by completion offset),
//! its in-memory `InMemoryPostPitLedger` model, the `ReErasePass` (re-apply each post-PIT erasure +
//! assert 0 resurrected), and the `RestoreVerifyGate::run_with_reerase` wiring. Storage's module
//! NAMED this prompt as the one that ships *"the real GDPR-owned erasure ledger (10.8) … co-built in
//! this band [that] wires the real binding"*. Per the coherence rule, this prompt does NOT re-define
//! the re-erasure mechanism — it ships the **authoritative GDPR-owned ledger** (the durable, PII-free
//! source of truth written on DSR completion) and the **neutral [`PostPitRecord`] projection** the
//! storage seam is populated FROM. The seam stays in Storage (it cannot import this crate — an upward
//! DAG edge); this crate cannot import Storage (the no-cross-store-read law, gdpr §3.1). The wiring —
//! GDPR ledger → storage seam — lands at the boot/cell-orchestration layer (`myelin-control-plane`,
//! which depends on BOTH), where the **CDC pair** ([`tests`] in
//! `crates/myelin-control-plane/tests/cdc_10_8_erasure_ledger_drives_reerase.rs`) proves the
//! provider (this ledger writes entries) ⇄ consumer (storage's `post_restore_reerase` reads them) seam.
//!
//! The [`PostPitRecord`] field shape — `{subject, tenant, completed_at_offset}` — mirrors Storage's
//! `ErasureRecord` exactly, so the boot wiring is a 1:1 field copy, not a translation that can drift.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The drills run at M1 scale here; they re-run at CELL scale at M5** (P-GA-32 → P-505, under
//!   world-scale load) and the **full H1–H18 fan-out (GA-D1)** is M5. NAMED per the DoD.
//! - **The durable Postgres `erasure_ledger` table** (the §2.3 register, NON-shred-erasable by
//!   construction — it is excluded from the per-tenant crypto-shred) is the same DB floor every M0/M1
//!   in-memory store carries (P-007 / P-S12). On this floor the ledger is an in-memory
//!   [`ErasureLedger`] with byte-for-byte the §4.4 / §3.2 semantics; the live `pg_restore` +
//!   WAL-replay driver that DRIVES the re-erasure pass is the P-S12/P-S15 storage floor (Storage's
//!   `reerase` module names it). The ledger's read shape ([`ErasureLedger::post_pit_records_after`])
//!   does not change when the real table lands.
//! - **The Merkle SEAL of the completion receipt + the audit-log hash-link** → **P-GA-20 → P-119**
//!   (the ledger records the PII-free completion fact; the Merkle inclusion that makes the certificate
//!   provable is P-GA-20).
//!
//! ## Mutation floor (mandatory-core — EI-01 §2; the prompt TESTS field). The **ledger-write** (the
//! idempotent `record_completion` keyed on the DSR id) and the **re-erasure-trigger** path
//! (`post_pit_records_after` — the `completed_at_offset > pit` selection that drives storage's
//! re-erasure) are mandatory-core. The achieved score is stated in the P-115 report
//! (`cargo mutants -p myelin-gdpr-service --file src/erasure_ledger.rs`).

use std::collections::BTreeMap;

use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

use crate::dsr::DsrId;

/// The PII-free holder id of the erasure ledger itself (the recursive holder — §2.3). A
/// `<kind>:<name>` token (contract 1.4); never a subject.
pub const ERASURE_LEDGER_STORE: &str = "gdpr_erasure_ledger:erasure_ledger";

/// The `erasure_ledger_entries` telemetry signal NAME + UNIT — the count of completed-erasure
/// records held (the `crypto-shred-lag`'s companion: how many erasures the ledger drives re-erasure
/// for). PII-free: a count, never a subject.
pub const ERASURE_LEDGER_ENTRIES: (&str, &str) = ("gdpr.erasure_ledger_entries", "count");

// ───────────────────────────── the per-holder destroyed key epoch ─────────────────────────────

/// One holder's destroyed-key-epoch record within an [`ErasureLedgerEntry`] — the §4.2 audit trail
/// that makes "we erased it" independently checkable against the KMS key-destruction log. PII-free:
/// the holder id (`<kind>:<name>`) + the destroyed epoch ordinal, never a subject.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DestroyedKeyEpoch {
    /// The holder whose key epoch was destroyed (the checklist key — a `<kind>:<name>` token).
    pub holder_id: String,
    /// The key epoch the crypto-shred destroyed (`None` for a carve-out holder that retained a
    /// minimised record and destroyed no key on a per-subject erase — gdpr §6.4). The presence/value
    /// is checkable against the KMS key-destruction log (§4.2).
    pub key_epoch_destroyed: Option<u64>,
}

// ───────────────────────────── the erasure-ledger entry (PII-free, 10.8) ─────────────────────────────

/// **One completed-erasure record in the erasure ledger (10.8) — PII-FREE.**
///
/// It records, for one DSR completion: the **opaque subject token** (the pseudonymous
/// `principal_id` — *never* a name/email), the tenant token, the **holders erased**, the **key
/// epochs destroyed** (the §4.2 trail), and the **completion offset** (the cross-seam cursor §4.4 —
/// the same `WalOffset` a restore lands at, so "completed AFTER the backup's PIT" is an exact,
/// assertable comparison; it drives Storage's `post_restore_reerase`). It holds **NO PII** by
/// construction (the architecture test asserts the schema has no PII-typed field) — which is exactly
/// why the ledger that holds these is *not itself crypto-shred-erasable* (retaining a PII-free record
/// leaks nothing, and the record must survive to drive re-erasure).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureLedgerEntry {
    /// The DSR this completion is for (the idempotency key — a re-completion of the same DSR does NOT
    /// duplicate the entry). PII-free (`dsr:<n>`, a per-register ordinal).
    pub dsr_id: DsrId,
    /// The opaque subject token erased — the pseudonymous `principal_id`, **never** a name/email. For
    /// a tenant offboarding this is the sentinel `"*"` (the whole tenant; no single subject).
    pub subject_token: String,
    /// The tenant the erasure ran within (the partition key; an opaque token).
    pub tenant_token: String,
    /// The holders erased in this completion (the fan-out set; `<kind>:<name>` tokens, sorted).
    pub holders_erased: Vec<String>,
    /// The per-holder destroyed key epochs (the §4.2 independent-check trail), sorted by holder id.
    pub key_epochs_destroyed: Vec<DestroyedKeyEpoch>,
    /// **The cross-seam completion offset (§4.4 — the §7.3 cursor).** An erasure with
    /// `completed_at_offset > T` (a restore's PIT) is one a restore of T would RESURRECT — the set
    /// Storage's `post_restore_reerase` re-applies. PII-free.
    pub completed_at_offset: u64,
    /// The wall-clock second the DSR completed (the completion timestamp). PII-free.
    pub completed_at_secs: u64,
}

impl ErasureLedgerEntry {
    /// `true` iff this entry erased `holder_id` (a checklist membership read the re-erasure +
    /// coverage assertions use).
    pub fn erased_holder(&self, holder_id: &str) -> bool {
        self.holders_erased.iter().any(|h| h == holder_id)
    }
}

/// **The neutral post-PIT projection the Storage `post_restore_reerase` seam is populated FROM.**
///
/// Its fields — `{subject, tenant, completed_at_offset}` — mirror `myelin-storage`'s `ErasureRecord`
/// EXACTLY, so the boot/cell-orchestration wiring (`myelin-control-plane`) populates the storage
/// `PostRestoreErasureLedger` seam by a 1:1 field copy (no translation that can drift). This crate
/// cannot name Storage's type (the no-cross-store-read law forbids the import); the projection is the
/// PII-free, minimal record the seam needs. The CDC pair (in `myelin-control-plane`) pins that the
/// field shapes stay aligned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostPitRecord {
    /// The opaque subject token erased (the pseudonymous `principal_id`). For a tenant offboarding the
    /// sentinel `"*"`.
    pub subject: String,
    /// The tenant the erasure ran within (an opaque token).
    pub tenant: String,
    /// The cross-seam offset the erasure completed at (the §4.4 cursor — `> pit` ⇒ re-apply).
    pub completed_at_offset: u64,
}

// ───────────────────────────── the erasure ledger (10.8) ─────────────────────────────

/// **The erasure ledger (contract 10.8) — the GDPR-owned, PII-free, NON-shred-erasable record of
/// every completed erasure (§2.3 / §3.2 / §4.4).**
///
/// On a DSR completion the orchestrator writes one [`ErasureLedgerEntry`] here (the
/// [`ErasureLedger::record_completion`] write). The write is **idempotent** — re-completing the same
/// DSR (a worker restart re-driving the same `dsr_id`) does NOT duplicate the entry. The ledger then
/// drives Storage's `post_restore_reerase` (11.5): a restore reads
/// [`ErasureLedger::post_pit_records_after`] and re-erases every subject the ledger records as erased
/// AFTER the restore's PIT, so a restore never resurrects erased PII.
///
/// **It is a recursive [`PersonalDataHolder`]** whose `erase` RETAINS the PII-free record (it must
/// survive to drive re-erasure — §2.3, the carve-out with the audit log). On the M1 floor it is an
/// in-memory store (the durable Postgres `erasure_ledger` table, excluded from the per-tenant
/// crypto-shred, is a named floor); the read shape does not change when the table lands.
#[derive(Debug, Default)]
pub struct ErasureLedger {
    /// dsr id → the completion record (the idempotency key is the DSR id — one entry per DSR).
    entries: std::sync::Mutex<BTreeMap<DsrId, ErasureLedgerEntry>>,
}

impl ErasureLedger {
    /// A fresh, empty erasure ledger (no completed erasures recorded).
    pub fn new() -> ErasureLedger {
        ErasureLedger::default()
    }

    /// **Record a completed erasure (the §4.4 step-5 / 10.8 write) — IDEMPOTENT.**
    ///
    /// On a DSR completion the orchestrator calls this with the PII-free completion facts. The write
    /// is keyed on `dsr_id`: re-completing the SAME DSR (a worker restart re-driving the same id over
    /// the durable checklist) **does not duplicate** the entry — the ledger holds exactly one record
    /// per DSR. Returns `true` iff this was a NEW entry (a duplicate completion returns `false` and
    /// leaves the ledger unchanged), so the caller can distinguish a first completion from a resume.
    ///
    /// The `subject_token` MUST be the opaque pseudonymous `principal_id` (never a name/email) — the
    /// ledger holds no PII (the architecture test asserts the schema has no PII-typed field).
    #[allow(clippy::too_many_arguments)]
    pub fn record_completion(
        &self,
        dsr_id: DsrId,
        subject_token: String,
        tenant_token: String,
        mut holders_erased: Vec<String>,
        mut key_epochs_destroyed: Vec<DestroyedKeyEpoch>,
        completed_at_offset: u64,
        completed_at_secs: u64,
    ) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        // Idempotent: one record per DSR. A re-completion (worker restart) is a no-op (it does NOT
        // overwrite the original completion offset/time — re-erasure drives off the FIRST completion).
        if entries.contains_key(&dsr_id) {
            return false;
        }
        holders_erased.sort();
        holders_erased.dedup();
        key_epochs_destroyed.sort();
        key_epochs_destroyed.dedup();
        entries.insert(
            dsr_id.clone(),
            ErasureLedgerEntry {
                dsr_id,
                subject_token,
                tenant_token,
                holders_erased,
                key_epochs_destroyed,
                completed_at_offset,
                completed_at_secs,
            },
        );
        true
    }

    /// The completion entry for a DSR, if recorded (a read-only snapshot).
    pub fn entry(&self, dsr_id: &DsrId) -> Option<ErasureLedgerEntry> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(dsr_id).cloned()
    }

    /// The number of completed-erasure records held (the `erasure_ledger_entries` telemetry signal).
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// `true` iff the ledger holds no records.
    pub fn is_empty(&self) -> bool {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// **The re-erasure-trigger read (§4.4 / GD-14) — every erasure completed AFTER `pit`.**
    ///
    /// Returns the [`PostPitRecord`] for every entry whose `completed_at_offset > pit` (the restore's
    /// point-in-time T) — the set a restore of T would RESURRECT, which Storage's
    /// `post_restore_reerase` re-applies. Records completed at-or-before `pit` are NOT returned (a
    /// pre-T erasure is already dead in the backup by construction — crypto-shred reaches the backup,
    /// §3.2). The boot wiring (`myelin-control-plane`) feeds these into Storage's
    /// `PostRestoreErasureLedger::erasures_completed_after`. Sorted by `(offset, subject)` for a
    /// deterministic re-apply order.
    ///
    /// A whole-tenant offboarding entry (`subject_token == "*"`) is excluded from the per-SUBJECT
    /// re-erasure fan-out (its re-erasure is a whole-tenant KEK re-destruction the storage tenant-kill
    /// path drives, not a per-subject DEK re-shred) — it is recorded for audit but not projected into
    /// the per-subject post-PIT set.
    pub fn post_pit_records_after(&self, pit: u64) -> Vec<PostPitRecord> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<PostPitRecord> = entries
            .values()
            .filter(|e| e.completed_at_offset > pit)
            .filter(|e| e.subject_token != "*") // a tenant offboarding is not a per-subject re-erase
            .map(|e| PostPitRecord {
                subject: e.subject_token.clone(),
                tenant: e.tenant_token.clone(),
                completed_at_offset: e.completed_at_offset,
            })
            .collect();
        out.sort_by(|a, b| {
            a.completed_at_offset
                .cmp(&b.completed_at_offset)
                .then_with(|| a.subject.cmp(&b.subject))
        });
        out
    }
}

// ───────────────────────────── the recursive holder (§2.3 — NON-shred-erasable) ─────────────────────────────

/// **The erasure ledger is itself a recursive [`PersonalDataHolder`] (§2.3) — but NON-shred-erasable.**
///
/// `erase` is a **carve-out** exactly like the audit log (gdpr §6.4): the ledger record is
/// **RETAINED**, never shredded — it holds no PII (only opaque tokens + offsets + epochs) and MUST
/// survive to drive re-erasure (if it were shredded, a restore would resurrect the very subject it
/// records as erased). `locate`/`export` report the PII-free record exists; `restrict`/`rectify` are
/// no-ops/refusals (a PII-free record has no PII to restrict, and editing it would corrupt the
/// re-erasure source). This is what makes the ledger the ONE holder in the H1–H18 set that the
/// per-tenant crypto-shred does NOT erase away.
impl PersonalDataHolder for ErasureLedger {
    fn locate(&self, _subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        // The ledger holds only the PII-FREE completion fact (opaque tokens). There is no recoverable
        // PII to locate — the access answer is the canonical 0-recoverable verdict.
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                ERASURE_LEDGER_STORE,
                "*", // the ledger keys on the DSR, not the subject — no per-subject PII to locate.
                &tenant.0,
                "located:0-recoverable",
                None,
                0,
            ),
        })
    }

    fn export(&self, _subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                ERASURE_LEDGER_STORE,
                "*",
                &tenant.0,
                "exported:pii-free-no-portable-data",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, _subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // The erasure ledger is NEVER rectified — editing the re-erasure source would risk a restore
        // resurrecting an erased subject (the same tamper-evidence reason the audit log refuses it).
        Err(DsrError(
            "erasure ledger (10.8): the PII-free completion record is NEVER edited — it is the source \
             that drives post-restore re-erasure; a rectification would corrupt the re-erasure source \
             and risk a restore resurrecting an erased subject (gdpr §3.2 / ADR-18). It holds no PII to \
             rectify."
                .to_string(),
        ))
    }

    fn restrict(&self, _subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // A PII-free record has no PII to suppress; the restrict op is a no-op acknowledgement (it
        // never indexes/agent-reads the record, so there is nothing to restrict).
        let outcome = if on { "restricted:noop-pii-free" } else { "restricted:clear-noop" };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                ERASURE_LEDGER_STORE,
                "*",
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        // THE NON-SHRED-ERASABLE CARVE-OUT (§2.3): the ledger record is RETAINED, never shredded. It
        // holds no PII (opaque tokens + offsets + epochs only), so retaining it leaks nothing, and it
        // MUST survive to drive re-erasure on a restore. No key is destroyed (`None` epoch). This is
        // the property that makes the ledger the recursive holder that does NOT crypto-shred away.
        let (subject, tenant) = match scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.0.clone())
            }
            EraseScope::Tenant(tenant) => ("*".to_string(), tenant.0.clone()),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                ERASURE_LEDGER_STORE,
                &subject,
                &tenant,
                // The carve-out verdict the architecture test asserts — retained, NON-shred-erasable
                // (the record survives to drive re-erasure; a shred would resurrect on a restore).
                "carve_out:retained-pii-free-record:non-shred-erasable:drives-re-erasure",
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

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    fn subject_ref(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant()))
    }

    fn epoch(holder: &str, e: Option<u64>) -> DestroyedKeyEpoch {
        DestroyedKeyEpoch { holder_id: holder.into(), key_epoch_destroyed: e }
    }

    // ───────────── the ledger schema is PII-FREE (the architecture test) ─────────────

    /// **ARCHITECTURE TEST (the prompt's unit requirement): the erasure-ledger schema is PII-free.**
    /// No PII-typed field appears in [`ErasureLedgerEntry`] — every field is an opaque token / id /
    /// offset / epoch / count. The subject is an opaque token (the pseudonymous `principal_id`), NEVER
    /// a `SubjectRef`/`Principal`/name/email. This is what makes the ledger the ONE holder that must
    /// NOT be shred-erasable (retaining a PII-free record leaks nothing).
    ///
    /// We assert it structurally: an entry built from a subject carries the OPAQUE token string, and
    /// the struct exposes NO `SubjectRef`/`Principal`/`email`/`name` field (a compile-time property —
    /// the construction below is the exhaustive field list; if a PII-typed field were added this test
    /// would have to name it).
    #[test]
    fn the_ledger_schema_is_pii_free() {
        let entry = ErasureLedgerEntry {
            dsr_id: DsrId("dsr:0".into()),
            subject_token: "p-opaque-123".into(), // the pseudonymous principal_id — NOT a name/email.
            tenant_token: "acme".into(),
            holders_erased: vec!["oltp:identity_oltp".into()],
            key_epochs_destroyed: vec![epoch("oltp:identity_oltp", Some(7))],
            completed_at_offset: 140,
            completed_at_secs: 1_700_000_000,
        };
        // The subject is an opaque token — there is no SubjectRef/Principal/email/name field to leak.
        assert_eq!(entry.subject_token, "p-opaque-123");
        assert!(!entry.subject_token.contains('@'), "no email form in the subject token");
        // The exhaustive field list (this destructure FAILS TO COMPILE if a new field is added —
        // forcing a reviewer to confirm any new field is PII-free).
        let ErasureLedgerEntry {
            dsr_id: _,
            subject_token: _,
            tenant_token: _,
            holders_erased: _,
            key_epochs_destroyed: _,
            completed_at_offset: _,
            completed_at_secs: _,
        } = entry;
    }

    // ───────────── a DSR completion writes a ledger entry (the §4.4 step-5 write) ─────────────

    /// **A DSR completion writes a ledger entry recording the opaque subject + holders + key epochs.**
    #[test]
    fn a_dsr_completion_writes_a_pii_free_entry() {
        let ledger = ErasureLedger::new();
        let is_new = ledger.record_completion(
            DsrId("dsr:1".into()),
            "p-7".into(),
            "acme".into(),
            vec!["oltp:identity_oltp".into(), "blob:blob_store".into()],
            vec![epoch("oltp:identity_oltp", Some(7)), epoch("blob:blob_store", Some(9))],
            140,
            1_700_000_000,
        );
        assert!(is_new, "the first completion writes a NEW entry");
        assert_eq!(ledger.len(), 1);
        let e = ledger.entry(&DsrId("dsr:1".into())).unwrap();
        assert_eq!(e.subject_token, "p-7");
        assert_eq!(e.tenant_token, "acme");
        assert!(e.erased_holder("oltp:identity_oltp"));
        assert!(e.erased_holder("blob:blob_store"));
        assert!(!e.erased_holder("nonexistent:holder"));
        // the key epochs are recorded (the §4.2 independent-check trail).
        assert_eq!(e.key_epochs_destroyed.len(), 2);
        assert_eq!(e.completed_at_offset, 140);
    }

    /// **The ledger write is IDEMPOTENT** — re-completing the same DSR does NOT duplicate the entry
    /// (a worker restart re-driving the same `dsr_id` over the durable checklist). The FIRST
    /// completion's offset/time is retained (re-erasure drives off the first completion). Kills the
    /// mutant that overwrites or appends on a duplicate.
    #[test]
    fn the_ledger_write_is_idempotent_per_dsr() {
        let ledger = ErasureLedger::new();
        let id = DsrId("dsr:2".into());
        assert!(ledger.record_completion(
            id.clone(), "p-9".into(), "acme".into(),
            vec!["a".into()], vec![epoch("a", Some(1))], 100, 500,
        ));
        // A re-completion (restart) with DIFFERENT later facts is a NO-OP (returns false; original kept).
        assert!(!ledger.record_completion(
            id.clone(), "p-9".into(), "acme".into(),
            vec!["a".into(), "b".into()], vec![epoch("a", Some(1)), epoch("b", Some(2))], 200, 999,
        ), "a duplicate completion is a no-op");
        assert_eq!(ledger.len(), 1, "no duplicate entry");
        let e = ledger.entry(&id).unwrap();
        assert_eq!(e.completed_at_offset, 100, "the FIRST completion's offset is retained");
        assert_eq!(e.completed_at_secs, 500, "the FIRST completion's time is retained");
        assert_eq!(e.holders_erased, vec!["a".to_string()], "the first holder set is retained");
    }

    // ───────────── the re-erasure-trigger read: ONLY post-PIT erasures ─────────────

    /// **MANDATORY-CORE: `post_pit_records_after` returns ONLY erasures completed AFTER the PIT** —
    /// the resurrection-risk set Storage's `post_restore_reerase` re-applies. An erasure at-or-before
    /// T is already dead in the backup (crypto-shred reaches the backup, §3.2) and is NOT returned.
    /// Kills the mutant that flips `>` to `>=`/`<` or returns all records.
    #[test]
    fn post_pit_records_selects_only_post_pit_erasures() {
        let ledger = ErasureLedger::new();
        ledger.record_completion(DsrId("dsr:a".into()), "pre".into(), "acme".into(), vec![], vec![], 50, 0);
        ledger.record_completion(DsrId("dsr:b".into()), "at".into(), "acme".into(), vec![], vec![], 100, 0);
        ledger.record_completion(DsrId("dsr:c".into()), "post".into(), "acme".into(), vec![], vec![], 140, 0);
        ledger.record_completion(DsrId("dsr:d".into()), "later".into(), "acme".into(), vec![], vec![], 200, 0);

        let after = ledger.post_pit_records_after(100);
        let subjects: Vec<&str> = after.iter().map(|r| r.subject.as_str()).collect();
        // only the post-PIT (offset > 100) erasures, in (offset, subject) order.
        assert_eq!(subjects, vec!["post", "later"]);
        // the field shape mirrors storage's ErasureRecord (subject, tenant, completed_at_offset).
        assert_eq!(after[0].tenant, "acme");
        assert_eq!(after[0].completed_at_offset, 140);
    }

    /// **A whole-tenant offboarding entry is NOT projected into the per-subject post-PIT set** (its
    /// re-erasure is a tenant-KEK re-destruction, not a per-subject DEK re-shred). It IS recorded for
    /// audit. Kills the mutant that drops the `subject_token != "*"` filter.
    #[test]
    fn a_tenant_offboarding_is_not_a_per_subject_post_pit_record() {
        let ledger = ErasureLedger::new();
        ledger.record_completion(DsrId("dsr:off".into()), "*".into(), "acme".into(), vec![], vec![], 140, 0);
        ledger.record_completion(DsrId("dsr:sub".into()), "p-1".into(), "acme".into(), vec![], vec![], 140, 0);
        assert_eq!(ledger.len(), 2, "both are recorded (the offboarding IS in the ledger for audit)");
        let after = ledger.post_pit_records_after(100);
        let subjects: Vec<&str> = after.iter().map(|r| r.subject.as_str()).collect();
        assert_eq!(subjects, vec!["p-1"], "only the per-subject erasure is a re-erasure target");
    }

    /// A subject erased BEFORE the backup is NOT in the post-PIT set (already dead by construction).
    #[test]
    fn a_pre_pit_erasure_is_not_a_re_erasure_target() {
        let ledger = ErasureLedger::new();
        ledger.record_completion(DsrId("dsr:pre".into()), "p-pre".into(), "acme".into(), vec![], vec![], 60, 0);
        assert!(ledger.post_pit_records_after(100).is_empty(), "a pre-PIT erasure is not re-applied");
    }

    /// The len/is_empty accessors are exact (they feed the telemetry + drill assertions).
    #[test]
    fn len_and_is_empty_are_exact() {
        let ledger = ErasureLedger::new();
        assert!(ledger.is_empty());
        assert_eq!(ledger.len(), 0);
        ledger.record_completion(DsrId("dsr:1".into()), "p".into(), "acme".into(), vec![], vec![], 10, 0);
        assert!(!ledger.is_empty());
        assert_eq!(ledger.len(), 1);
    }

    // ───────────── the recursive holder is NON-shred-erasable (§2.3) ─────────────

    /// **The ledger's `erase` RETAINS the PII-free record (NON-shred-erasable) — the §2.3 carve-out.**
    /// It never destroys a key (`None` epoch) and the receipt records the non-shred-erasable verdict.
    /// This is the property that makes the ledger the ONE holder the per-tenant crypto-shred does NOT
    /// erase away (it must survive to drive re-erasure).
    #[test]
    fn the_ledger_erase_is_a_non_shred_erasable_carve_out() {
        let ledger = ErasureLedger::new();
        let receipt = ledger
            .erase(EraseScope::Subject { subject: subject_ref("p-1"), tenant: tenant() })
            .unwrap();
        assert_eq!(
            receipt.receipt.key_epoch_destroyed, None,
            "the ledger erase destroys NO key — it is non-shred-erasable (it must survive)"
        );
        assert_eq!(receipt.receipt.operation, "erase");
        // and a record written BEFORE the erase still survives the erase (the record is retained).
        ledger.record_completion(DsrId("dsr:1".into()), "p-1".into(), "acme".into(), vec![], vec![], 140, 0);
        ledger.erase(EraseScope::Subject { subject: subject_ref("p-1"), tenant: tenant() }).unwrap();
        assert_eq!(ledger.len(), 1, "the erase RETAINS the record (it drives re-erasure)");
        assert!(
            !ledger.post_pit_records_after(100).is_empty(),
            "the retained record STILL drives re-erasure after the subject's erase"
        );
    }

    /// The ledger's `locate`/`export` report the PII-free record (0 recoverable); `rectify` is refused
    /// (editing the re-erasure source is a tamper-evidence violation); `restrict` is a no-op ack.
    #[test]
    fn the_recursive_holder_read_ops_report_pii_free() {
        let ledger = ErasureLedger::new();
        let loc = ledger.locate(&subject_ref("p-1"), tenant()).unwrap();
        assert_eq!(
            loc.receipt.content_hash,
            Receipt::content_addressed("locate", ERASURE_LEDGER_STORE, "*", "acme", "located:0-recoverable", None, 0).content_hash,
        );
        assert!(ledger.export(&subject_ref("p-1"), tenant()).is_ok());
        assert!(ledger.rectify(&subject_ref("p-1"), Patch("x".into())).is_err(), "the ledger is NEVER rectified");
        assert!(ledger.restrict(&subject_ref("p-1"), true).is_ok(), "restrict is a no-op ack");
    }

    /// The telemetry signal name + unit are pinned (the `erasure_ledger_entries` SLO).
    #[test]
    fn telemetry_name_and_unit_are_pinned() {
        assert_eq!(ERASURE_LEDGER_ENTRIES.0, "gdpr.erasure_ledger_entries");
        assert_eq!(ERASURE_LEDGER_ENTRIES.1, "count");
        assert_eq!(ERASURE_LEDGER_STORE, "gdpr_erasure_ledger:erasure_ledger");
    }
}
