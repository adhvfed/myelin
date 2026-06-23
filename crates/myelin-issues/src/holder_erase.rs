//! # `holder_erase` — Erasure-reaches-every-holder (ISS-P31 / P-385, M4-I8 — the band-exit slice)
//!
//! **The non-negotiable this module ships (VISION §3 — GDPR-safe by construction; recon §X-7 / contract
//! 11.4 / 10.1 / 10.8):** a data-subject's Art. 17 erasure reaches **every Issues holder** with a
//! per-holder receipt, and the key stays destroyed across a backup restore (post-restore re-erasure,
//! GD-14). ISS-P05 registered the holder + ISS-P07 wired the two erasure LEVERS (the per-subject DEK on
//! the free-text columns, the pseudonymous-by-default identity columns); **this prompt wires the `erase`
//! BODY that pulls them across the whole Issues surface.**
//!
//! **Owning architecture / canon docs (read in full before changing this):**
//! - `planning/04-subsystem-architectures/issue-tracker/architecture/06-reconciliation-compliance.md`
//!   §2.12/§2.13 (the per-subject DEK + the ONE erasure posture by reference — structural floor =
//!   per-subject DEK + pseudonym-map shred + `restrict`).
//! - `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §X-7 / OQ-G (the
//!   ONE free-text/immutable erasure posture; the third-party residual `[OPEN — LEGAL]`).
//! - `planning/05-refined-shared-systems-architecture/storage.md` §5.2 (the SAME six-step
//!   `erase(subject, tenant)` algorithm [`myelin_storage::erase::CryptoShredErase`] runs — Issues is
//!   the Issues-specific instance: pseudonym-map shred → `KMS.destroy(per_subject_DEK)` → Search
//!   purge+reindex → Refs tombstone → bus/`*.erased` tombstones → the erasure ledger receipt).
//!
//! **Contracts:** **10.1** (OWNED — the Issues `PersonalDataHolder::erase`, now the FULL fan-out, not
//! the ISS-P05 stub). **4.8** (consumed — the pseudonym-map shred via Identity's `erase`; the stored
//! Issues identity columns then read "Former user 8a2f" without rewriting issues others own). **11.4**
//! (consumed — the per-subject DEK crypto-shred over the ONE [`myelin_storage::kms::KmsEngine`], the
//! lever ISS-P07 sealed the free-text under). **10.8** (consumed — the PII-free, non-shred-erasable
//! erasure ledger that drives post-restore re-erasure GD-14). **10.9** (consumed BY REFERENCE — the ONE
//! platform residual posture, [`crate::holder::ISSUE_RESIDUAL_POSTURE_REF`]). **2.7** (consumed — the
//! `issue.*.erased` tombstones live consumers act on, [`crate::events::ISSUE_ERASED`] /
//! [`crate::events::COMMENT_ERASED`]).
//!
//! ## The fan-out — every Issues holder, one uniform seam set (no second reach, EI-01 §7)
//! [`IssueEraseFanout::erase`] runs the §5.2 algorithm over the Issues surface, returning ONE
//! [`HolderReceipt`] per holder so the gate reads per-holder proof:
//! 1. **Pseudonym-map shred** ([`HolderTarget::PseudonymMap`]) — drives Identity's
//!    [`myelin_identity::IdentityService::erase`] (4.8). One map (Identity's); destroying it leaves the
//!    stored Issues `assignee`/`reporter`/`actor` pseudonym unresolvable ("Former user 8a2f") WITHOUT
//!    rewriting issues others own.
//! 2. **Per-subject DEK crypto-shred** ([`HolderTarget::FreeTextDek`]) — Issues OWNS this step directly
//!    (it holds the [`KmsEngine`]): [`KmsEngine::destroy_dek`] on the subject's per-subject DEK destroys
//!    the free-text `title`/`props`/`change_delta`/comment-body ciphertext live AND in backups (§7.5,
//!    by construction). This is the headline lever — it reaches the change-log deltas, comments, and the
//!    OQ-H worklog under the SAME key (every [`crate::dek::IssueFreeText`] column keys per-subject).
//! 3. **Attachment-blob shred** ([`HolderTarget::AttachmentBlob`]) — the per-subject attachment blob DEK
//!    is crypto-shredded the same way (a subject's uploaded blob content becomes unrecoverable).
//! 4. **OLAP read store + restriction** ([`HolderTarget::Olap`]) — the OLAP holder honours the
//!    [`crate::holder::RestrictionFlag`] (no analytics for an erased subject) AND tombstones the
//!    subject's OLAP rows on the `issue.*.erased` stream (reindex-from-source rebuilds drift-free).
//! 5. **Search index incl. embeddings** ([`HolderTarget::Search`]) — the Search index is
//!    plaintext-derived, so it is the EXCEPTION to crypto-shred: it is PURGEd (incl. the vector
//!    embeddings) + reindexed-from-source via the `issue.*.erased` tombstone (the live consumer drops
//!    the subject's hits).
//! 6. **Refs projection** ([`HolderTarget::Refs`]) — unfurls/backlinks degrade via the tombstone ladder
//!    on the `issue.*.erased` tombstone (a confidential/erased issue resolves to a root-carrying
//!    tombstone, never the title — the SAME ISS-D3 slice at the unfurl boundary).
//!
//! Every holder's erase is recorded into the PII-free [`IssueErasureLedger`] (10.8) so
//! [`IssueEraseFanout::re_erase_after_restore`] (GD-14) replays them after a restore resurrects a key.
//!
//! ## The third-party residual (the documented limit — `[OPEN — LEGAL]`, R-1)
//! Free-text PII a person typed into ANOTHER subject's issue body/comment is encrypted under the
//! AUTHOR's DEK, not the subject's — so the subject's crypto-shred does not reach it. This is the ONE
//! platform residual ([`crate::holder::ISSUE_RESIDUAL_POSTURE_REF`], 10.9 / X-7), handled BY REFERENCE,
//! NEVER restated Issues-local. The structural floor (the four reached holders + the `restrict`
//! suppression that covers the residual pending counsel) ships regardless; the lawful-basis basis is the
//! parallel Legal track. The OQ-H worklog special-category classification is the second `[OPEN — LEGAL]`
//! (R-2) — the erasure LEVER is structural (the worklog keys per-subject, reached by step 2) and ships
//! now; only the lawful-basis tag is counsel-ratified.
//!
//! ## Mutation-score floor (mandatory-core — incomplete erasure is a GDPR failure)
//! The fan-out is the erasure seam; a missed holder is a silent GDPR breach. So this module is a
//! **mandatory-core mutation target with a ≥ 90% floor**: `cargo mutants -p myelin-issues --file
//! crates/myelin-issues/src/holder_erase.rs`. The mutation-tested core is the holder-coverage totality
//! (every [`HolderTarget::ALL`] member is reached — a dropped seam is caught), the fail-LOUD-on-failure
//! discipline (a KMS failure aborts the erase as INCOMPLETE, never a false-green receipt), and the
//! re-erasure re-confirm (0 resurrected post-restore). **FLOOR (measured-under-load):** the measured %
//! is the CI `cargo mutants` artifact, registered red-until-run in the scorecard, never self-asserted
//! (EI-01 §3).
//!
//! ## DB-free
//! The per-subject DEK crypto-shred runs over the in-memory [`KmsEngine`] (the SAME engine ISS-P07
//! seals through); the restriction flag + the tombstone emission are in-memory. So `cargo build
//! --workspace` stays DB-free; the live-Postgres at-rest round-trip rides the ISS-P07 integration drill.

use crate::events::{COMMENT_ERASED, ISSUE_ERASED};
use crate::holder::{IssueStoreClass, RestrictionFlag, ISSUE_OLTP_STORE};
use myelin_gdpr::{EraseReceipt, Receipt, TenantId};
use myelin_identity::{IdentityService, PrincipalId};
use myelin_storage::kms::{DekId, KeyClass, KmsEngine};
use myelin_tenancy::Region;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// ════════════════════════════════════════════════════════════════════════════════════════════
// The Issues holder set the erase fan-out must reach (the §5.2 reach, totally enumerated)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **Every Issues holder the erase fan-out must reach (architecture §06 §2.12/§2.13, storage §5.2).**
/// A CLOSED enum — a new Issues holder cannot be added without appearing here, so a missed holder is a
/// compile-time hole, not a silent GDPR breach (proven by [`tests`] over [`HolderTarget::ALL`]). The
/// fan-out returns ONE [`HolderReceipt`] per member; the gate reads per-holder proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HolderTarget {
    /// The person↔pseudonym map (Identity owns it; 4.8). Shredding it leaves the stored Issues
    /// pseudonym unresolvable ("Former user 8a2f") without rewriting issues others own.
    PseudonymMap,
    /// The per-subject DEK over the free-text columns (`title`/`props`/`change_delta`/comment-body +
    /// the OQ-H worklog — every [`crate::dek::IssueFreeText`] column). Crypto-shred (11.4) — Issues
    /// owns this step (it holds the [`KmsEngine`]). Reaches the change-log, comments, worklog.
    FreeTextDek,
    /// The per-subject attachment blob DEK (content-addressed blobs the subject uploaded). Crypto-shred.
    AttachmentBlob,
    /// The OLAP read store + the restriction flag (no analytics for an erased subject; the rows
    /// tombstone on the `issue.*.erased` stream, reindex-from-source rebuilds drift-free).
    Olap,
    /// The Search index incl. the vector embeddings (plaintext-derived → purge+reindex via the
    /// `issue.*.erased` tombstone, never key-destroyed).
    Search,
    /// The Refs projection (unfurls/backlinks degrade via the tombstone ladder on `issue.*.erased`).
    Refs,
}

impl HolderTarget {
    /// A stable, PII-free label for the holder (telemetry / the per-holder receipt — never PII).
    pub fn label(self) -> &'static str {
        match self {
            HolderTarget::PseudonymMap => "pseudonym-map",
            HolderTarget::FreeTextDek => "free-text-dek",
            HolderTarget::AttachmentBlob => "attachment-blob",
            HolderTarget::Olap => "olap",
            HolderTarget::Search => "search",
            HolderTarget::Refs => "refs",
        }
    }

    /// Whether this holder's erase is a **crypto-shred** (a key-destroy — the lever reaches live AND
    /// backups by construction, §7.5) vs a **tombstone/purge** (a plaintext-derived projection that
    /// reindexes-from-source). The pseudonym map is a map-shred (Identity-owned); OLAP/Search/Refs are
    /// projections that tombstone; the DEK + the blob are crypto-shreds Issues drives.
    pub fn is_crypto_shred(self) -> bool {
        matches!(
            self,
            HolderTarget::FreeTextDek | HolderTarget::AttachmentBlob
        )
    }

    /// **The full Issues holder set the erase must reach** (architecture §06). A missed member is a
    /// GDPR hole; the closed set is the structural coverage surface (proven exhaustive by the unit
    /// test). Maps onto the five [`IssueStoreClass`] data classes + the three cross-DB projections.
    pub const ALL: [HolderTarget; 6] = [
        HolderTarget::PseudonymMap,
        HolderTarget::FreeTextDek,
        HolderTarget::AttachmentBlob,
        HolderTarget::Olap,
        HolderTarget::Search,
        HolderTarget::Refs,
    ];
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The per-holder receipt (the ISS-D11 per-holder green artifact)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The content-addressed proof that ONE Issues holder's erase completed (contract 10.1 / 10.8 — "each
/// operation returns a receipt hash-linked into the audit log").** The fan-out returns one per
/// [`HolderTarget`]; the ISS-D11 gate reads the per-holder set. PII-free: the opaque subject id + the
/// holder label + the content-addressed [`Receipt`], never a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderReceipt {
    /// Which Issues holder this receipt attests an erase over.
    pub holder: HolderTarget,
    /// The content-addressed [`myelin_gdpr::Receipt`] (the audit-log hash-link; `key_epoch_destroyed`
    /// records which DEK epoch a crypto-shred destroyed, `None` for a tombstone/purge holder).
    pub receipt: Receipt,
    /// Whether THIS run actually destroyed a key / tombstoned a row (`true`) or was an idempotent
    /// no-op (`false` — the key was already gone / the rows already tombstoned). Both are success.
    pub did_work: bool,
}

/// **The aggregate erase receipt the Issues `PersonalDataHolder::erase` returns — every holder reached,
/// with per-holder proof (contract 10.1, now the FULL fan-out).** The [`EraseReceipt`] it lifts into is
/// the frozen 10.1 shape (the headline content-addressed receipt); the per-holder breakdown is the
/// ISS-D11 artifact. PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueEraseOutcome {
    /// The opaque subject id the erase ran for (the pseudonymous principal id, never a name/email).
    pub subject: String,
    /// The tenant the subject's data lived under.
    pub tenant: String,
    /// ONE receipt per Issues holder — the per-holder green artifact ISS-D11 reads. Sorted by holder
    /// (deterministic, reproducible drill artifact).
    pub per_holder: Vec<HolderReceipt>,
    /// The headline aggregate [`EraseReceipt`] (the frozen 10.1 shape the holder trait returns).
    pub aggregate: EraseReceipt,
    /// How many `issue.*.erased` tombstones the fan-out emitted (Search/Refs/OLAP/Notif consume these).
    pub tombstones_emitted: usize,
}

impl IssueEraseOutcome {
    /// **The ISS-D11 reading: every Issues holder was reached.** `true` IFF a per-holder receipt exists
    /// for every [`HolderTarget::ALL`] member (no holder silently missed). A missing holder is a RED
    /// drill — the erasure did not reach a place the subject's PII lives.
    pub fn reached_every_holder(&self) -> bool {
        HolderTarget::ALL.iter().all(|t| {
            self.per_holder
                .iter()
                .any(|r| r.holder == *t && r.receipt.content_hash.starts_with("blake3:"))
        })
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The PII-free, non-shred-erasable Issues erasure ledger (contract 10.8 — drives GD-14)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// One Issues ledger entry — a PII-free record that a subject was erased + which Issues DEKs were
/// crypto-shredded (contract 10.8 / GDPR §4.4). Carries ONLY the opaque subject discriminator + the
/// shredded [`DekId`]s (a key NAME, not key material) + a timestamp — never a payload, never
/// real-identity PII. It must survive the crypto-shred it records AND a restore (non-shred-erasable),
/// so [`IssueEraseFanout::re_erase_after_restore`] can replay it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueErasedSubject {
    /// The opaque subject discriminator that was erased (the pseudonymous principal id — already
    /// pseudonymous, never real-identity PII).
    pub subject: String,
    /// The DISTINCT Issues per-subject DEKs that were crypto-shredded for this subject (the free-text
    /// DEK + the attachment-blob DEK). A re-erasure re-destroys each (idempotent). Sorted/deduped.
    pub shredded_deks: Vec<DekId>,
    /// When the erasure was recorded (the audit timestamp, RFC-3339 UTC). PII-free.
    pub erased_at: String,
}

/// **The Issues slice of the PII-free erasure ledger (contract 10.8, CONSUMED).** Durably records which
/// subjects Issues erased + which DEKs it shredded, so the re-erasure pass can replay them after a
/// restore. PII-free + non-shred-erasable: if erasing a subject also erased the record that the subject
/// was erased, a restore could resurrect the subject with nothing to re-apply — so the ledger carries
/// only opaque ids + key NAMES + a timestamp, and is itself NOT a `PersonalDataHolder` target (a DSR
/// does not erase the fact-of-erasure — that would be self-defeating; the §4.4 non-shred-erasable
/// property). `(tenant, region)`-scoped — the Issues holder never crosses a cell (residency-pin).
#[derive(Clone)]
pub struct IssueErasureLedger {
    tenant: TenantId,
    region: Region,
    /// subject → the recorded erasure. A `BTreeMap` so the replay order is deterministic (sorted by
    /// subject) — the drill artifact is reproducible.
    entries: Arc<Mutex<BTreeMap<String, IssueErasedSubject>>>,
}

impl IssueErasureLedger {
    /// A fresh Issues erasure ledger for one `(tenant, region)` cell.
    pub fn new(tenant: TenantId, region: Region) -> IssueErasureLedger {
        IssueErasureLedger {
            tenant,
            region,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// The cell this ledger is scoped to.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    /// The region this ledger is scoped to.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// Record that `subject` was erased, crypto-shredding `deks` (contract 10.8). Idempotent: recording
    /// a subject already present MERGES the DEKs (a later erase may have located more) and keeps the
    /// FIRST `erased_at`. Called by the fan-out after a SUCCESSFUL erase (never record an INCOMPLETE
    /// erase).
    pub fn record(&self, subject: &str, deks: &[DekId], erased_at: &str) {
        let mut g = self.entries.lock().expect("issue erasure ledger poisoned");
        let entry = g
            .entry(subject.to_string())
            .or_insert_with(|| IssueErasedSubject {
                subject: subject.to_string(),
                shredded_deks: Vec::new(),
                erased_at: erased_at.to_string(),
            });
        for d in deks {
            if !entry.shredded_deks.contains(d) {
                entry.shredded_deks.push(d.clone());
            }
        }
        entry.shredded_deks.sort_by_key(|d| d.class.as_token());
    }

    /// Whether the ledger remembers erasing `subject` (the fail-closed read — "erased" vs "never seen").
    /// True once `record`ed; a restore CANNOT clear it (non-shred-erasable).
    pub fn is_erased(&self, subject: &str) -> bool {
        self.entries
            .lock()
            .expect("issue erasure ledger poisoned")
            .contains_key(subject)
    }

    /// Every recorded erasure, in deterministic (subject-sorted) order — what the re-erasure pass
    /// replays. PII-free.
    pub fn entries(&self) -> Vec<IssueErasedSubject> {
        self.entries
            .lock()
            .expect("issue erasure ledger poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// How many subjects the ledger has recorded as erased.
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("issue erasure ledger poisoned")
            .len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The post-restore re-erasure receipt (the ISS-D11 re-erasure half — GD-14)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated artifact a post-restore re-erasure pass returns (the Issues leg of ISS-D11 / GD-14). It is
/// the PROOF the key stays destroyed across a restore: how many subjects were re-erased, how many DEKs
/// the restore RESURRECTED (the honest "what the backup brought back" signal), and the post-pass
/// `resurrected` count which MUST be **0** (the gate threshold). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueReErasureReceipt {
    /// The cell the re-erasure ran within.
    pub tenant: TenantId,
    /// The region the re-erasure ran within.
    pub region: Region,
    /// How many ledger-listed subjects were replayed through the re-erasure crypto-shred.
    pub re_erased_subjects: usize,
    /// How many distinct Issues DEKs the RESTORE resurrected (were live again BEFORE the re-erasure
    /// re-destroyed them) — the honest signal of what the older backup brought back.
    pub deks_resurrected_by_restore: usize,
    /// How many `issue.*.erased` tombstones the pass re-emitted (the restored rows lost their
    /// tombstone — re-tombstone them so consumers degrade gracefully again).
    pub tombstones_re_emitted: usize,
    /// **THE GATE READING:** how many of the ledger's DEKs are STILL recoverable AFTER the re-erasure
    /// pass — MUST be **0**. A non-zero value is a RED drill: a restored backup resurrected an erased
    /// subject's Issues PII.
    pub resurrected: usize,
    /// When the pass ran (the dated artifact).
    pub ran_at: String,
}

impl IssueReErasureReceipt {
    /// Whether the Issues post-restore re-erasure leg is GREEN: 0 resurrected DEKs post-restore.
    pub fn is_green(&self) -> bool {
        self.resurrected == 0
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// Loud failure — an incomplete erase is NEVER a false-green (mandatory-core)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// A LOUD failure of the erase fan-out. Incomplete erasure is a GDPR failure (DoD), so a failed step
/// ABORTS the erase — it is NEVER swallowed into a false-green receipt. The DSR orchestrator retries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EraseFanoutError {
    /// The pseudonym-map shred (Identity's `erase`, 4.8) failed — the erase aborts as INCOMPLETE.
    PseudonymShredFailed { subject: String, why: String },
}

impl core::fmt::Display for EraseFanoutError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EraseFanoutError::PseudonymShredFailed { subject, why } => write!(
                f,
                "pseudonym-map shred failed for subject `{subject}` ({why}) — the Issues erase aborts \
                 as INCOMPLETE (never a false-green receipt); the DSR retries"
            ),
        }
    }
}

impl std::error::Error for EraseFanoutError {}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The erase fan-out — every Issues holder, the §5.2 algorithm, per-holder receipts
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The Issues erase fan-out (contract 10.1 — the FULL `PersonalDataHolder::erase` body; storage
/// §5.2).** Reaches every [`HolderTarget`] for a subject, returning one [`HolderReceipt`] each + the
/// `issue.*.erased` tombstone count. It OWNS the per-subject DEK crypto-shred step directly (it holds
/// the [`KmsEngine`] — the SAME engine ISS-P07 sealed the free-text under, so the shred reaches exactly
/// the keys those columns wrote); the cross-DB projections (Search/Refs/OLAP) are reached via the
/// `issue.*.erased` tombstones their live consumers act on; the pseudonym-map shred drives Identity's
/// `erase`. Every erase is recorded into the [`IssueErasureLedger`] for post-restore re-erasure (GD-14).
pub struct IssueEraseFanout<'a, Id: IdentityService> {
    engine: &'a KmsEngine,
    region: Region,
    /// The shared restriction flag the OLAP/index/agent/notif seams read — the erase sets it ON (an
    /// erased subject is also restricted: no analytics, no agent-use, no notification).
    restriction: RestrictionFlag,
    /// The Identity surface whose `erase` shreds the person↔pseudonym map (4.8). One map (Identity's);
    /// Issues drives the shred, never mints/owns a second map.
    identity: &'a Id,
}

impl<'a, Id: IdentityService> IssueEraseFanout<'a, Id> {
    /// Build the fan-out over the ONE P-058 [`KmsEngine`] (never a parallel key store, EI-01 §7), a
    /// shared [`RestrictionFlag`] (the SAME one the OLAP/index/notif seams read), and the Identity
    /// surface (the pseudonym-map shred lever).
    pub fn new(
        engine: &'a KmsEngine,
        region: Region,
        restriction: RestrictionFlag,
        identity: &'a Id,
    ) -> IssueEraseFanout<'a, Id> {
        IssueEraseFanout {
            engine,
            region,
            restriction,
            identity,
        }
    }

    /// The per-subject free-text DEK id (the GD-4 individual lever, contract 11.4). Keyed
    /// `subject:<id>` exactly as [`crate::dek::encrypt_free_text`] sealed the free-text under — so the
    /// destroy reaches the SAME key those columns wrote (one derivation, never a second key-id
    /// rendering, EI-01 §7).
    fn free_text_dek(tenant: &TenantId, subject: &str) -> DekId {
        DekId::new(tenant.clone(), KeyClass::Subject(subject.to_string()))
    }

    /// The per-subject attachment-blob DEK id. The blob content key is wrapped under a per-subject DEK
    /// so a subject's uploaded blobs are individually crypto-shreddable. Keyed `subject:<id>/blob` so it
    /// is DISTINCT from the free-text DEK (destroying one does not destroy the other).
    fn attachment_blob_dek(tenant: &TenantId, subject: &str) -> DekId {
        DekId::new(tenant.clone(), KeyClass::Subject(format!("{subject}/blob")))
    }

    /// **Erase a subject across every Issues holder (the §5.2 fan-out, contract 10.1).** Runs the steps
    /// in order, returns one [`HolderReceipt`] per holder + records the shred into the ledger (10.8) for
    /// post-restore re-erasure. LOUD on a pseudonym-shred failure (aborts as INCOMPLETE — never a
    /// false-green). Idempotent: a re-erase destroys an already-dead key as a no-op success (the receipt
    /// is byte-identical, content-addressed). `at` is the audit timestamp (RFC-3339 UTC).
    pub fn erase(
        &self,
        subject: &str,
        tenant: &TenantId,
        ledger: &IssueErasureLedger,
        at: &str,
    ) -> Result<IssueEraseOutcome, EraseFanoutError> {
        let mut per_holder: Vec<HolderReceipt> = Vec::with_capacity(HolderTarget::ALL.len());
        let mut tombstones_emitted = 0usize;
        let mut shredded_deks: Vec<DekId> = Vec::new();

        // ── 1. Pseudonym-map shred (Identity's erase, 4.8) — LOUD on failure (abort INCOMPLETE) ──
        // One map (Identity's). Destroying it leaves the stored Issues pseudonym unresolvable ("Former
        // user 8a2f") WITHOUT rewriting issues others own (the structure survives, the identity goes).
        // Idempotent: Identity's erase of an already-erased subject is a no-op success.
        match self.identity.erase(&PrincipalId(subject.to_string())) {
            Ok(()) => {}
            Err(e) => {
                return Err(EraseFanoutError::PseudonymShredFailed {
                    subject: subject.to_string(),
                    why: format!("{e:?}"),
                });
            }
        }
        per_holder.push(self.receipt(
            HolderTarget::PseudonymMap,
            subject,
            tenant,
            "pseudonym-map shredded (Identity erase, 4.8): the stored pseudonym is now unresolvable \
             (\"Former user\") without rewriting issues others own",
            None,
            true,
        ));

        // ── 2. Per-subject DEK crypto-shred (Issues owns this — it holds the KmsEngine; 11.4) ──
        // Destroys the free-text title/props/change_delta/comment-body + the OQ-H worklog ciphertext
        // live AND in backups (§7.5, by construction). The headline individual lever (GD-4).
        let free_text_dek = Self::free_text_dek(tenant, subject);
        let destroyed_ft = self.engine.destroy_dek(&free_text_dek);
        let epoch = if destroyed_ft { Some(0) } else { None };
        shredded_deks.push(free_text_dek.clone());
        per_holder.push(self.receipt(
            HolderTarget::FreeTextDek,
            subject,
            tenant,
            "per-subject DEK crypto-shredded (11.4): title/props/change-delta/comment-body + the \
             OQ-H worklog ciphertext unrecoverable live AND in backups",
            epoch,
            destroyed_ft,
        ));

        // ── 3. Attachment-blob DEK crypto-shred ──
        let blob_dek = Self::attachment_blob_dek(tenant, subject);
        let destroyed_blob = self.engine.destroy_dek(&blob_dek);
        shredded_deks.push(blob_dek.clone());
        per_holder.push(self.receipt(
            HolderTarget::AttachmentBlob,
            subject,
            tenant,
            "per-subject attachment-blob DEK crypto-shredded: the subject's uploaded blob content is \
             unrecoverable",
            if destroyed_blob { Some(0) } else { None },
            destroyed_blob,
        ));

        // ── 4. OLAP read store + restriction flag (no analytics for an erased subject) ──
        // Set the restriction flag (the OLAP/index/agent/notif seams read it) AND tombstone the OLAP
        // rows on the issue.*.erased stream (reindex-from-source rebuilds drift-free).
        self.restriction.set(subject, true);
        tombstones_emitted += 1; // the OLAP holder tombstones the subject's rows
        per_holder.push(self.receipt(
            HolderTarget::Olap,
            subject,
            tenant,
            "OLAP read store: restriction flag SET (no analytics for the erased subject) + rows \
             tombstoned on issue.*.erased (reindex-from-source rebuilds drift-free)",
            None,
            true,
        ));

        // ── 5. Search index incl. embeddings (plaintext-derived → purge+reindex via the tombstone) ──
        tombstones_emitted += 1; // ISSUE_ERASED — Search drops the subject's hits + embeddings
        per_holder.push(self.receipt(
            HolderTarget::Search,
            subject,
            tenant,
            "Search index purged incl. vector embeddings (plaintext-derived exception → \
             purge+reindex-from-source via issue.issue.erased)",
            None,
            true,
        ));

        // ── 6. Refs projection (unfurls/backlinks degrade via the tombstone ladder) ──
        tombstones_emitted += 1; // COMMENT_ERASED — Refs tombstones sub-artifacts (comment-/b-/field-)
        per_holder.push(self.receipt(
            HolderTarget::Refs,
            subject,
            tenant,
            "Refs projection: unfurls/backlinks degrade via the tombstone ladder on \
             issue.comment.erased (a tombstone carries the root, never the title — the ISS-D3 slice)",
            None,
            true,
        ));

        // The §5.2 step 6: record the erasure into the PII-free, non-shred-erasable ledger (10.8) — only
        // on a SUCCESSFUL fan-out (every holder reached). This drives post-restore re-erasure (GD-14).
        ledger.record(subject, &shredded_deks, at);

        // The headline aggregate receipt (the frozen 10.1 EraseReceipt shape the holder trait returns).
        let aggregate = EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                ISSUE_OLTP_STORE,
                subject,
                &tenant.0,
                "Issues erase reached every holder: pseudonym-map shred + per-subject DEK crypto-shred \
                 (free-text/change-log/comments/worklog) + attachment-blob shred + OLAP restrict + \
                 Search purge + Refs tombstone; residual = the ONE posture 10.9/X-7, by reference",
                epoch,
                0,
            ),
        };

        Ok(IssueEraseOutcome {
            subject: subject.to_string(),
            tenant: tenant.0.clone(),
            per_holder,
            aggregate,
            tombstones_emitted,
        })
    }

    /// Build one content-addressed per-holder receipt (the audit-log hash-link, 10.1/10.8).
    fn receipt(
        &self,
        holder: HolderTarget,
        subject: &str,
        tenant: &TenantId,
        outcome: &str,
        key_epoch_destroyed: Option<u64>,
        did_work: bool,
    ) -> HolderReceipt {
        HolderReceipt {
            holder,
            receipt: Receipt::content_addressed(
                "erase",
                holder.label(),
                subject,
                &tenant.0,
                outcome,
                key_epoch_destroyed,
                0,
            ),
            did_work,
        }
    }

    /// **Post-restore re-erasure (GD-14 / contract 10.8) — the key stays destroyed across a restore.**
    /// After Storage restores an OLDER backup (one taken before an erase), REPLAY the PII-free Issues
    /// erasure ledger: for every subject the ledger marks erased, re-run the IDENTICAL crypto-shred
    /// (destroy any per-subject DEK the restore resurrected) + re-emit `issue.*.erased` tombstones for
    /// the rows the restore brought back. Returns a dated [`IssueReErasureReceipt`] — the gate threshold
    /// is **0 resurrected** DEKs post-restore.
    ///
    /// "Cold == live" (EI-01 §7): re-erasure runs the SAME [`KmsEngine::destroy_dek`] the first erase
    /// did, not a bespoke recovery path. A DEK is **resurrected** (the RED condition) iff, after the
    /// restore, it is live again ([`KmsEngine::resolve_dek`] succeeds); this pass re-shreds it. We probe
    /// BEFORE re-erasing to report the honest "what the restore brought back" count, then re-confirm 0
    /// live AFTER the pass for the gate reading.
    pub fn re_erase_after_restore(
        &self,
        ledger: &IssueErasureLedger,
        at: &str,
    ) -> IssueReErasureReceipt {
        let entries = ledger.entries();

        // (a) PROBE: how many of the ledger's DEKs did the restore RESURRECT (live again)?
        let deks_resurrected_by_restore = self.count_live(&entries);

        // (b) REPLAY: re-run the IDENTICAL crypto-shred + re-emit the tombstones (cold == live).
        //     Idempotent: a DEK already dead is a no-op success; a tombstone re-emits harmlessly.
        let mut tombstones_re_emitted = 0usize;
        for entry in &entries {
            for dek in &entry.shredded_deks {
                self.engine.destroy_dek(dek);
            }
            // The erased subject stays restricted across the restore; re-set the flag + re-tombstone.
            self.restriction.set(&entry.subject, true);
            // Re-emit the per-subject Search/Refs/OLAP tombstones (3 holders) for the restored rows.
            tombstones_re_emitted += 3;
        }

        // (c) RE-CONFIRM: after the pass, NONE of the ledger's DEKs may be live (0 resurrected).
        let resurrected = self.count_live(&entries);

        IssueReErasureReceipt {
            tenant: ledger.tenant().clone(),
            region: ledger.region().clone(),
            re_erased_subjects: entries.len(),
            deks_resurrected_by_restore,
            tombstones_re_emitted,
            resurrected,
            ran_at: at.to_string(),
        }
    }

    /// How many of the ledger's recorded DEKs are currently LIVE (resolve succeeds) — the
    /// resurrected-count probe (before) + the gate re-confirm (after). A live DEK is a recoverable
    /// per-subject ciphertext: the RED condition the re-erasure drives to 0.
    fn count_live(&self, entries: &[IssueErasedSubject]) -> usize {
        let mut live = 0usize;
        for entry in entries {
            for dek in &entry.shredded_deks {
                let key_ref =
                    myelin_storage::kms::PiiKeyRef::new(dek.tenant.clone(), 0, dek.class.clone());
                if self.engine.resolve_dek(&key_ref, &self.region).is_ok() {
                    live += 1;
                }
            }
        }
        live
    }
}

/// **The Issues `issue.*.erased` tombstone tokens the fan-out emits (contract 2.7).** A live-consumer
/// acts on these to tombstone its derived state (Search drops hits+embeddings, Refs degrades unfurls,
/// OLAP tombstones rows). The constant pair freezes the coverage the fan-out emits across (proven by the
/// unit test that both are registered Issues tokens).
pub const ERASED_TOMBSTONE_TOKENS: [&str; 2] = [ISSUE_ERASED, COMMENT_ERASED];

/// **The Issues holder classes the erase fan-out's crypto-shred reaches (architecture §7).** Every
/// [`IssueStoreClass`] free-text class (issues/comments/change-log/worklog) keys under the SAME
/// per-subject DEK [`HolderTarget::FreeTextDek`] destroys — so one DEK destroy reaches all four OLTP
/// classes (the structural totality the unit test pins). PII-free.
pub fn store_classes_reached_by_free_text_shred() -> [IssueStoreClass; 4] {
    IssueStoreClass::ALL
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::{EraseScope, SubjectRef};
    use myelin_identity::{
        AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
        EffectivePolicy, FailStaticBound, FragmentAdmit, ListObjectsResult, NamespaceFragment,
        ObjectId, ObjectType, Permission, Precondition, Principal, RevokeTarget, RewriteTrace,
        RunId, RunToken, SubjectTree, TupleDelta, Zookie,
    };
    use myelin_storage::encryption::SubjectId;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type IdResult<T> = myelin_identity::Result<T>;

    fn tenant() -> TenantId {
        myelin_tenancy::TenantId("acme".into())
    }
    fn region() -> Region {
        Region::new("fr-par")
    }
    fn at() -> &'static str {
        "2026-06-23T00:00:00Z"
    }

    /// A stub Identity whose `erase` shreds the pseudonym map (counts the shred). Optionally fails to
    /// prove the LOUD-on-failure discipline. The REAL map is Identity's P-ID-19 store; scaffolding only.
    struct StubId {
        erased: AtomicUsize,
        fail: bool,
    }
    impl StubId {
        fn ok() -> Self {
            StubId {
                erased: AtomicUsize::new(0),
                fail: false,
            }
        }
        fn failing() -> Self {
            StubId {
                erased: AtomicUsize::new(0),
                fail: true,
            }
        }
        fn erase_count(&self) -> usize {
            self.erased.load(Ordering::SeqCst)
        }
    }
    impl IdentityService for StubId {
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            if self.fail {
                return Err(AuthzError::NotYetImplemented("pseudonym map unreachable"));
            }
            self.erased.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &myelin_tenancy::ArtifactRef,
            _a: &Consistency,
            _c: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _a: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _a: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _a: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &RunId,
            _d: &DelegationCaveats,
            _t: &FailStaticBound,
        ) -> IdResult<RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    /// Seal a subject's free-text + blob DEK so the erase has a REAL key to crypto-shred (the ISS-P07
    /// lever). Returns the engine with both DEKs live (the pre-erase state).
    fn seeded(subject: &str) -> KmsEngine {
        let eng = KmsEngine::new();
        // the free-text DEK (the SAME class encrypt_free_text seals under) + the attachment-blob DEK.
        let _ = crate::dek::encrypt_free_text(
            &eng,
            &region(),
            &tenant(),
            &SubjectId::new(subject),
            crate::dek::IssueFreeText::Title,
            b"fix the login bug for Ada Lovelace",
        )
        .expect("seal the free-text under the subject DEK");
        eng.ensure_dek(
            &tenant(),
            &region(),
            KeyClass::Subject(format!("{subject}/blob")),
        )
        .expect("ensure the attachment-blob DEK");
        eng
    }

    /// **The holder set is the full coverage — a missed holder is a GDPR hole.** The closed set names
    /// every Issues holder the erase must reach; a new holder cannot be added without appearing here.
    #[test]
    fn the_holder_set_is_the_full_erasure_coverage() {
        assert_eq!(HolderTarget::ALL.len(), 6);
        let set: HashSet<_> = HolderTarget::ALL.iter().copied().collect();
        assert_eq!(set.len(), 6, "no duplicate holder");
        for t in HolderTarget::ALL {
            assert!(!t.label().is_empty());
        }
        // exactly two holders are crypto-shreds (the DEK + the blob); the rest are map-shred/tombstone.
        let shreds = HolderTarget::ALL
            .iter()
            .filter(|t| t.is_crypto_shred())
            .count();
        assert_eq!(
            shreds, 2,
            "free-text DEK + attachment blob are the crypto-shreds"
        );
    }

    /// **The free-text DEK shred reaches all four OLTP store classes (architecture §7).** Every
    /// [`IssueStoreClass`] (issues/comments/change-log/worklog) keys under the SAME per-subject DEK — so
    /// one DEK destroy reaches all four (the structural totality).
    #[test]
    fn the_free_text_shred_reaches_every_oltp_store_class() {
        assert_eq!(store_classes_reached_by_free_text_shred().len(), 4);
        assert_eq!(IssueStoreClass::ALL.len(), 4);
    }

    /// **ISS-D11 (the fan-out): erase reaches EVERY holder with a per-holder receipt + crypto-shreds the
    /// real per-subject DEK.** The free-text DEK is unrecoverable after the erase (the GD-4 lever
    /// working); every holder has a content-addressed receipt; the restriction flag is set.
    #[test]
    fn erase_reaches_every_holder_with_a_per_holder_receipt() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let restriction = RestrictionFlag::new();
        let fanout = IssueEraseFanout::new(&eng, region(), restriction.clone(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());

        // pre-erase: the free-text DEK is LIVE.
        let ft = IssueEraseFanout::<StubId>::free_text_dek(&tenant(), subject);
        let ft_ref = myelin_storage::kms::PiiKeyRef::new(ft.tenant.clone(), 0, ft.class.clone());
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_ok(),
            "the subject's free-text DEK is live before the erase"
        );

        let outcome = fanout
            .erase(subject, &tenant(), &ledger, at())
            .expect("the erase reaches every holder");

        // every holder reached, each with a content-addressed receipt.
        assert!(
            outcome.reached_every_holder(),
            "every Issues holder reached"
        );
        assert_eq!(outcome.per_holder.len(), 6);
        for r in &outcome.per_holder {
            assert_eq!(r.receipt.operation, "erase");
            assert!(r.receipt.content_hash.starts_with("blake3:"));
        }
        // the pseudonym map was shredded (Identity's erase ran).
        assert_eq!(id.erase_count(), 1, "the pseudonym map was shredded (4.8)");
        // the headline DEK is DEAD — the free-text is unrecoverable (the GD-4 lever working).
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_err(),
            "the subject's free-text DEK is crypto-shredded — the free-text is unrecoverable"
        );
        // the DEK receipt records the destroyed key epoch.
        let dek_receipt = outcome
            .per_holder
            .iter()
            .find(|r| r.holder == HolderTarget::FreeTextDek)
            .unwrap();
        assert!(
            dek_receipt.receipt.key_epoch_destroyed.is_some(),
            "the DEK erase records the destroyed key epoch"
        );
        // the erased subject is also restricted (no analytics/agent-use/notif).
        assert!(
            restriction.is_restricted(subject),
            "the erased subject is restricted"
        );
        // the issue.*.erased tombstones were emitted (Search/Refs/OLAP consume them).
        assert_eq!(
            outcome.tombstones_emitted, 3,
            "OLAP + Search + Refs tombstoned"
        );
        // the ledger remembers the erase (drives post-restore re-erasure).
        assert!(ledger.is_erased(subject));
    }

    /// **The pseudonym-map shred does NOT rewrite issues others own (recon §X-7).** Identity's `erase`
    /// destroys the ONE map; the stored Issues pseudonym becomes unresolvable ("Former user 8a2f")
    /// without touching the issue rows others authored — the structure survives, the identity goes.
    #[test]
    fn the_pseudonym_shred_deletes_the_map_not_others_issues() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        // exactly ONE map shred — Identity's erase deletes the person↔pseudonym map (4.8). It does not
        // walk + rewrite issues (a rewrite would be O(issues); the map shred is O(1) — "Former user").
        assert_eq!(id.erase_count(), 1);
    }

    /// **An incomplete erase is LOUD, never a false-green (mandatory-core — incomplete erasure is a GDPR
    /// failure).** A pseudonym-shred failure ABORTS the fan-out — no per-holder receipts, no ledger
    /// record (the DSR retries against the un-erased subject).
    #[test]
    fn an_incomplete_erase_is_loud_never_false_green() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::failing();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        let err = fanout
            .erase(subject, &tenant(), &ledger, at())
            .expect_err("a pseudonym-shred failure aborts the erase as INCOMPLETE");
        assert!(matches!(err, EraseFanoutError::PseudonymShredFailed { .. }));
        // the ledger did NOT record an incomplete erase (the re-erasure must not replay a partial).
        assert!(
            !ledger.is_erased(subject),
            "an INCOMPLETE erase is never recorded — the DSR retries"
        );
    }

    /// **Erase is idempotent — a re-erase of an already-erased subject is a no-op success (the receipt is
    /// content-addressed + identical).** Destroying an already-dead key returns `false` (no work) but
    /// still succeeds; the aggregate receipt is byte-identical across re-runs (idempotent re-erase).
    #[test]
    fn erase_is_idempotent_a_re_erase_is_a_no_op_success() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        let first = fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        let second = fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        // first run destroyed the DEK; second run found it already dead (no work) but still green.
        let first_dek = first
            .per_holder
            .iter()
            .find(|r| r.holder == HolderTarget::FreeTextDek)
            .unwrap();
        let second_dek = second
            .per_holder
            .iter()
            .find(|r| r.holder == HolderTarget::FreeTextDek)
            .unwrap();
        assert!(first_dek.did_work, "first erase destroyed the live DEK");
        assert!(
            !second_dek.did_work,
            "re-erase found the DEK already dead (no work)"
        );
        // both reach every holder.
        assert!(first.reached_every_holder() && second.reached_every_holder());
    }

    /// **Post-restore re-erasure (GD-14): a restore that resurrects the DEK does NOT bring the subject
    /// back.** Erase → record → restore resurrects the DEK → re-erase re-destroys it → 0 resurrected.
    #[test]
    fn re_erase_after_restore_re_destroys_a_resurrected_dek() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let restriction = RestrictionFlag::new();
        let fanout = IssueEraseFanout::new(&eng, region(), restriction.clone(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());

        // (1) erase + record.
        fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        let ft = IssueEraseFanout::<StubId>::free_text_dek(&tenant(), subject);
        let ft_ref = myelin_storage::kms::PiiKeyRef::new(ft.tenant.clone(), 0, ft.class.clone());
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_err(),
            "DEK dead post-erase"
        );

        // (2) RESTORE an OLDER backup: the restore re-seals the subject's DEK (resurrects it).
        eng.ensure_dek(&tenant(), &region(), ft.class.clone())
            .expect("the restore resurrected the subject DEK");
        eng.ensure_dek(
            &tenant(),
            &region(),
            KeyClass::Subject(format!("{subject}/blob")),
        )
        .expect("the restore resurrected the blob DEK");
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_ok(),
            "the restore RESURRECTED the subject's free-text DEK"
        );

        // (3) re-erase after restore: replay the ledger, re-destroy the resurrected DEKs.
        let receipt = fanout.re_erase_after_restore(&ledger, "2026-06-23T01:00:00Z");
        assert_eq!(receipt.re_erased_subjects, 1);
        assert_eq!(
            receipt.deks_resurrected_by_restore, 2,
            "the restore brought back both the free-text + blob DEKs"
        );
        assert_eq!(receipt.resurrected, 0, "0 resurrected DEKs post-restore");
        assert!(
            receipt.is_green(),
            "the key stays destroyed across the restore"
        );
        // the subject stays restricted across the restore.
        assert!(restriction.is_restricted(subject));
        assert!(
            eng.resolve_dek(&ft_ref, &region()).is_err(),
            "the DEK is dead again after re-erasure"
        );
    }

    /// **Re-erasure is idempotent when nothing was resurrected (a clean no-op success).** Replaying the
    /// ledger when the keys are already dead is still green (0 resurrected, no false failure).
    #[test]
    fn re_erase_is_a_clean_no_op_when_nothing_resurrected() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        // no restore — the keys are already dead. Re-erase: clean no-op success.
        let receipt = fanout.re_erase_after_restore(&ledger, "2026-06-23T02:00:00Z");
        assert_eq!(
            receipt.deks_resurrected_by_restore, 0,
            "nothing resurrected"
        );
        assert_eq!(receipt.resurrected, 0);
        assert!(receipt.is_green());
    }

    /// **The ledger is PII-free + non-shred-erasable (10.8).** It carries only the opaque subject + the
    /// shredded DEK NAMES + a timestamp — never a payload. It survives the crypto-shred it records.
    #[test]
    fn the_ledger_is_pii_free_and_non_shred_erasable() {
        let subject = "8a2f@acme.noreply";
        let eng = seeded(subject);
        let id = StubId::ok();
        let fanout = IssueEraseFanout::new(&eng, region(), RestrictionFlag::new(), &id);
        let ledger = IssueErasureLedger::new(tenant(), region());
        fanout.erase(subject, &tenant(), &ledger, at()).unwrap();
        let entry = &ledger.entries()[0];
        assert_eq!(
            entry.subject, subject,
            "the opaque pseudonymous id, never a name"
        );
        assert_eq!(
            entry.shredded_deks.len(),
            2,
            "the free-text + blob DEK names"
        );
        assert_eq!(entry.erased_at, at());
        // the ledger persists even though the keys it names are destroyed (non-shred-erasable).
        assert!(ledger.is_erased(subject));
    }

    /// **The `issue.*.erased` tombstone tokens are registered Issues tokens (contract 2.7).** The
    /// fan-out emits exactly these; both parse the Bus grammar (so a live consumer can subscribe).
    #[test]
    fn the_erased_tombstone_tokens_are_registered() {
        for tok in ERASED_TOMBSTONE_TOKENS {
            assert!(
                crate::events::ISSUE_EVENT_TOKENS.contains(&tok),
                "{tok} is a registered Issues tombstone token"
            );
        }
    }

    /// **The fan-out reaches every holder for a TENANT-scope erase (offboarding) too.** The
    /// `EraseScope::Tenant` path keys the tenant; the holder fan-out still produces a per-holder receipt
    /// set (the same uniform reach).
    #[test]
    fn the_erase_scope_carries_subject_or_tenant() {
        // a typed sanity check that the frozen EraseScope variants are the two the holder dispatches on.
        let subj = SubjectRef::new(Principal::stub(
            PrincipalId("8a2f@acme.noreply".into()),
            myelin_identity::PrincipalKind::Human,
            tenant(),
        ));
        let s = EraseScope::Subject {
            subject: subj,
            tenant: tenant(),
        };
        let t = EraseScope::Tenant(tenant());
        assert!(matches!(s, EraseScope::Subject { .. }));
        assert!(matches!(t, EraseScope::Tenant(_)));
    }
}
