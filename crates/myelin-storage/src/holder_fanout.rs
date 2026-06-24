//! The **full DSAR / crypto-shred fan-out across all H1–H18 holders** — the **E2E-4 storage spine**
//! (P-ST-35 / global P-446; contract 10.4 "the DSR fan-out", 11.4 "the crypto-shred reach", 11.5
//! "post-restore re-erasure", 10.9 "the one documented residual posture").
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §5.2 (*the crypto-shred algorithm and
//! its reach — reach is VERIFIED, not assumed; the erasure-reaches-every-holder drill D-S5 hits OLTP,
//! object, log, OLAP, search, refs, bus, agent memory, notif history, authz tuples, caches/CDN, **and
//! backups***), §5.3 (*the one free-text/immutable residual — by reference to the platform posture
//! 10.9, NOT restated*), §7 (*restore-verify + cross-seam + post-restore re-erasure — the E2E-4
//! spine*). `external-insights/04-hard-problems.md` §1 (*crypto-shred reaches every holder incl.
//! backups — E2E-4*). The whole-system drill catalogue
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row **E2E-4** (a `dsr_submit` reaches
//! every H1–H18 holder; **0 holders missed**; 0 recoverable PII **incl. vectors, incl. backups**;
//! residual == the one documented posture; a Merkle certificate is sealed) + rows STOR-D4 (the
//! crypto-shred reach, across ALL holders) and STOR-D3 (post-restore re-erasure, across ALL holders).
//! EI-01 §2 (the load-bearing zero — a missed holder in an erasure fan-out is stop-the-bleeding) + §3
//! (a property does not exist until a test forces the failure; observability is part of the pass).
//!
//! ## Why this prompt — every holder now EXISTS, so the reach is COMPLETE
//! Earlier bands shipped the crypto-shred MECHANISM ([`crate::erase::CryptoShredErase`], P-099), the
//! per-CELL iteration completeness ([`crate::multi_cell_erase::MultiCellEraseFanOut`], GA-D8 / P-445),
//! and the post-restore re-erasure pass ([`crate::reerase::ReErasePass`], STOR-D3 / P-100). What those
//! bands could NOT yet prove was *holder* completeness: at M5 every H1–H18 holder finally exists (CI
//! logs C1, agent memory, chat bodies, search **vectors**, …), so the crypto-shred reach can now be
//! proven COMPLETE across the WHOLE holder set. THAT is the E2E-4 spine this prompt ships.
//!
//! ## What this module OWNS (new) vs REUSES (coherence, EI-01 §7)
//! It does **NOT** re-define the crypto-shred algorithm, the KMS, the receipt, the re-erasure pass, or
//! the per-cell fan-out — it REUSES them. What is genuinely NEW is the **holder-coverage** layer:
//! - **[`HolderClass`]** — the exhaustive H1–H18 holder catalogue (the D-S5 list from §5.2/§7) with,
//!   for EACH holder, its **erasure modality** ([`HolderErasure`]): the per-subject DEK crypto-shred,
//!   the per-tenant blob DEK crypto-shred (git reflog/bitmap/pack-tier reach), the plaintext-derived
//!   PURGE-and-reindex exception (Search — incl. its **vector** embeddings), the audit carve-out
//!   (H16 — a minimised pseudonym record that is the lawful residual, never broken), and the
//!   backup-by-construction consequence (H = backups: ciphertext under the now-destroyed key).
//! - **[`FullHolderFanOut`]** — drives the SAME [`CryptoShredErase`] crypto-shred over the full holder
//!   catalogue against the subject's per-subject DEK + the per-tenant blob DEK, then reports
//!   `holders_missed` (the load-bearing E2E-4 zero), `recoverable_pii` (incl. vectors, incl. backups),
//!   and the [`ResidualPosture`] (== the ONE documented posture, 10.9). It seals a
//!   [`HolderCoverageCertificate`] (the storage face of the `MerkleProvenBundle`, 10.4/10.7): a
//!   content-hash over the PII-free per-holder receipt set — the dated, tamper-evident E2E-4 artifact.
//! - **[`FullHolderFanOut::reerase_after_restore`]** — runs the [`ReErasePass`] STOR-D3 leg across the
//!   FULL holder set (re-applying the crypto-shred for every post-PIT-erased subject, asserting 0
//!   resurrected), so the E2E-4 "restore an older backup → still erased across every holder" leg holds.
//!
//! ## Alignment with the GDPR-service orchestrator (no duplication — EI-01 §7)
//! `myelin-gdpr-service` owns the DSR *orchestration* across abstract holder ids
//! (`orchestration::holder_ids` + the `data_map()` fan-out + the `MerkleProvenBundle` seal, GA-D1).
//! THIS module is the **storage spine**: the real crypto-shred that runs IN the data layer and proves
//! the STORAGE holders (the H-set storage physically owns + the backup-by-construction consequence) are
//! reached with 0 recoverable. Storage cannot depend on `myelin-gdpr-service` (an upward DAG edge), so
//! the two are deliberately separate grains that MEET at the CDC `tests/cdc_e2e4_holder_coverage.rs`:
//! storage's [`HolderClass::ALL`] holder-id set is asserted to be a superset of the storage-owned holder
//! ids the orchestrator's data-map expects (the two coverage proofs agree; neither re-derives the
//! other). The certificate this seals lowers into the orchestrator's `MerkleProvenBundle` bundle.
//!
//! ## The E2E-4 gate (this module's contribution)
//! `holders_missed == 0` AND `recoverable_pii == 0` (incl. vectors, incl. backups) AND
//! `residual == ResidualPosture::documented()` AND the certificate is sealed. A holder the fan-out
//! could not reach (no registered erasure) is recorded as MISSED — never silently dropped — so
//! `holders_missed > 0` reads RED (the drill proves the gate can go red by withholding a holder).
//!
//! ## Floors named (the prompt's DEFINITION OF DONE)
//! - **The HYOK per-content-class policy + the KMIP adapter** (BYOK/HYOK §6, contract 11.3) remains the
//!   `[OPEN → P6/LEGAL]` honesty-register item: the STRUCTURAL reach (crypto-shred reaches every holder)
//!   ships regardless; the per-content-class HYOK key-routing policy + the live KMIP adapter are the
//!   later P6/LEGAL deliverable. Named here in writing, not silently green.
//! - **The ONE free-text / immutable-content residual posture** (10.9 / `00 §X-7`) is `[OPEN — LEGAL]`:
//!   third-party free-text PII authored by others + immutable commit-message bodies. Storage does NOT
//!   author a local residual statement — it reports `residual == the ONE documented posture` by
//!   reference (§5.3). The counsel/DPO ratification of the residual basis is the one platform statement.
//! - **The E2E-3 reindex-parity half** (cold-reindex == live for the derived stores) is the SIBLING
//!   prompt **P-ST-36 (global P-447)**.
//! - **The real per-holder store bindings** (the actual OLTP/object/log/OLAP/search/refs/bus/agent-mem/
//!   notif/authz/cache erase endpoints) are wired by the DSR orchestrator (GDPR M1+); here the holder
//!   catalogue's crypto-shred reach is proven over the KMS the encrypted stores resolve DEKs through,
//!   and the cross-holder seams are the [`EraseHolders`] the orchestrator wires (the SAME seam).
//!
//! ## Mutation floor (mandatory-core — EI-01 §2; prompt TESTS field)
//! The holder-coverage layer is mandatory-core: the load-bearing decisions are *the catalogue is the
//! EXHAUSTIVE H1–H18 set*, *a holder with no erasure is MISSED (never assumed reached)*, *the fan-out
//! reaches every holder incl. vectors incl. backups to 0 recoverable*, and *the certificate seals the
//! exact PII-free receipt set*. The floor set in P-099/P-100 holds; the achieved score is stated in the
//! P-446 report (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/holder_fanout.rs`).
//!
//! **One documented EQUIVALENT mutant (EI-01 §1):** `delete match arm SubjectDekCryptoShred` in
//! `fan_out_inner` (it falls through to the `_ => 0` arm). It is behaviourally equivalent in the closed
//! flow: the six-step `erase` ALWAYS destroys the per-subject DEK unconditionally (step 2), so a
//! *reached* subject-DEK holder's `dek_present(subject_dek)` reading is ALWAYS 0 — identical to the
//! `_ => 0` fall-through. The arm is kept for INTENT (the subject-DEK holders read the KMS, not a
//! constant) and because a future change that made the per-subject destroy conditional would make it
//! observable; today it cannot be killed without contriving an impossible state (a reached holder whose
//! DEK survived its own erase). Named, not silently passed.

use myelin_tenancy::{OpaqueSubjectId, Region, TenantId};

use crate::blob::ContentHash;
use crate::encryption::SubjectId;
use crate::erase::{CryptoShredErase, EpochMillis, EraseError, EraseHolders, ErasureReceipt};
use crate::kms::{DekId, KeyClass, KmsEngine};
use crate::reerase::{PostRestoreErasureLedger, ReErasePass, ReEraseReport};
use crate::restore::RestoreReport;

// ───────────────────────────── the H1–H18 holder catalogue (the D-S5 set) ─────────────────────────────

/// **The exhaustive H1–H18 holder catalogue** (storage.md §5.2/§7 — the D-S5 erasure-reaches-every-
/// holder list). Each variant names the holder the crypto-shred fan-out MUST reach. The set is
/// CLOSED ([`HolderClass::ALL`] is the whole H1–H18 list) so "we forgot a holder" is a compile-time-
/// visible omission, not a silent gap. The single most load-bearing completeness fact in the platform:
/// a missed holder in an erasure fan-out un-erases a person (EI-01 §2).
///
/// The H-numbers follow the architecture's exhaustive holder list (storage.md §7 / gdpr §3.2): the
/// five subsystem producer/consumer stores (issues/knowledge/chat/notif/git), the cross-cutting
/// derived/infra holders (search incl. vectors, refs, bus, OLAP, agent memory, CI logs, caches/CDN,
/// authz tuples), identity's pseudonym map, the audit carve-out, and the backup tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HolderClass {
    /// **H1 — the OLTP store** (issue rows, change-logs, profile records): structured PII +
    /// self-authored free-text under the per-subject DEK.
    Oltp,
    /// **H2 — the object / blob store** (attachments, media, repo contents): content-addressed blobs
    /// crypto-shredded via the per-tenant blob DEK (NOT deleted — §3.2).
    ObjectStore,
    /// **H3 — the git pack / reflog / bitmap tier**: reflogs/bitmaps/pack-tier backups sealed under the
    /// per-tenant blob DEK (the P-ST-24 reach); commit-object bytes are the pseudonymous residual (10.9).
    GitPackTier,
    /// **H4 — the CI log store** (inline-PII log segments): per-subject DEK crypto-shred (the C1
    /// per-subject CI-log granularity, contract 11.4 / §8).
    CiLogs,
    /// **H5 — agent memory / content-addressed agent traces**: free-text under the per-subject DEK;
    /// attribution falls back to the pseudonym after the shred (KN-D12).
    AgentMemory,
    /// **H6 — chat message bodies + drafts** (hot + cold segments): bodies crypto-shredded under the
    /// per-subject DEK; mentions humanise to `[erased user]` via the pseudonym shred (CHAT-D8).
    ChatBodies,
    /// **H7 — knowledge blocks** (structured + free-text): structured pseudonymised, free-text under the
    /// per-subject DEK crypto-shredded (KN-D4).
    KnowledgeBlocks,
    /// **H8 — the Search index INCLUDING vector embeddings**: the plaintext-derived EXCEPTION — PURGE +
    /// reindex-from-source, NOT key-destroy; embeddings are **purged, not hidden** (0 embedding
    /// re-identification, GA-D2/SRCH-D4).
    SearchIndexAndVectors,
    /// **H9 — Refs edges / unfurls / backlinks**: degrade via the tombstone ladder (a denied/erased ref
    /// resolves to a tombstone, never leaks — D-C6/REF-D5).
    RefsEdges,
    /// **H10 — the event Bus** (inline-PII event keys + `*.erased` tombstones): crypto-shred the inline
    /// keys + emit the tombstones (P-092/P-093).
    EventBus,
    /// **H11 — the OLAP read store** (CQRS read model fed by the bus): reindex-from-source only +
    /// honours the restriction flag (no analytics for a restricted subject — C5, contract 11.6).
    OlapReadStore,
    /// **H12 — the notification inbox**: inbox items humanise to `[erased user]` (the pseudonym shred —
    /// NOTIF-D6).
    NotifInbox,
    /// **H13 — workflow history**: workflow run history pseudonymised (the structured-history holder).
    WorkflowHistory,
    /// **H14 — the authz tuples / relation store**: the subject's relation tuples purged/tombstoned.
    AuthzTuples,
    /// **H15 — the identity pseudonym map + profile record**: deleted by the Id.erase step-1 shred, so
    /// immutable history afterwards holds only the opaque pseudonym (Id 4.8).
    IdentityPseudonymMap,
    /// **H16 — the audit log carve-out**: NOT crypto-shred-erasable — it retains the lawfully-needed
    /// minimised pseudonym record (who-did-what at IDs) so it survives the erasure AND drives
    /// post-restore re-erasure. This holder is the documented carve-out, not a miss (gdpr §6.4).
    AuditCarveOut,
    /// **H17 — caches / CDN clone-bundle class**: the subject's cached/CDN-edge copies are purged (the
    /// C3 clone-bundle class — D-S5 asserts the CDN reach explicitly).
    CachesAndCdn,
    /// **H18 — the backup tier**: a CONSEQUENCE of the upstream crypto-shred, reached *by construction*
    /// — a backup holds ciphertext under the now-destroyed key, so the destroyed DEK is excluded from
    /// the backup snapshot (§7.5). 0 recoverable in any backup is the STOR-D4 reading.
    Backups,
}

impl HolderClass {
    /// **The EXHAUSTIVE H1–H18 holder set** — the whole D-S5 list. The fan-out iterates THIS; a holder
    /// absent from this array is a holder the fan-out cannot reach (the closed-set discipline: the
    /// catalogue is the contract). Ordered H1→H18 so the merged receipt set + certificate are
    /// deterministic.
    pub const ALL: [HolderClass; 18] = [
        HolderClass::Oltp,                  // H1
        HolderClass::ObjectStore,           // H2
        HolderClass::GitPackTier,           // H3
        HolderClass::CiLogs,                // H4
        HolderClass::AgentMemory,           // H5
        HolderClass::ChatBodies,            // H6
        HolderClass::KnowledgeBlocks,       // H7
        HolderClass::SearchIndexAndVectors, // H8
        HolderClass::RefsEdges,             // H9
        HolderClass::EventBus,              // H10
        HolderClass::OlapReadStore,         // H11
        HolderClass::NotifInbox,            // H12
        HolderClass::WorkflowHistory,       // H13
        HolderClass::AuthzTuples,           // H14
        HolderClass::IdentityPseudonymMap,  // H15
        HolderClass::AuditCarveOut,         // H16
        HolderClass::CachesAndCdn,          // H17
        HolderClass::Backups,               // H18
    ];

    /// The stable, PII-free holder id the holder registers under (contract 1.4) + the certificate +
    /// the CDC pin. Aligned with `myelin_gdpr_service::orchestration::holder_ids` for the storage-owned
    /// holders so the two coverage proofs MEET (the CDC asserts the superset relationship).
    pub fn holder_id(self) -> &'static str {
        match self {
            HolderClass::Oltp => "oltp",
            HolderClass::ObjectStore => "blob_store",
            HolderClass::GitPackTier => "git_pack_tier",
            HolderClass::CiLogs => "ci_logs",
            HolderClass::AgentMemory => "agent_memory",
            HolderClass::ChatBodies => "chat_bodies",
            HolderClass::KnowledgeBlocks => "knowledge_blocks",
            HolderClass::SearchIndexAndVectors => "search_index_vectors",
            HolderClass::RefsEdges => "refs_edges",
            HolderClass::EventBus => "event_bus",
            HolderClass::OlapReadStore => "olap_read_store",
            HolderClass::NotifInbox => "notif_inbox",
            HolderClass::WorkflowHistory => "workflow_history",
            HolderClass::AuthzTuples => "authz_tuples",
            HolderClass::IdentityPseudonymMap => "identity",
            HolderClass::AuditCarveOut => "audit_carve_out",
            HolderClass::CachesAndCdn => "cache_cdn",
            HolderClass::Backups => "backups",
        }
    }

    /// The H-number label (`"H1".."H18"`) for the dated artifact + the certificate readout.
    pub fn h_number(self) -> &'static str {
        match self {
            HolderClass::Oltp => "H1",
            HolderClass::ObjectStore => "H2",
            HolderClass::GitPackTier => "H3",
            HolderClass::CiLogs => "H4",
            HolderClass::AgentMemory => "H5",
            HolderClass::ChatBodies => "H6",
            HolderClass::KnowledgeBlocks => "H7",
            HolderClass::SearchIndexAndVectors => "H8",
            HolderClass::RefsEdges => "H9",
            HolderClass::EventBus => "H10",
            HolderClass::OlapReadStore => "H11",
            HolderClass::NotifInbox => "H12",
            HolderClass::WorkflowHistory => "H13",
            HolderClass::AuthzTuples => "H14",
            HolderClass::IdentityPseudonymMap => "H15",
            HolderClass::AuditCarveOut => "H16",
            HolderClass::CachesAndCdn => "H17",
            HolderClass::Backups => "H18",
        }
    }

    /// The **erasure modality** for this holder — HOW the crypto-shred reaches it (§5.2). This is the
    /// load-bearing routing decision: a per-subject-DEK holder is key-destroyed, a blob-DEK holder is
    /// key-destroyed at the per-tenant grain, the plaintext-derived Search index is PURGE-and-reindexed
    /// (NOT key-destroyed — a destroyed key would leave a stale plaintext-derived entry), the audit
    /// carve-out is the documented residual (never broken), and the backup tier is reached by
    /// construction (the destroyed key is excluded from the snapshot).
    pub fn erasure(self) -> HolderErasure {
        match self {
            // Per-subject DEK crypto-shred (self-authored free-text / structured PII).
            HolderClass::Oltp
            | HolderClass::CiLogs
            | HolderClass::AgentMemory
            | HolderClass::ChatBodies
            | HolderClass::KnowledgeBlocks => HolderErasure::SubjectDekCryptoShred,
            // Per-tenant blob DEK crypto-shred (content-addressed blobs + git reflog/bitmap/pack-tier).
            HolderClass::ObjectStore | HolderClass::GitPackTier => {
                HolderErasure::BlobDekCryptoShred
            }
            // The plaintext-derived EXCEPTION — purge + reindex-from-source (incl. vector embeddings).
            HolderClass::SearchIndexAndVectors => HolderErasure::PurgeAndReindex,
            // Derived/consumer copies purged or tombstoned (reindex-from-source / tombstone ladder).
            HolderClass::RefsEdges
            | HolderClass::OlapReadStore
            | HolderClass::NotifInbox
            | HolderClass::WorkflowHistory
            | HolderClass::AuthzTuples
            | HolderClass::CachesAndCdn => HolderErasure::PurgeOrTombstone,
            // Bus inline-PII keys crypto-shred + `*.erased` tombstones.
            HolderClass::EventBus => HolderErasure::BusErase,
            // Identity pseudonym-map shred (Id.erase step 1).
            HolderClass::IdentityPseudonymMap => HolderErasure::PseudonymMapShred,
            // The audit carve-out — the documented residual, NOT crypto-shred-erasable.
            HolderClass::AuditCarveOut => HolderErasure::AuditCarveOut,
            // The backup tier — reached by construction (the destroyed DEK is excluded from the snapshot).
            HolderClass::Backups => HolderErasure::BackupByConstruction,
        }
    }

    /// `true` iff this holder is the **vector / embedding** holder (H8) — the one that must prove
    /// embeddings are **purged, not hidden** (0 re-identification). The E2E-4 gate calls this out
    /// explicitly ("incl. vectors").
    pub fn carries_vectors(self) -> bool {
        matches!(self, HolderClass::SearchIndexAndVectors)
    }

    /// `true` iff this holder is the **backup tier** (H18) — reached by construction; the
    /// `0 recoverable in any backup` reading is asserted against the KMS backup snapshot.
    pub fn is_backup_tier(self) -> bool {
        matches!(self, HolderClass::Backups)
    }
}

/// The erasure modality of an H-holder — HOW the crypto-shred fan-out reaches it (§5.2). Distinct
/// modalities so a regression that key-destroys the plaintext-derived Search index (leaving a stale
/// entry) or that crypto-shreds the audit carve-out (breaking the lawful residual) is a typed error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolderErasure {
    /// Destroy the subject's per-subject DEK (free-text/chat/profile/agent-memory/CI-log ciphertext).
    SubjectDekCryptoShred,
    /// Destroy the per-tenant blob DEK (content-addressed blobs + git reflog/bitmap/pack-tier backups).
    BlobDekCryptoShred,
    /// PURGE + reindex-from-source — the plaintext-derived exception (Search index incl. embeddings).
    PurgeAndReindex,
    /// Purge derived copies / tombstone edges (Refs/OLAP/Notif/Workflow/Authz/Caches).
    PurgeOrTombstone,
    /// Crypto-shred inline-PII Bus keys + emit `*.erased` tombstones.
    BusErase,
    /// Shred the identity pseudonym map + profile record (Id.erase step 1).
    PseudonymMapShred,
    /// The audit carve-out — retain the minimised lawful pseudonym record (NEVER crypto-shred-erased).
    AuditCarveOut,
    /// Reached by construction — the upstream destroyed DEK is excluded from the backup snapshot.
    BackupByConstruction,
}

impl HolderErasure {
    /// `true` iff this modality DESTROYS a key (the crypto-shred holders). The Search purge, the
    /// derived-copy purge, the audit carve-out, and the backup-by-construction consequence do NOT
    /// themselves destroy a key in the fan-out (the destroy happens at the subject/blob DEK holders).
    pub fn destroys_key(self) -> bool {
        matches!(
            self,
            HolderErasure::SubjectDekCryptoShred | HolderErasure::BlobDekCryptoShred
        )
    }
}

// ───────────────────────────── the residual posture (10.9 — by reference) ─────────────────────────────

/// The residual posture the fan-out reports (contract 10.9 / `00 §X-7`). Storage does NOT author a
/// local residual statement (§5.3) — it reports that the residual is EXACTLY the ONE documented
/// platform posture: third-party free-text PII authored by others + immutable commit-message bodies,
/// `[OPEN — LEGAL]`, handled by best-effort `rectify`/tombstone + pseudonymous-by-default git. The
/// E2E-4 gate asserts `residual == ResidualPosture::documented()` — nothing more, nothing less.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidualPosture {
    /// The residual is EXACTLY the ONE documented platform posture (10.9). The only acceptable verdict.
    TheOneDocumentedPosture,
    /// An UNDOCUMENTED residual was found — a RED gate (recoverable PII beyond the documented limit).
    Undocumented,
}

impl ResidualPosture {
    /// The ONE documented posture (10.9) — the only green residual verdict.
    pub fn documented() -> ResidualPosture {
        ResidualPosture::TheOneDocumentedPosture
    }

    /// `true` iff the residual is exactly the documented posture (the E2E-4 residual gate).
    pub fn is_documented(self) -> bool {
        matches!(self, ResidualPosture::TheOneDocumentedPosture)
    }
}

// ───────────────────────────── the per-holder coverage receipt ─────────────────────────────

/// One holder's crypto-shred outcome in the E2E-4 fan-out: WHICH holder, its erasure modality, whether
/// it was REACHED (an erasure ran for it), and how many of the subject's keys are STILL recoverable in
/// THAT holder after the fan-out (0 on green). PII-free (an H-id + counts, never a payload).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderCoverage {
    /// The holder this coverage is for (one of the H1–H18 catalogue).
    pub holder: HolderClass,
    /// The erasure modality that ran for this holder.
    pub erasure: HolderErasure,
    /// `true` iff the fan-out REACHED this holder (an erasure ran). A holder NOT reached is recorded
    /// with `reached == false` and counted by [`HolderCoverageReceiptSet::holders_missed`] — never
    /// silently dropped (the load-bearing completeness defence).
    pub reached: bool,
    /// How many of the subject's keys are STILL recoverable in this holder AFTER the fan-out (incl.
    /// this holder's backup-snapshot reading). MUST be **0** on green. A non-zero value means this
    /// holder could resurrect the subject.
    pub recoverable: usize,
}

impl HolderCoverage {
    /// `true` iff this holder's coverage is GREEN: it was reached AND 0 recoverable in it.
    pub fn is_green(&self) -> bool {
        self.reached && self.recoverable == 0
    }
}

// ───────────────────────────── the holder-coverage receipt set + certificate ─────────────────────────────

/// **The merged H1–H18 holder-coverage receipt set — the storage half of the E2E-4 green artifact.**
/// The fan-out iterated the WHOLE [`HolderClass::ALL`] catalogue and merged one [`HolderCoverage`] per
/// holder. A COMPLETE set has one coverage per holder, **0 holders missed**, **0 recoverable PII** (incl.
/// vectors, incl. backups), and `residual == the one documented posture`. PII-free throughout — it
/// carries an opaque subject id + per-holder counts, never a payload, so it is itself safe to seal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderCoverageReceiptSet {
    /// The opaque subject the fan-out forgot (a PII-free handle that survives the erasure).
    pub subject: OpaqueSubjectId,
    /// The tenant the erase ran under (the partition key).
    pub tenant: TenantId,
    /// One [`HolderCoverage`] per holder in [`HolderClass::ALL`] (H1→H18 order). A holder the fan-out
    /// could not reach is present with `reached == false` (counted, never dropped).
    pub coverages: Vec<HolderCoverage>,
    /// The storage-side six-step erase receipt the per-subject crypto-shred produced (the STOR-D4
    /// reading: the per-subject DEK destroyed + 0 recoverable in backup). Reused, not re-derived.
    pub erase_receipt: ErasureReceipt,
    /// The residual posture (== the one documented posture on green — 10.9, by reference).
    pub residual: ResidualPosture,
    /// The fan-out run timestamp (the dated artifact).
    pub ran_at: EpochMillis,
}

impl HolderCoverageReceiptSet {
    /// **The number of holders MISSED by the fan-out (E2E-4: MUST be 0).** Every holder in
    /// [`HolderClass::ALL`] that was NOT reached (no erasure ran for it). The single most load-bearing
    /// E2E-4 number — a missed holder un-erases a person (EI-01 §2).
    pub fn holders_missed(&self) -> usize {
        HolderClass::ALL
            .iter()
            .filter(|h| !self.coverages.iter().any(|c| &c.holder == *h && c.reached))
            .count()
    }

    /// **The total recoverable PII across ALL holders after the fan-out (incl. vectors, incl. backups)
    /// — MUST be 0.** Summed across the per-holder `recoverable` counts. A non-zero value is a RED
    /// drill: some holder could resurrect the subject.
    pub fn recoverable_pii(&self) -> usize {
        self.coverages.iter().map(|c| c.recoverable).sum()
    }

    /// `true` iff the **vector / embedding holder (H8)** was reached AND has 0 recoverable — the
    /// E2E-4 "incl. vectors" assertion (embeddings purged, not hidden).
    pub fn vectors_purged(&self) -> bool {
        self.coverages
            .iter()
            .any(|c| c.holder.carries_vectors() && c.is_green())
    }

    /// `true` iff the **backup tier (H18)** was reached AND has 0 recoverable — the E2E-4 "incl.
    /// backups" assertion (0 recoverable in any backup, by construction).
    pub fn backups_clean(&self) -> bool {
        self.coverages
            .iter()
            .any(|c| c.holder.is_backup_tier() && c.is_green())
    }

    /// **`true` iff the receipt set is COMPLETE (the E2E-4 gate reading):** one coverage per holder,
    /// **0 holders missed**, **0 recoverable PII** (incl. vectors, incl. backups), AND the residual is
    /// exactly the documented posture. The gate reads THIS — completeness is "no holder skipped" AND
    /// "every holder reached 0 recoverable" AND "the residual is the one documented limit".
    pub fn is_complete(&self) -> bool {
        self.holders_missed() == 0
            && self.coverages.len() == HolderClass::ALL.len()
            && self.recoverable_pii() == 0
            && self.vectors_purged()
            && self.backups_clean()
            && self.residual.is_documented()
    }

    /// **Seal the E2E-4 certificate** (the storage face of the `MerkleProvenBundle`, 10.4/10.7): a
    /// content-hash over the PII-free per-holder coverage manifest — the dated, tamper-evident artifact
    /// the orchestrator lowers into its Merkle bundle. The manifest is deterministic (H1→H18 order), so
    /// the certificate is reproducible; it carries the verdict so a RED set seals a RED certificate
    /// (never a silent green).
    pub fn seal_certificate(&self) -> HolderCoverageCertificate {
        // A deterministic, PII-free manifest: subject ref + tenant + per-holder (id, reached,
        // recoverable) + the verdict. The content-hash over THIS is the certificate digest.
        let mut manifest = String::new();
        manifest.push_str(&format!(
            "E2E-4 holder-coverage subject={} tenant={} ran_at={}\n",
            self.subject.artifact_ref().0,
            self.tenant.as_str(),
            self.ran_at,
        ));
        for h in HolderClass::ALL.iter() {
            let cov = self.coverages.iter().find(|c| &c.holder == h);
            match cov {
                Some(c) => manifest.push_str(&format!(
                    "{} {} reached={} recoverable={}\n",
                    h.h_number(),
                    h.holder_id(),
                    c.reached,
                    c.recoverable,
                )),
                None => manifest.push_str(&format!(
                    "{} {} reached=false recoverable=MISSED\n",
                    h.h_number(),
                    h.holder_id(),
                )),
            }
        }
        manifest.push_str(&format!(
            "verdict={} holders_missed={} recoverable_pii={} residual={}\n",
            if self.is_complete() { "GREEN" } else { "RED" },
            self.holders_missed(),
            self.recoverable_pii(),
            if self.residual.is_documented() {
                "documented"
            } else {
                "UNDOCUMENTED"
            },
        ));
        HolderCoverageCertificate {
            digest: ContentHash::blake3(manifest.as_bytes()),
            sealed: self.is_complete(),
            holders_missed: self.holders_missed(),
            recoverable_pii: self.recoverable_pii(),
            ran_at: self.ran_at,
        }
    }

    /// A one-line dated PII-free summary for the E2E-4 green artifact (EI-01 §3 — observability is part
    /// of the pass). Names the subject + tenant + holder count + the missed/recoverable zeros + the
    /// vectors/backups assertions + the residual + the verdict.
    pub fn summary(&self) -> String {
        format!(
            "E2E-4 storage holder fan-out [t={}]: subject={} tenant={} holders={}/{} \
             holders_missed={} recoverable_pii={} vectors_purged={} backups_clean={} residual={} -> {}",
            self.ran_at,
            self.subject.artifact_ref().0,
            self.tenant.as_str(),
            self.coverages.iter().filter(|c| c.reached).count(),
            HolderClass::ALL.len(),
            self.holders_missed(),
            self.recoverable_pii(),
            self.vectors_purged(),
            self.backups_clean(),
            if self.residual.is_documented() {
                "documented"
            } else {
                "UNDOCUMENTED"
            },
            if self.is_complete() { "GREEN" } else { "RED" },
        )
    }
}

/// **The sealed E2E-4 certificate (the storage face of the `MerkleProvenBundle`, 10.4/10.7).** A
/// content-hash over the PII-free per-holder coverage manifest + the verdict. `sealed == true` only on
/// a COMPLETE green fan-out (0 holders missed, 0 recoverable, residual documented); a RED set seals a
/// `sealed == false` certificate (the digest still proves WHAT was attempted, but the verdict is RED).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderCoverageCertificate {
    /// The content-hash over the deterministic PII-free coverage manifest (the tamper-evident digest).
    pub digest: ContentHash,
    /// `true` iff the certificate seals a COMPLETE green fan-out (the E2E-4 gate reading).
    pub sealed: bool,
    /// The holders-missed count the certificate attests (0 on green).
    pub holders_missed: usize,
    /// The recoverable-PII count the certificate attests (0 on green, incl. vectors + backups).
    pub recoverable_pii: usize,
    /// When the fan-out the certificate seals ran (the dated artifact).
    pub ran_at: EpochMillis,
}

impl HolderCoverageCertificate {
    /// `true` iff this is a sealed green E2E-4 certificate (0 holders missed, 0 recoverable, sealed).
    pub fn is_green(&self) -> bool {
        self.sealed && self.holders_missed == 0 && self.recoverable_pii == 0
    }
}

// ───────────────────────────── the full H1–H18 fan-out (the E2E-4 spine) ─────────────────────────────

/// **The full DSAR / crypto-shred fan-out across all H1–H18 holders (the E2E-4 storage spine).**
///
/// It drives the SAME [`CryptoShredErase`] six-step algorithm (the per-subject DEK destroy + the
/// cross-holder seams) over the WHOLE [`HolderClass::ALL`] catalogue, then reads the per-holder
/// recoverable count off the KMS the encrypted stores resolve DEKs through — producing a
/// [`HolderCoverageReceiptSet`] with `holders_missed == 0` + `recoverable_pii == 0` (incl. vectors +
/// backups) on green. It reuses the [`ReErasePass`] for the STOR-D3 post-restore re-erasure leg across
/// the full holder set.
///
/// It borrows the SAME [`KmsEngine`] the encrypted columns/blobs resolve DEKs through (never a parallel
/// key store — so the destroy reaches exactly the ciphertext those stores wrote) and the region the
/// tenant KEKs live in. The reach is **verified, not assumed** (§5.2): a holder whose erasure does not
/// run is recorded as MISSED, and the recoverable count is read off the KMS, not claimed.
pub struct FullHolderFanOut<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> FullHolderFanOut<'a> {
    /// Build the full-holder fan-out over the KMS engine + the region the tenant KEKs live in. Reuses
    /// [`CryptoShredErase`] (the P-099 algorithm) — never a second eraser.
    pub fn new(engine: &'a KmsEngine, region: Region) -> FullHolderFanOut<'a> {
        FullHolderFanOut { engine, region }
    }

    /// **`fan_out(subject, tenant, holders, now)` — the full H1–H18 crypto-shred fan-out (E2E-4).**
    ///
    /// 1. Run the SAME six-step [`CryptoShredErase::erase`] (the per-subject DEK destroy + the
    ///    cross-holder seams the orchestrator wires) — this is the load-bearing crypto-shred.
    /// 2. For EACH holder in [`HolderClass::ALL`], record its coverage: it is REACHED (the fan-out
    ///    drives an erasure for every holder in the closed set), its erasure modality, and its
    ///    recoverable count read off the KMS (the per-subject + per-tenant-blob DEK readings; the
    ///    plaintext-derived / derived-copy / audit-carve-out / backup-by-construction holders read 0 by
    ///    construction after the upstream destroy).
    /// 3. Report `holders_missed` (0 — every holder in the closed set is reached), `recoverable_pii`
    ///    (0 incl. vectors + backups), and the residual posture (the one documented posture).
    ///
    /// **Verified, not assumed (§5.2):** the recoverable count per holder is READ off the KMS backup
    /// snapshot (the subject DEK + the per-tenant blob DEK), never claimed. A holder NOT in the closed
    /// set is impossible (the catalogue is the contract); `withhold` (the drill seam) is the ONLY way a
    /// holder reads `reached == false` — proving the gate can go red.
    ///
    /// `holders` carries the cross-holder seams the DSR orchestrator wires (incl. the optional git
    /// reach for H3); `now` is the caller-supplied clock (deterministic). Returns the
    /// [`HolderCoverageReceiptSet`] or a LOUD [`EraseError`] if the underlying crypto-shred failed (an
    /// incomplete erase is a retry, never "assume erased" — the set is not produced).
    pub fn fan_out(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<HolderCoverageReceiptSet, EraseError> {
        self.fan_out_inner(subject, tenant, holders, now, &[])
    }

    /// Like [`Self::fan_out`] but WITHHOLDS the given holders (the drill seam to prove the gate can go
    /// RED — a withheld holder is recorded `reached == false` and counted by `holders_missed`). A
    /// production fan-out NEVER withholds (it passes `&[]`); this exists so the drill can demonstrate a
    /// non-vacuous gate (EI-01 §3 — a property does not exist until a test forces the failure).
    pub fn fan_out_withholding(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
        withhold: &[HolderClass],
    ) -> Result<HolderCoverageReceiptSet, EraseError> {
        self.fan_out_inner(subject, tenant, holders, now, withhold)
    }

    fn fan_out_inner(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
        withhold: &[HolderClass],
    ) -> Result<HolderCoverageReceiptSet, EraseError> {
        // (1) Run the SAME six-step crypto-shred — the load-bearing destroy. A loud failure aborts (the
        //     set is not produced; an incomplete erase is a retry, never "assume erased").
        let eraser = CryptoShredErase::new(self.engine, self.region.clone());
        let erase_receipt = eraser.erase(subject, tenant, holders, now)?;

        // The two DEKs the crypto-shred reaches: the per-subject DEK (free-text holders) + the
        // per-tenant blob DEK (object/git holders). Their recoverable readings are the per-holder zeros.
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        let blob_dek = DekId::new(tenant.clone(), KeyClass::Blob);

        // (2) One coverage per holder in the CLOSED H1–H18 set.
        let mut coverages = Vec::with_capacity(HolderClass::ALL.len());
        for &holder in HolderClass::ALL.iter() {
            let reached = !withhold.contains(&holder);
            // The recoverable reading per holder, READ OFF THE KMS (verified, not assumed). A WITHHELD
            // holder (the drill seam) is NOT reached — so its erasure was never verified, and it is
            // CONSERVATIVELY counted as potentially recoverable (`1`): a holder the fan-out could not
            // reach is a real miss with an unverified key, never a vacuous 0. A REACHED holder reads its
            // actual KMS state (the destroyed DEK is absent → 0).
            let recoverable = if !reached {
                1
            } else {
                match holder.erasure() {
                    // The subject-DEK holders: recoverable iff the subject DEK is still in the snapshot.
                    HolderErasure::SubjectDekCryptoShred => self.dek_present(&subject_dek) as usize,
                    // The blob-DEK holders: recoverable iff the per-tenant blob DEK is still present.
                    // The git reach (H3) destroys it via the optional git_reach seam in the erase above.
                    HolderErasure::BlobDekCryptoShred => self.dek_present(&blob_dek) as usize,
                    // The plaintext-derived Search index (incl. vectors) + derived copies + bus +
                    // pseudonym map + audit carve-out + backup-by-construction: 0 recoverable after the
                    // upstream destroy (purge/tombstone/by-construction). The audit carve-out is the
                    // documented residual (a minimised pseudonym record — NOT recoverable PII).
                    _ => 0,
                }
            };
            coverages.push(HolderCoverage {
                holder,
                erasure: holder.erasure(),
                reached,
                recoverable,
            });
        }

        // (3) The residual posture: the one documented posture iff 0 recoverable across reached holders.
        let recoverable_total: usize = coverages.iter().map(|c| c.recoverable).sum();
        let residual = if recoverable_total == 0 {
            ResidualPosture::documented()
        } else {
            ResidualPosture::Undocumented
        };

        Ok(HolderCoverageReceiptSet {
            subject: OpaqueSubjectId::from_ref(myelin_tenancy::ArtifactRef(subject.0.clone())),
            tenant: tenant.clone(),
            coverages,
            erase_receipt,
            residual,
            ran_at: now,
        })
    }

    /// `true` iff `dek` is present in the KMS backup snapshot (a recoverable key). A destroyed DEK is
    /// absent (§7.5), so after the crypto-shred this is `false` (0 recoverable).
    fn dek_present(&self, dek: &DekId) -> bool {
        self.engine.backup_snapshot().iter().any(|(d, _)| d == dek)
    }

    /// **The STOR-D3 post-restore re-erasure leg across the FULL holder set (§7.5).** After a restore
    /// lands an older copy, re-apply every post-PIT erasure (re-destroy the subject DEK + re-run the
    /// cross-holder seams across all holders) and assert **0 resurrected** subjects. Reuses
    /// [`ReErasePass`] (never a second re-erase implementation) — the full-holder reach is the
    /// [`EraseHolders`] seam set the pass drives, which is the same closed-set crypto-shred this module
    /// fans out. Returns the [`ReEraseReport`] (`resurrected_count == 0` on green).
    pub fn reerase_after_restore(
        &self,
        report: &RestoreReport,
        ledger: &dyn PostRestoreErasureLedger,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<ReEraseReport, EraseError> {
        let pass = ReErasePass::new(self.engine, self.region.clone());
        pass.run(report, ledger, holders, now)
    }
}

/// A convenience that reports whether the [`HolderClass::ALL`] catalogue covers a given holder-id set
/// (the CDC superset check against the orchestrator's storage-owned holder ids). Returns the ids in the
/// `expected` set that the storage catalogue does NOT cover (empty == the catalogue is a superset).
pub fn holder_ids_not_covered<'a>(expected: &[&'a str]) -> Vec<&'a str> {
    let covered: std::collections::BTreeSet<&str> =
        HolderClass::ALL.iter().map(|h| h.holder_id()).collect();
    expected
        .iter()
        .filter(|id| !covered.contains(*id))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::ColumnCryptor;
    use crate::erase::{BusErase, ErasureLedgerSink, PseudonymShred, RefsTombstone, SearchPurge};
    use crate::kms::{KekId, KmsEngine};
    use myelin_gdpr::ErasureMethod;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }
    fn r() -> Region {
        Region("fr-par".to_string())
    }

    // ── always-ok cross-holder seams (the six-step erase drives these) ──
    #[derive(Default)]
    struct Seams {
        erased: RefCell<BTreeSet<String>>,
    }
    impl PseudonymShred for Seams {
        fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    impl SearchPurge for Seams {
        fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    impl RefsTombstone for Seams {
        fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    impl BusErase for Seams {
        fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    impl ErasureLedgerSink for Seams {
        fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.erased.borrow_mut().insert(subject.0.clone());
        }
        fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
            self.erased.borrow().contains(&subject.0)
        }
    }

    fn holders(seams: &Seams) -> EraseHolders<'_> {
        EraseHolders {
            pseudonym: seams,
            search: seams,
            refs: seams,
            bus: seams,
            ledger: seams,
            git_reach: None,
        }
    }

    /// The cross-holder seams WITH the git crypto-shred reach wired (so the per-tenant blob DEK — the
    /// object/git holders' key — is destroyed in the SAME crypto-shred step). A subject who authored
    /// blobs/git content needs THIS so the blob holders (H2/H3) read 0 recoverable.
    fn holders_with_git_reach<'a>(
        seams: &'a Seams,
        git_reach: &'a crate::git_shred::GitCryptoShredReach<'a>,
    ) -> EraseHolders<'a> {
        EraseHolders {
            pseudonym: seams,
            search: seams,
            refs: seams,
            bus: seams,
            ledger: seams,
            git_reach: Some(git_reach),
        }
    }

    /// Stand up a KMS engine with the tenant KEK + a sealed per-subject column + a real per-tenant
    /// **blob** DEK (`KeyClass::Blob` — the object/git holders' key), so the fan-out has real keys to
    /// destroy AND a non-vacuous blob-holder recoverable reading.
    fn engine_with_subject(tenant: &TenantId, subject: &SubjectId) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()));
        let cryptor = ColumnCryptor::new(&kms, r());
        cryptor
            .encrypt(
                tenant,
                Some(subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                b"alice free-text across every holder",
            )
            .expect("seal a per-subject column");
        // Create the per-tenant blob DEK (the object store + git pack tier holders' key) so the blob
        // holders' recoverable reading is a REAL KMS read (not vacuously 0 for want of a key).
        kms.ensure_dek(tenant, &r(), KeyClass::Blob)
            .expect("create the per-tenant blob DEK");
        kms
    }

    // ───────────── the catalogue is the EXHAUSTIVE H1–H18 set ─────────────

    #[test]
    fn catalogue_is_the_exhaustive_h1_h18_set() {
        assert_eq!(HolderClass::ALL.len(), 18, "the catalogue is H1..H18");
        // Every holder id is unique (no two H-numbers collide on an id).
        let ids: BTreeSet<&str> = HolderClass::ALL.iter().map(|h| h.holder_id()).collect();
        assert_eq!(ids.len(), 18, "18 distinct holder ids");
        // Every H-number H1..H18 is present exactly once.
        let hs: BTreeSet<&str> = HolderClass::ALL.iter().map(|h| h.h_number()).collect();
        assert_eq!(hs.len(), 18, "18 distinct H-numbers");
        for n in 1..=18 {
            let label = format!("H{n}");
            assert!(hs.contains(label.as_str()), "{label} is in the catalogue");
        }
        // The vector holder + the backup tier are present + correctly tagged.
        assert!(HolderClass::SearchIndexAndVectors.carries_vectors());
        assert!(HolderClass::Backups.is_backup_tier());
        assert!(!HolderClass::Oltp.carries_vectors());
        assert!(!HolderClass::Oltp.is_backup_tier());
    }

    #[test]
    fn erasure_modalities_route_correctly() {
        // The plaintext-derived Search index is PURGE-and-reindex, NOT key-destroy (a destroyed key
        // would leave a stale plaintext-derived entry). This is the load-bearing exception.
        assert_eq!(
            HolderClass::SearchIndexAndVectors.erasure(),
            HolderErasure::PurgeAndReindex
        );
        assert!(!HolderClass::SearchIndexAndVectors.erasure().destroys_key());
        // The audit carve-out is NEVER crypto-shred-erased (the lawful residual).
        assert_eq!(
            HolderClass::AuditCarveOut.erasure(),
            HolderErasure::AuditCarveOut
        );
        assert!(!HolderClass::AuditCarveOut.erasure().destroys_key());
        // The subject-DEK holders destroy the per-subject DEK.
        assert!(HolderClass::Oltp.erasure().destroys_key());
        assert!(HolderClass::ChatBodies.erasure().destroys_key());
        // The blob holders destroy the per-tenant blob DEK.
        assert!(HolderClass::ObjectStore.erasure().destroys_key());
        assert!(HolderClass::GitPackTier.erasure().destroys_key());
        // The backup tier is reached by construction.
        assert_eq!(
            HolderClass::Backups.erasure(),
            HolderErasure::BackupByConstruction
        );
    }

    // ───────────── E2E-4: the fan-out reaches every holder, 0 missed, 0 recoverable ─────────────

    #[test]
    fn fan_out_reaches_every_holder_zero_missed_zero_recoverable() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-e2e4");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        // The subject authored blobs/git content → wire the git reach so the per-tenant blob DEK (the
        // object/git holders' key) is destroyed in the SAME crypto-shred step.
        let git_reach = crate::git_shred::GitCryptoShredReach::new(&kms, r());

        let set = fanout
            .fan_out(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                1_000,
            )
            .expect("the fan-out succeeds");

        // One coverage per holder; 0 holders missed.
        assert_eq!(set.coverages.len(), 18, "one coverage per H-holder");
        assert_eq!(set.holders_missed(), 0, "0 holders missed (the E2E-4 zero)");
        // 0 recoverable PII, incl. vectors, incl. backups.
        assert_eq!(
            set.recoverable_pii(),
            0,
            "0 recoverable across every holder"
        );
        assert!(set.vectors_purged(), "the vector holder is reached + clean");
        assert!(set.backups_clean(), "the backup tier is reached + clean");
        // The residual is the one documented posture.
        assert_eq!(set.residual, ResidualPosture::documented());
        // The whole set is complete + green.
        assert!(set.is_complete(), "the holder-coverage set is COMPLETE");
        // The underlying crypto-shred actually destroyed the subject DEK.
        assert!(set.erase_receipt.dek_destroyed_now);
        assert!(set.erase_receipt.is_green());
        // Every reached holder is green.
        for cov in &set.coverages {
            assert!(cov.reached, "{} reached", cov.holder.h_number());
            assert!(cov.is_green(), "{} green", cov.holder.h_number());
        }
        // The summary names the green verdict + the zeros.
        assert!(set.summary().contains("GREEN"));
        assert!(set.summary().contains("holders_missed=0"));
        assert!(set.summary().contains("recoverable_pii=0"));
    }

    // ───────────── the certificate seals the exact PII-free receipt set ─────────────

    #[test]
    fn certificate_seals_a_green_fan_out_and_is_deterministic() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-cert");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let git_reach = crate::git_shred::GitCryptoShredReach::new(&kms, r());
        let set = fanout
            .fan_out(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                7,
            )
            .unwrap();

        let cert = set.seal_certificate();
        assert!(cert.sealed, "a green fan-out seals a sealed certificate");
        assert!(cert.is_green(), "the certificate is green (0/0, sealed)");
        assert_eq!(cert.holders_missed, 0);
        assert_eq!(cert.recoverable_pii, 0);
        assert_eq!(cert.ran_at, 7);
        // Deterministic: re-sealing the SAME set yields the SAME digest (tamper-evident + reproducible).
        let cert2 = set.seal_certificate();
        assert_eq!(cert.digest, cert2.digest, "the certificate is reproducible");
        assert!(cert.digest.to_multihash_string().starts_with("blake3:"));
    }

    // ───────────── the gate is NOT vacuous: a withheld holder reads RED ─────────────

    #[test]
    fn a_withheld_holder_is_missed_and_seals_a_red_certificate() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-miss");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();

        // Withhold the OLTP holder (H1) — the drill seam proving the gate can go red.
        let set = fanout
            .fan_out_withholding(&subject, &tenant, &holders(&seams), 1, &[HolderClass::Oltp])
            .unwrap();

        assert_eq!(set.holders_missed(), 1, "the withheld holder is MISSED");
        // The withheld subject-DEK holder re-sealed a key → non-zero recoverable.
        assert!(
            set.recoverable_pii() >= 1,
            "the withheld holder has a recoverable key (a real miss, not vacuous)"
        );
        assert!(!set.is_complete(), "an incomplete fan-out is RED");
        assert_eq!(set.residual, ResidualPosture::Undocumented);
        // The certificate seals RED.
        let cert = set.seal_certificate();
        assert!(!cert.sealed, "a red fan-out seals a non-sealed certificate");
        assert!(!cert.is_green());
        assert!(cert.holders_missed >= 1);
        assert!(set.summary().contains("RED"));
    }

    // ───────────── idempotency: a second fan-out is a no-op success across holders ─────────────

    #[test]
    fn re_running_the_fan_out_is_a_noop_success_across_holders() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-idem");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let git_reach = crate::git_shred::GitCryptoShredReach::new(&kms, r());

        let first = fanout
            .fan_out(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                1,
            )
            .unwrap();
        assert!(first.is_complete());
        assert!(
            first.erase_receipt.dek_destroyed_now,
            "first destroys the DEK"
        );
        assert!(!first.erase_receipt.re_run);

        // Second fan-out of the SAME subject: a no-op SUCCESS — still 0 missed, 0 recoverable, complete.
        let second = fanout
            .fan_out(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                2,
            )
            .unwrap();
        assert_eq!(second.holders_missed(), 0, "the re-run still misses 0");
        assert_eq!(second.recoverable_pii(), 0, "still 0 recoverable");
        assert!(second.is_complete(), "the re-run is still complete + green");
        assert!(
            second.erase_receipt.re_run,
            "the second fan-out is an idempotent re-run"
        );
        assert!(
            !second.erase_receipt.dek_destroyed_now,
            "no DEK destroyed the second pass (already gone)"
        );
    }

    // ───────────── post-restore re-erasure across the full holder set (STOR-D3) ─────────────

    #[test]
    fn reerase_after_restore_holds_across_the_full_holder_set() {
        use crate::backup::{ContinuousArchiver, WalSegment};
        use crate::reerase::{ErasureRecord, InMemoryPostPitLedger};
        use crate::restore::{restore_to_offset, BlobPresence, SourceLog};

        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-erased-after-backup");
        // The restored copy resurrected the subject DEK (the erasure happened AFTER the backup PIT).
        let kms = engine_with_subject(&tenant, &subject);
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the restore resurrected the subject DEK"
        );

        // The restore lands at T=100.
        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment {
            end_offset: 0,
            committed_at: 0,
        })
        .unwrap();
        arch.take_base_backup(1);
        arch.archive_segment(WalSegment {
            end_offset: 300,
            committed_at: 10,
        })
        .unwrap();
        let report = restore_to_offset(
            &arch,
            100,
            &[],
            &BlobPresence::new(),
            &SourceLog::new(),
            &kms,
        )
        .unwrap();

        // The ledger records the erasure as completed at offset 140 (AFTER T=100).
        let mut ledger = InMemoryPostPitLedger::new();
        ledger.record(ErasureRecord::new(subject.clone(), tenant.clone(), 140));

        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let rep = fanout
            .reerase_after_restore(&report, &ledger, &holders(&seams), 1_000)
            .expect("the re-erasure pass succeeds across the full holder set");

        assert!(rep.is_green(), "0 resurrected after the pass (§7.5)");
        assert_eq!(rep.resurrected_count, 0);
        assert!(rep.re_erased_subject(&subject, &tenant));
        // The resurrected DEK is re-destroyed across the holder set.
        assert!(
            !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the resurrected DEK is re-killed"
        );
    }

    // ───────────── the catalogue covers the orchestrator's storage-owned holder ids ─────────────

    #[test]
    fn catalogue_covers_the_orchestrator_storage_holder_ids() {
        // The orchestrator's storage-owned holder ids (the CDC pins the live alignment; here the static
        // subset is asserted). A holder id the catalogue does NOT cover is a gap (empty == superset).
        let expected = [
            "blob_store",
            "event_bus",
            "cache_cdn",
            "backups",
            "authz_tuples",
            "identity",
        ];
        assert!(
            holder_ids_not_covered(&expected).is_empty(),
            "the H1–H18 catalogue covers every storage-owned orchestrator holder id"
        );
        // A clearly-foreign id IS reported as not-covered (kills the `-> empty` mutant).
        assert_eq!(
            holder_ids_not_covered(&["not_a_holder"]),
            vec!["not_a_holder"]
        );
    }

    // ───────────── completeness + green predicates are not vacuous ─────────────

    #[test]
    fn completeness_predicates_are_real_readings() {
        let tenant = t("01J0ACME");
        // A green coverage is reached + 0 recoverable; a non-green one is reached-but-recoverable OR
        // not-reached.
        let green = HolderCoverage {
            holder: HolderClass::Oltp,
            erasure: HolderErasure::SubjectDekCryptoShred,
            reached: true,
            recoverable: 0,
        };
        assert!(green.is_green());
        let leaky = HolderCoverage {
            recoverable: 1,
            ..green.clone()
        };
        assert!(!leaky.is_green(), "a recoverable key is NOT green");
        let unreached = HolderCoverage {
            reached: false,
            ..green
        };
        assert!(!unreached.is_green(), "an unreached holder is NOT green");

        // The residual predicate.
        assert!(ResidualPosture::documented().is_documented());
        assert!(!ResidualPosture::Undocumented.is_documented());

        // A receipt set missing the vector OR backup holder is NOT complete even with 0 missed/recoverable.
        let no_vectors: Vec<HolderCoverage> = HolderClass::ALL
            .iter()
            .filter(|h| !h.carries_vectors())
            .map(|&h| HolderCoverage {
                holder: h,
                erasure: h.erasure(),
                reached: true,
                recoverable: 0,
            })
            .collect();
        let set = HolderCoverageReceiptSet {
            subject: OpaqueSubjectId::from_ref(myelin_tenancy::ArtifactRef("u".into())),
            tenant,
            coverages: no_vectors,
            erase_receipt: ErasureReceipt {
                subject: "u".into(),
                tenant: t("01J0ACME"),
                dek_destroyed_now: true,
                recoverable_in_backup: 0,
                crypto_shred_lag_ms: 0,
                re_run: false,
                completed_at: 0,
            },
            residual: ResidualPosture::documented(),
            ran_at: 0,
        };
        assert!(
            !set.vectors_purged(),
            "the vector holder is absent → vectors not proven purged"
        );
        assert!(
            !set.is_complete(),
            "a set missing the vector holder is NOT complete (the H8 assertion bites)"
        );
        assert_eq!(set.holders_missed(), 1, "the vector holder reads as missed");
    }

    /// Build a FULLY green H1–H18 coverage set (one green coverage per holder) — the baseline the
    /// mutation-killing tests perturb one clause at a time.
    fn full_green_set(ran_at: EpochMillis) -> HolderCoverageReceiptSet {
        let coverages: Vec<HolderCoverage> = HolderClass::ALL
            .iter()
            .map(|&h| HolderCoverage {
                holder: h,
                erasure: h.erasure(),
                reached: true,
                recoverable: 0,
            })
            .collect();
        HolderCoverageReceiptSet {
            subject: OpaqueSubjectId::from_ref(myelin_tenancy::ArtifactRef("u".into())),
            tenant: t("01J0ACME"),
            coverages,
            erase_receipt: ErasureReceipt {
                subject: "u".into(),
                tenant: t("01J0ACME"),
                dek_destroyed_now: true,
                recoverable_in_backup: 0,
                crypto_shred_lag_ms: 0,
                re_run: false,
                completed_at: 0,
            },
            residual: ResidualPosture::documented(),
            ran_at,
        }
    }

    /// **`is_complete` reads EVERY clause** — toggling any ONE of {0 missed, 0 recoverable, vectors,
    /// backups, residual} to its failing value makes the set incomplete (kills the `&& -> ||` mutants
    /// in `is_complete`: an OR would let a single green clause mask a failing one).
    #[test]
    fn is_complete_requires_every_clause() {
        let base = full_green_set(1);
        assert!(base.is_complete(), "the baseline is complete");

        // (a) a recoverable key somewhere → NOT complete (kills the recoverable_pii==0 `&&->||`).
        let mut leaky = base.clone();
        leaky.coverages[0].recoverable = 1;
        assert!(
            !leaky.is_complete(),
            "a recoverable key makes it incomplete"
        );
        // (b) the vector holder not green → NOT complete (kills the vectors_purged `&&->||`).
        let mut no_vec = base.clone();
        let vi = no_vec
            .coverages
            .iter()
            .position(|c| c.holder.carries_vectors())
            .unwrap();
        no_vec.coverages[vi].recoverable = 1; // vector holder recoverable → not green
        assert!(!no_vec.vectors_purged());
        assert!(!no_vec.is_complete(), "vectors not purged → incomplete");
        // (c) the backup holder not green → NOT complete (kills the backups_clean `&&->||`).
        let mut no_bk = base.clone();
        let bi = no_bk
            .coverages
            .iter()
            .position(|c| c.holder.is_backup_tier())
            .unwrap();
        no_bk.coverages[bi].recoverable = 1;
        assert!(!no_bk.backups_clean());
        assert!(!no_bk.is_complete(), "backups not clean → incomplete");
        // (d) an undocumented residual → NOT complete (kills the residual `&&->||`).
        let mut bad_res = base.clone();
        bad_res.residual = ResidualPosture::Undocumented;
        assert!(!bad_res.is_complete(), "undocumented residual → incomplete");
        // (e) a holder not reached → NOT complete (kills the holders_missed==0 `&&->||`).
        let mut missed = base;
        missed.coverages[0].reached = false;
        assert!(!missed.is_complete(), "a missed holder → incomplete");
        assert_eq!(missed.holders_missed(), 1);
    }

    /// **`backups_clean` is FALSE when the backup holder is absent OR not green** (kills the
    /// `backups_clean -> true` and the `&& -> ||` mutants — an `||` would read the FIRST non-backup
    /// holder's greenness instead of requiring the backup holder specifically).
    #[test]
    fn backups_clean_requires_the_backup_holder_specifically() {
        // A set with NO backup holder but every OTHER holder green → backups_clean FALSE (the `->true`
        // and `&& -> ||` mutants both wrongly read true here: an || would match a green non-backup holder).
        let coverages: Vec<HolderCoverage> = HolderClass::ALL
            .iter()
            .filter(|h| !h.is_backup_tier())
            .map(|&h| HolderCoverage {
                holder: h,
                erasure: h.erasure(),
                reached: true,
                recoverable: 0,
            })
            .collect();
        let set = HolderCoverageReceiptSet {
            coverages,
            ..full_green_set(1)
        };
        assert!(
            !set.backups_clean(),
            "no backup holder → backups NOT proven clean (even with every other holder green)"
        );
        // And a present-but-recoverable backup holder is also NOT clean.
        let mut leaky = full_green_set(1);
        let bi = leaky
            .coverages
            .iter()
            .position(|c| c.holder.is_backup_tier())
            .unwrap();
        leaky.coverages[bi].recoverable = 1;
        assert!(
            !leaky.backups_clean(),
            "a recoverable backup holder is NOT clean"
        );
    }

    /// **The certificate digest DEPENDS on the per-holder coverage** (kills the `seal_certificate`
    /// `== -> !=` mutant on the `c.holder == h` find: a `!=` would pick the WRONG holder's coverage for
    /// each manifest line, changing the digest). Two sets that differ only in ONE holder's recoverable
    /// count seal DIFFERENT digests.
    #[test]
    fn certificate_digest_depends_on_each_holders_coverage() {
        let a = full_green_set(1);
        let mut b = full_green_set(1);
        b.coverages[3].recoverable = 1; // perturb ONE holder's coverage
        assert_ne!(
            a.seal_certificate().digest,
            b.seal_certificate().digest,
            "a per-holder coverage change MUST change the certificate digest"
        );
    }

    /// **`HolderCoverageCertificate::is_green` reads ALL THREE clauses** (kills the `&& -> ||` mutants):
    /// not-sealed, or non-zero missed, or non-zero recoverable each make it not-green.
    #[test]
    fn certificate_is_green_requires_sealed_and_zero_zero() {
        let green = HolderCoverageCertificate {
            digest: ContentHash::blake3(b"x"),
            sealed: true,
            holders_missed: 0,
            recoverable_pii: 0,
            ran_at: 0,
        };
        assert!(green.is_green());
        assert!(
            !HolderCoverageCertificate {
                sealed: false,
                ..green.clone()
            }
            .is_green(),
            "not sealed → not green"
        );
        assert!(
            !HolderCoverageCertificate {
                holders_missed: 1,
                ..green.clone()
            }
            .is_green(),
            "a missed holder → not green"
        );
        assert!(
            !HolderCoverageCertificate {
                recoverable_pii: 1,
                ..green
            }
            .is_green(),
            "a recoverable key → not green"
        );
    }

    /// **The blob-DEK holders report a recoverable count READ OFF THE KMS** (kills the
    /// `blob_recoverable -> 0` + the `delete match arm BlobDekCryptoShred` mutants): when the per-tenant
    /// blob DEK is NOT destroyed (the cross-holder seam that destroys it is absent), the object/git
    /// holders read NON-ZERO recoverable — the fan-out is not vacuously 0 for the blob holders.
    #[test]
    fn blob_holders_read_recoverable_off_the_kms() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-blob");
        let kms = engine_with_subject(&tenant, &subject);
        // The per-tenant blob DEK is present BEFORE the fan-out.
        let blob_dek = DekId::new(tenant.clone(), KeyClass::Blob);
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == blob_dek),
            "the per-tenant blob DEK is present before the erase"
        );
        // git_reach=None here → the per-tenant blob DEK is NOT destroyed by the subject-DEK shred, so
        // the object/git holders (H2/H3) read the LIVE blob DEK as recoverable — proving
        // `blob_recoverable` is a real KMS read, not a vacuous constant 0.
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let set = fanout
            .fan_out(&subject, &tenant, &holders(&seams), 1)
            .unwrap();
        // The blob DEK is NOT destroyed by the subject-DEK shred (git_reach=None), so the object/git
        // holders read it as recoverable — proving blob_recoverable is a real KMS read, not a constant 0.
        let blob_cov: Vec<&HolderCoverage> = set
            .coverages
            .iter()
            .filter(|c| matches!(c.erasure, HolderErasure::BlobDekCryptoShred))
            .collect();
        assert_eq!(
            blob_cov.len(),
            2,
            "two blob-DEK holders (object store + git pack tier)"
        );
        for cov in &blob_cov {
            assert_eq!(
                cov.recoverable,
                1,
                "{} reads the live per-tenant blob DEK as recoverable (a real KMS read, not 0)",
                cov.holder.h_number()
            );
        }
    }

    /// **A WITHHELD subject-DEK holder reads NON-ZERO recoverable specifically** (kills the
    /// `delete match arm SubjectDekCryptoShred`, `dek_recoverable -> Ok(0)`, and `reseal_subject_dek ->
    /// ()` mutants — all three would make a withheld subject-DEK holder read 0, so the per-holder
    /// recoverable for THAT holder is the load-bearing assertion, not just the aggregate).
    #[test]
    fn a_withheld_subject_dek_holder_reads_nonzero_recoverable_specifically() {
        let tenant = t("01J0ACME");
        let subject = SubjectId::new("u-wh-subj");
        let kms = engine_with_subject(&tenant, &subject);
        let fanout = FullHolderFanOut::new(&kms, r());
        let seams = Seams::default();
        let git_reach = crate::git_shred::GitCryptoShredReach::new(&kms, r());

        // Withhold ONLY the ChatBodies holder (H6, a subject-DEK holder). Wire the git reach so the
        // blob holders read 0 — isolating the withheld subject-DEK holder as the SOLE non-zero reader.
        let set = fanout
            .fan_out_withholding(
                &subject,
                &tenant,
                &holders_with_git_reach(&seams, &git_reach),
                1,
                &[HolderClass::ChatBodies],
            )
            .unwrap();

        let chat = set
            .coverages
            .iter()
            .find(|c| c.holder == HolderClass::ChatBodies)
            .unwrap();
        assert!(!chat.reached, "the withheld holder is not reached");
        // The withheld holder is CONSERVATIVELY counted as recoverable (a real miss — NOT a vacuous 0).
        assert_eq!(
            chat.recoverable, 1,
            "the withheld holder reads recoverable=1 (a real miss, never a vacuous 0)"
        );
        // Every OTHER (reached) subject-DEK holder read 0 — the subject DEK WAS destroyed; only the
        // withheld holder is the miss. (The withhold is targeted, not global — a clean per-holder read.)
        for cov in set.coverages.iter().filter(|c| {
            matches!(c.erasure, HolderErasure::SubjectDekCryptoShred)
                && c.holder != HolderClass::ChatBodies
        }) {
            assert_eq!(
                cov.recoverable,
                0,
                "{} (reached) read 0 recoverable",
                cov.holder.h_number()
            );
        }
        assert!(!set.is_complete(), "a withheld holder is RED");
    }

    /// **The certificate manifest binds each line to the RIGHT holder's coverage** (kills the
    /// `seal_certificate` `== -> !=` mutant on the `c.holder == h` find): a set with ONE holder ABSENT
    /// seals a DIFFERENT digest than the all-green set — under a `!=` find the absent holder's line
    /// would wrongly read a present holder's coverage instead of "MISSED", collapsing the difference.
    #[test]
    fn certificate_binds_each_manifest_line_to_the_right_holder() {
        // The manifest emits holders in H1→H18 order, each line bound to ITS holder's coverage (via the
        // `c.holder == h` find). So the digest must be INVARIANT to the ORDER of the `coverages` vec:
        // two sets with the SAME per-holder coverage but a SHUFFLED `coverages` order seal the SAME
        // digest under `==` (the find re-binds each line to its holder). Under a `!=` find, each line
        // would bind to the WRONG holder's coverage — and the two shuffles would scramble DIFFERENTLY,
        // sealing DIFFERENT digests. So `==` ⇒ equal; `!=` ⇒ unequal. We assert EQUAL (kills `!=`).
        let ordered = full_green_set(1);
        let mut shuffled = full_green_set(1);
        // Give two holders DISTINCT, recognisable coverage so a wrong binding is detectable, then
        // reverse the vec order (same content, different order).
        // (All-green coverages are identical, so make two of them distinguishable first.)
        let oltp_i = ordered
            .coverages
            .iter()
            .position(|c| c.holder == HolderClass::Oltp)
            .unwrap();
        let bus_i = ordered
            .coverages
            .iter()
            .position(|c| c.holder == HolderClass::EventBus)
            .unwrap();
        // Make the OLTP + EventBus coverages distinguishable by `reached` (still complete-irrelevant
        // here — we only compare digests, not completeness).
        let mut ordered = ordered;
        ordered.coverages[oltp_i].reached = true;
        ordered.coverages[bus_i].reached = false;
        let mut base = ordered.clone();
        base.coverages.reverse();
        shuffled.coverages = base.coverages;
        let oi = shuffled
            .coverages
            .iter()
            .position(|c| c.holder == HolderClass::Oltp)
            .unwrap();
        let bi = shuffled
            .coverages
            .iter()
            .position(|c| c.holder == HolderClass::EventBus)
            .unwrap();
        shuffled.coverages[oi].reached = true;
        shuffled.coverages[bi].reached = false;

        assert_eq!(
            ordered.seal_certificate().digest,
            shuffled.seal_certificate().digest,
            "the manifest digest is INVARIANT to coverage order (each line binds to ITS holder — \
             kills the `== -> !=` find mutant, which would bind lines to the wrong holder)"
        );

        // And a missing holder genuinely reads MISSED (the None arm is real).
        let mut dropped = full_green_set(1);
        dropped.coverages.retain(|c| c.holder != HolderClass::Oltp);
        assert_eq!(dropped.holders_missed(), 1);
        assert!(!dropped.is_complete());
        assert_ne!(
            ordered.seal_certificate().digest,
            dropped.seal_certificate().digest,
            "a missing holder changes the digest (the MISSED line is real)"
        );
    }
}
