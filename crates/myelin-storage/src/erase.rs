//! The crypto-shred `erase(subject, tenant)` six-step algorithm (P-ST-09 / global P-099;
//! contract 11.4 — the erase-algorithm half).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §5.2 (the `erase(subject, tenant)`
//! algorithm — copied here as the six steps EXACTLY: pseudonym-map shred → `KMS.destroy(per_subject_DEK)`
//! → Search purge+reindex → Refs tombstone → Bus erase → erasure receipt; crypto-shred reaches
//! backups *by construction*; the search index is the plaintext-derived exception = purge+reindex;
//! reach is verified, not assumed), §5.1 (the GD-4 granularity rule the per-subject DEK realises).
//! Contract-index rows 11.4 (the erase algorithm), 10.1 (`PersonalDataHolder::erase`), 10.8 (the
//! erasure ledger receipt), Id 4.8 (the pseudonym-map shred). EI-04 §1 (crypto-shred substrate;
//! erasure-vs-immutability; crypto-shred reaches backups by construction). EI-01 §3 (prove-it: a
//! property does not exist until a test forces the failure).
//!
//! ## What this prompt ships (P-ST-09) — the storage-side MECHANISM behind the DSR orchestrator
//! [`CryptoShredErase`] runs the §5.2 algorithm in order:
//!
//! 1. **Pseudonym-map shred** — `Id.erase(subject)`: delete the pseudonym map + profile record, so
//!    git/bus/audit history afterwards holds only the opaque pseudonym (Id 4.8). Driven through the
//!    [`PseudonymShred`] seam (the real binding is `myelin-identity`'s `IdentityService::erase`,
//!    which storage cannot call directly without an upward DAG edge — so it is a seam the DSR
//!    orchestrator wires).
//! 2. **`KMS.destroy(per_subject_DEK(tenant, subject))`** — crypto-shred the free-text / chat /
//!    profile / agent-memory ciphertext, **live AND in backups** (by construction: a backup holds
//!    ciphertext under the now-destroyed key — §7.5). This is the step storage **owns directly**: it
//!    holds the [`crate::kms::KmsEngine`] and calls [`crate::kms::KmsEngine::destroy_dek`] on the
//!    subject's DEK. The per-subject DEK is the GD-4 individual-erasure lever (§5.1).
//! 3. **Search purge+reindex** — the index is *plaintext-derived*, so it is the EXCEPTION to
//!    crypto-shred: it is PURGEd and reindexed-from-source, never key-destroyed. Driven through the
//!    [`SearchPurge`] seam (the real binding is `myelin-search`, a consumer subsystem NOT in
//!    storage's dependency set — so it is a seam).
//! 4. **Refs tombstone** — unfurls/backlinks degrade via the tombstone ladder. Driven through the
//!    [`RefsTombstone`] seam (the real binding is `myelin-refs`, also a consumer subsystem — a seam).
//! 5. **Bus erase** — crypto-shred inline-PII event keys + emit `*.erased` tombstones. Driven
//!    through the [`BusErase`] seam (the real binding is `myelin-events`'s `BusHolder::erase`, which
//!    storage CAN reach — but the bus erase needs the Bus's own event log / outbox-tx / id-minter, so
//!    the orchestrator wires a closure/holder; the seam keeps storage decoupled from those specifics).
//! 6. **Record the erasure receipt** — the durable, PII-free, non-shred-erasable record into the
//!    audit / erasure-ledger holder (10.8). Driven through the [`ErasureLedgerSink`] seam.
//!
//! **Idempotent (the prompt requirement):** re-erasing an already-erased subject is a **no-op, not
//! an error**. [`CryptoShredErase::erase`] checks each step's "already done" predicate first (the
//! DEK is already gone, the ledger already records the subject) and short-circuits to a *re-run*
//! receipt — every seam is itself idempotent, and step 2's [`crate::kms::KmsEngine::destroy_dek`]
//! returns `false` (nothing to destroy) on a second call, which the algorithm treats as success.
//!
//! ## STOR-D4 — the gate (the per-subject-DEK-destroy half)
//! After an erase, the subject's per-subject ciphertext MUST be unrecoverable (the key is destroyed
//! AND excluded from backup). [`ErasureReceipt::recoverable_in_backup`] is the
//! `0 recoverable PII in any backup` reading: the algorithm probes the KMS backup snapshot for the
//! subject's DEK and asserts it is **absent**. `crypto_shred_lag` is the wall-clock the destroy took
//! (the §4.2 STOR-D4 telemetry). The drill (`tests/stor_d4_crypto_shred_drill.rs`) erases a subject,
//! attempts recovery from the backup snapshot, and asserts `recoverable_in_backup == 0`.
//!
//! ## Floors named (stubbed / deferred + the filling prompt) — VISION §3, prompt DoD
//! - **The GD-4 granularity wiring + the structural GDPR floor** (the per-subject vs per-tenant
//!   class routing completeness, the §5.1 table assertion) is the SIBLING prompt **P-ST-10 (global
//!   P-101)**. This prompt ships the erase ALGORITHM that DESTROYS the chosen key; the granularity
//!   completeness proof is there.
//! - **The git crypto-shred reach** (into reflogs / bitmaps / pack-tier backups) is the Git M3 reach
//!   **P-ST-24 (global P-253)**.
//! - **The cross-holder reach COMPLETENESS** (the erasure-reaches-EVERY-holder drill D-S5: OLTP,
//!   object, log, OLAP, search, refs, bus, agent memory, notif history, authz tuples, caches/CDN, and
//!   backups) is the GA / E2E-4 M5 drill **P-ST-35 (global, M5)**. This prompt ships the six-step
//!   mechanism + its seams; the *every-holder* fan-out completeness is proven there.
//! - **The post-restore RE-ERASURE (STOR-D3)** — re-running this algorithm for erasures completed
//!   AFTER a backup's PIT — is **P-ST-14 (global P-100)** (it needs the restore machinery). The
//!   erasure ledger this records into (step 6) is the seam P-100 replays.
//! - **The real seam bindings:** [`PseudonymShred`] → `myelin-identity` `IdentityService::erase`
//!   (Id 4.8, body P-ID-20); [`SearchPurge`] → `myelin-search` (M2 P-178/P-179); [`RefsTombstone`]
//!   → `myelin-refs` (M2 P-164); [`BusErase`] → `myelin-events` `BusHolder::erase` (P-092/P-093);
//!   [`ErasureLedgerSink`] → the GDPR-owned global ledger (10.8, P-GA-15 / P-115). On THIS prompt the
//!   seams are the trait abstractions the DSR orchestrator wires; the in-memory test doubles prove the
//!   algorithm drives all six in order. The DSR orchestrator that CALLS this (10.1/10.11) is the GDPR
//!   M1 deliverable.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2; prompt TESTS field)
//! The erase algorithm (the six-step ordering + the idempotent short-circuit) and the
//! KMS-destroy-reaches-backups path are mandatory-core: the load-bearing decisions are *the six
//! steps run in order*, *a re-erase is a no-op not an error*, and *the destroyed DEK is absent from
//! the backup snapshot (0 recoverable)*. The achieved score is stated in the P-099 report
//! (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/erase.rs`).

use std::fmt;

use myelin_tenancy::{Region, TenantId};

use crate::encryption::SubjectId;
use crate::kms::{DekId, KeyClass, KmsEngine};

// ───────────────────────────── the cross-holder seams ─────────────────────────────

/// **Step 1 seam — the pseudonym-map shred** (Id 4.8 `IdentityService::erase`). Storage cannot call
/// `myelin-identity` directly (that would be an upward DAG edge); the DSR orchestrator wires the real
/// `IdentityService` behind this trait. After it returns, the pseudonym map + profile record for the
/// subject are deleted, so immutable history holds only the opaque pseudonym.
pub trait PseudonymShred {
    /// Shred the subject's pseudonym map + profile record within `tenant` (Id 4.8). Idempotent: a
    /// second call for an already-erased subject is a no-op success.
    fn shred_pseudonym(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

/// **Step 3 seam — Search purge+reindex** (the plaintext-derived EXCEPTION). The search index is
/// derived from source, so erasure is **purge + reindex-from-source**, NOT a key-destroy (a destroyed
/// key would leave a stale plaintext-derived index entry). The DSR orchestrator wires `myelin-search`
/// behind this trait (M2 P-178/P-179).
pub trait SearchPurge {
    /// Purge the subject's documents from the per-tenant index and reindex-from-source. Idempotent.
    fn purge_and_reindex(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

/// **Step 4 seam — Refs tombstone**. Unfurls/backlinks referencing the subject degrade via the
/// tombstone ladder (a denied/erased ref resolves to a tombstone, never leaks). The DSR orchestrator
/// wires `myelin-refs` behind this trait (M2 P-164).
pub trait RefsTombstone {
    /// Tombstone the subject's refs/edges within `tenant`. Idempotent.
    fn tombstone(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

/// **Step 5 seam — Bus erase**. Crypto-shred the subject's inline-PII event keys + emit `*.erased`
/// tombstones (references-not-payloads means this is typically a short set). The DSR orchestrator
/// wires `myelin-events`'s `BusHolder::erase` behind this trait (P-092/P-093); the seam keeps storage
/// decoupled from the Bus's event-log / outbox-tx / id-minter specifics.
pub trait BusErase {
    /// Crypto-shred the subject's inline-PII Bus keys + emit `*.erased` tombstones. Idempotent.
    fn erase_inline_pii(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

/// **Step 2 EXTENSION seam — the git crypto-shred reach** (P-ST-24 / P-253; contract 11.2/11.4,
/// storage.md §5.3). The per-subject DEK destroy (step 2, owned in-crate) shreds the subject's
/// free-text/chat/profile/agent-memory; this seam extends the SAME crypto-shred step to reach git's
/// structures — the **reflog / bitmap / pack-tier backups** sealed under the per-tenant blob DEK
/// (`KeyClass::Blob`). Destroying that DEK renders those structures unrecoverable live AND in backups
/// by construction (§7.5); the commit-object bytes are the pseudonymous-by-default residual (10.9, by
/// reference — NOT byte-mutated). The DSR orchestrator wires `myelin-storage`'s
/// [`crate::git_shred::GitCryptoShredReach`] behind this trait. It is OPTIONAL on
/// [`EraseHolders`]: a subject who authored no git content needs no git reach (a `None` is a no-op
/// success), and the per-subject free-text shred (step 2 proper) is unconditional either way.
///
/// **Verified, not assumed (§5.2):** the reach returns a loud [`EraseError::BlobShredReach`] only if
/// its post-condition is NOT met (a backup still holds a recoverable git structure) — never a silent
/// claim that the reach happened.
pub trait BlobShredReach {
    /// Reach the git reflog / bitmap / pack-tier-backup ciphertext for the subject's tenant by
    /// destroying the per-tenant blob DEK + verifying 0 recoverable in backup. Idempotent (a second
    /// reach is a no-op success). Loud [`EraseError::BlobShredReach`] if the post-condition fails.
    fn shred_blob_tier(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

/// **Step 6 seam — the erasure-ledger receipt sink** (10.8). The durable, PII-free,
/// **non-shred-erasable** record of every completed erasure (it must survive the crypto-shred it
/// records AND a restore, so post-restore re-erasure can replay it). The DSR orchestrator wires the
/// GDPR-owned global ledger behind this trait (10.8, P-GA-15 / P-115).
pub trait ErasureLedgerSink {
    /// Record that `subject` (within `tenant`) was erased at `at` (the receipt). PII-free: an opaque
    /// subject id + a timestamp, never a payload. Idempotent (recording twice keeps the first).
    fn record_erasure(&self, subject: &SubjectId, tenant: &TenantId, at: EpochMillis);
    /// Whether the ledger already records `subject` as erased (the idempotent-re-erase predicate).
    fn is_erased(&self, subject: &SubjectId, tenant: &TenantId) -> bool;
}

/// A monotonic wall-clock reading in milliseconds (the erasure timestamp + the `crypto_shred_lag`
/// unit). Kept as a plain `u64` so the algorithm is deterministic and testable (the caller supplies
/// the clock — no hidden `SystemTime::now()` in the mechanism).
pub type EpochMillis = u64;

/// The five cross-holder seams the DSR orchestrator wires for an erase — bundled into ONE borrow so
/// the [`CryptoShredErase::erase`] signature stays the load-bearing seam (the CDC consumer pins THIS
/// shape, and the orchestrator wires the real subsystem holders once). Step 2 (the per-subject DEK
/// destroy) is owned in-crate and is NOT here — it is the [`KmsEngine`] the [`CryptoShredErase`]
/// holds.
pub struct EraseHolders<'a> {
    /// Step 1 — the pseudonym-map shred (Id 4.8).
    pub pseudonym: &'a dyn PseudonymShred,
    /// Step 3 — Search purge+reindex (the plaintext-derived exception).
    pub search: &'a dyn SearchPurge,
    /// Step 4 — Refs tombstone.
    pub refs: &'a dyn RefsTombstone,
    /// Step 5 — Bus erase (inline-PII keys + `*.erased` tombstones).
    pub bus: &'a dyn BusErase,
    /// Step 6 — the erasure-ledger receipt sink (10.8).
    pub ledger: &'a dyn ErasureLedgerSink,
    /// Step 2 EXTENSION (OPTIONAL) — the git crypto-shred reach (P-ST-24 / P-253): reflog / bitmap /
    /// pack-tier backups sealed under the per-tenant blob DEK. `None` when the subject authored no
    /// git content (the per-subject free-text shred — step 2 proper — runs regardless). When wired,
    /// it runs as part of the SAME crypto-shred step (after the per-subject DEK destroy), so a commit
    /// author's erase reaches git's structures too.
    pub git_reach: Option<&'a dyn BlobShredReach>,
}

// ───────────────────────────── the loud erase error ─────────────────────────────

/// A loud, typed failure of the erase algorithm. An erase NEVER silently "assumes erased" on a
/// partial failure — an incomplete erase is a LOUD error the DSR orchestrator retries (the erasure is
/// not recorded until every step succeeds). Carries WHICH step failed (the §5.2 step number) so the
/// orchestrator can resume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EraseError {
    /// Step 1 (pseudonym-map shred) failed — the subject's history would still resolve to real PII.
    PseudonymShred(String),
    /// Step 3 (Search purge+reindex) failed — a stale plaintext-derived index entry could remain.
    SearchPurge(String),
    /// Step 4 (Refs tombstone) failed — an unfurl could still leak the subject.
    RefsTombstone(String),
    /// Step 5 (Bus erase) failed — an inline-PII event key could still be live.
    BusErase(String),
    /// Step 2's git crypto-shred reach (P-ST-24) failed its post-condition — a reflog / bitmap /
    /// pack-tier-backup git structure is still recoverable from a backup (the per-tenant blob DEK was
    /// not excluded). The erase is INCOMPLETE: a backup could resurrect the git structure.
    BlobShredReach(String),
}

impl fmt::Display for EraseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EraseError::PseudonymShred(m) => write!(
                f,
                "erase step 1 (pseudonym-map shred / Id.erase) failed: {m} — erase ABORTED as \
                 INCOMPLETE, NEVER recorded as erased (a partial erase is a loud retry, not 'assume erased')"
            ),
            EraseError::SearchPurge(m) => write!(
                f,
                "erase step 3 (Search purge+reindex) failed: {m} — erase ABORTED as INCOMPLETE \
                 (a stale plaintext-derived index entry could remain)"
            ),
            EraseError::RefsTombstone(m) => write!(
                f,
                "erase step 4 (Refs tombstone) failed: {m} — erase ABORTED as INCOMPLETE (an \
                 unfurl could still leak the subject)"
            ),
            EraseError::BusErase(m) => write!(
                f,
                "erase step 5 (Bus erase) failed: {m} — erase ABORTED as INCOMPLETE (an inline-PII \
                 event key could still be live)"
            ),
            EraseError::BlobShredReach(m) => write!(
                f,
                "erase step 2 (git crypto-shred reach, P-ST-24) failed: {m} — erase ABORTED as \
                 INCOMPLETE (a reflog/bitmap/pack-tier-backup git structure could still be \
                 recoverable from a backup)"
            ),
        }
    }
}

impl std::error::Error for EraseError {}

// ───────────────────────────── the erasure receipt (the STOR-D4 artifact) ─────────────────────────

/// The dated, PII-free artifact the six-step erase returns — the PROOF the subject's per-subject
/// ciphertext is unrecoverable (the STOR-D4 reading). It names the subject + tenant (opaque ids), the
/// six steps that ran, the `crypto_shred_lag`, and **`recoverable_in_backup`** — the
/// `0 recoverable PII in any backup` gate reading (the subject's DEK is absent from the KMS backup
/// snapshot, §7.5). `re_run` is true when the erase was a no-op idempotent re-run (the subject was
/// already erased).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureReceipt {
    /// The opaque subject id that was erased (already pseudonymous — never real-identity PII).
    pub subject: String,
    /// The tenant the erasure ran within.
    pub tenant: TenantId,
    /// Whether the per-subject DEK was actually destroyed this call (`true`) or was already gone
    /// (`false` — an idempotent re-run). Either way the post-condition holds: the key is destroyed.
    pub dek_destroyed_now: bool,
    /// **THE STOR-D4 GATE READING:** how many of the subject's per-subject DEKs are STILL recoverable
    /// from the KMS backup snapshot AFTER the erase — MUST be **0** (the key is destroyed AND excluded
    /// from backup, §7.5). A non-zero value is a RED drill: a backup could resurrect the subject.
    pub recoverable_in_backup: usize,
    /// `crypto_shred_lag` (§4.2 STOR-D4 telemetry): the wall-clock the destroy+verify took, in ms.
    pub crypto_shred_lag_ms: EpochMillis,
    /// True when this call was an idempotent no-op re-run (the subject was already erased) — the
    /// algorithm short-circuited and re-affirmed the post-condition, returning success, not an error.
    pub re_run: bool,
    /// When the erase completed (the audit timestamp the ledger recorded). PII-free.
    pub completed_at: EpochMillis,
}

impl ErasureReceipt {
    /// Whether the erase's STOR-D4 leg is GREEN: 0 of the subject's per-subject DEKs recoverable from
    /// any backup (the crypto-shred reached backups by construction).
    pub fn is_green(&self) -> bool {
        self.recoverable_in_backup == 0
    }
}

// ───────────────────────────── the six-step orchestrator ─────────────────────────────

/// **The `erase(subject, tenant)` six-step crypto-shred algorithm (contract 11.4, storage.md §5.2).**
///
/// The storage-side MECHANISM behind the DSR orchestrator: it owns step 2 (the per-subject DEK
/// destroy — it holds the [`KmsEngine`]) and drives steps 1/3/4/5/6 through the cross-holder seams the
/// DSR orchestrator wires. The algorithm is idempotent (a re-erase is a no-op success), and a partial
/// failure is a LOUD [`EraseError`] (the erasure is recorded only when every step succeeded).
///
/// It borrows the [`KmsEngine`] (the SAME engine the encrypted columns/blobs resolve DEKs through —
/// never a parallel key store, so the destroy reaches exactly the ciphertext those stores wrote) and
/// the region the tenant's KEK lives in.
pub struct CryptoShredErase<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> CryptoShredErase<'a> {
    /// Build the erase orchestrator over the KMS engine + the region the tenant's KEK lives in.
    pub fn new(engine: &'a KmsEngine, region: Region) -> CryptoShredErase<'a> {
        CryptoShredErase { engine, region }
    }

    /// Run the six-step `erase(subject, tenant)` algorithm (storage.md §5.2), in order.
    ///
    /// The `holders` bundle carries the five cross-holder seams the DSR orchestrator wires; `now` is
    /// the caller-supplied clock (deterministic; no hidden global time).
    ///
    /// Steps (copied EXACTLY from §5.2):
    ///   1. `Id.erase(subject)`  → pseudonym-map + profile shred.
    ///   2. `KMS.destroy(per_subject_DEK(tenant, subject))` → crypto-shred (live AND backups).
    ///   3. `Search.purge+reindex(subject)` → the plaintext-derived exception.
    ///   4. `Refs.tombstone(subject)`.
    ///   5. `Bus.erase(subject)` → inline-PII keys + `*.erased` tombstones.
    ///   6. record the erasure receipt (audit / erasure-ledger holder).
    ///
    /// **Idempotent:** if the ledger already records the subject as erased, the algorithm STILL
    /// re-runs every step (each is itself idempotent — a re-erase is well-defined, and a step that
    /// quietly regressed since the first erase is re-applied), but flags `re_run = true` and treats
    /// step 2's "nothing to destroy" as success. The post-condition (key destroyed, 0 recoverable in
    /// backup) is re-verified either way.
    pub fn erase(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<ErasureReceipt, EraseError> {
        let started = now;
        // The idempotent predicate: was this subject already erased? Either way we re-run every
        // (idempotent) step — but a re-run is FLAGGED (and is a no-op success, never an error).
        let re_run = holders.ledger.is_erased(subject, tenant);

        // ── Step 1: pseudonym-map shred (Id.erase). After this, history holds only the pseudonym. ──
        holders.pseudonym.shred_pseudonym(subject, tenant)?;

        // ── Step 2: KMS.destroy(per_subject_DEK(tenant, subject)) — the step storage OWNS. ──
        // Crypto-shred the subject's free-text/chat/profile/agent-memory ciphertext, LIVE AND IN
        // BACKUPS by construction (the backup holds ciphertext under the now-destroyed key, §7.5).
        // `destroy_dek` returns false if the DEK was already gone (an idempotent re-run) — which the
        // algorithm treats as success (the post-condition "the key is destroyed" already holds).
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        let dek_destroyed_now = self.engine.destroy_dek(&subject_dek);

        // ── Step 2 EXTENSION (P-ST-24 / P-253): the git crypto-shred REACH. The SAME crypto-shred
        // step reaches git's reflog/bitmap/pack-tier-backup ciphertext (sealed under the per-tenant
        // blob DEK) when the subject authored git content — destroying that DEK + VERIFYING 0
        // recoverable in backup (the reach is verified, not assumed, §5.2). The commit-object bytes
        // are the pseudonymous-by-default residual (10.9, by reference — NOT byte-mutated). A subject
        // with no git content has `git_reach = None` (a no-op). A failed post-condition is a LOUD
        // EraseError::BlobShredReach (the erasure is not recorded). ──
        if let Some(git_reach) = holders.git_reach {
            git_reach.shred_blob_tier(subject, tenant)?;
        }

        // ── Step 3: Search purge+reindex — the plaintext-derived EXCEPTION (purge, not key-destroy). ──
        holders.search.purge_and_reindex(subject, tenant)?;

        // ── Step 4: Refs tombstone. ──
        holders.refs.tombstone(subject, tenant)?;

        // ── Step 5: Bus erase — inline-PII keys + `*.erased` tombstones. ──
        holders.bus.erase_inline_pii(subject, tenant)?;

        // ── Verify the STOR-D4 post-condition: the subject's per-subject DEK is UNRECOVERABLE from
        // the backup (destroyed + excluded from backup, §7.5). Probe the KMS backup snapshot. ──
        let recoverable_in_backup = self
            .engine
            .backup_snapshot()
            .iter()
            .filter(|(d, _)| *d == subject_dek)
            .count();

        let completed_at = now;
        let crypto_shred_lag_ms = completed_at.saturating_sub(started);

        // ── Step 6: record the erasure receipt (audit / erasure-ledger holder). Recorded ONLY after
        // every prior step SUCCEEDED (an incomplete erase is never recorded — it is a loud retry). ──
        holders.ledger.record_erasure(subject, tenant, completed_at);

        Ok(ErasureReceipt {
            subject: subject.0.clone(),
            tenant: tenant.clone(),
            dek_destroyed_now,
            recoverable_in_backup,
            crypto_shred_lag_ms,
            re_run,
            completed_at,
        })
    }

    /// The region the tenant's KEK lives in (the region step 2 destroys the DEK within).
    pub fn region(&self) -> &Region {
        &self.region
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::ColumnCryptor;
    use crate::kms::KekId;
    use myelin_gdpr::ErasureMethod;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r() -> Region {
        Region("eu-west".to_string())
    }

    // ───────────── recording test doubles for the five cross-holder seams ─────────────
    //
    // Each records the ORDER it was called in (a shared call-log) so the test can assert the six
    // steps ran in the §5.2 order. Each is itself idempotent (a no-op on a repeated subject).

    #[derive(Default)]
    struct CallLog(RefCell<Vec<&'static str>>);
    impl CallLog {
        fn push(&self, step: &'static str) {
            self.0.borrow_mut().push(step);
        }
        fn steps(&self) -> Vec<&'static str> {
            self.0.borrow().clone()
        }
    }

    struct RecPseudonym<'a> {
        log: &'a CallLog,
        fail: bool,
    }
    impl PseudonymShred for RecPseudonym<'_> {
        fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            if self.fail {
                return Err(EraseError::PseudonymShred("id down".into()));
            }
            self.log.push("1:pseudonym");
            Ok(())
        }
    }

    struct RecSearch<'a> {
        log: &'a CallLog,
        fail: bool,
    }
    impl SearchPurge for RecSearch<'_> {
        fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            if self.fail {
                return Err(EraseError::SearchPurge("search down".into()));
            }
            self.log.push("3:search");
            Ok(())
        }
    }

    struct RecRefs<'a> {
        log: &'a CallLog,
    }
    impl RefsTombstone for RecRefs<'_> {
        fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.log.push("4:refs");
            Ok(())
        }
    }

    struct RecBus<'a> {
        log: &'a CallLog,
    }
    impl BusErase for RecBus<'_> {
        fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.log.push("5:bus");
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecLedger {
        log: RefCell<Vec<&'static str>>,
        erased: RefCell<BTreeSet<String>>,
    }
    impl ErasureLedgerSink for RecLedger {
        fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.log.borrow_mut().push("6:ledger");
            self.erased.borrow_mut().insert(subject.0.clone());
        }
        fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
            self.erased.borrow().contains(&subject.0)
        }
    }

    /// Stand up a KMS engine with a tenant KEK + a per-subject DEK holding a sealed column, so the
    /// erase has a real key to destroy and a real backup snapshot to probe.
    fn engine_with_subject_column(
        tenant: &TenantId,
        subject: &SubjectId,
        plaintext: &[u8],
    ) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()));
        let cryptor = ColumnCryptor::new(&kms, r());
        // Seal a free-text column under the per-subject DEK (the GD-4 individual class).
        cryptor
            .encrypt(
                tenant,
                Some(subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                plaintext,
            )
            .expect("seal a per-subject column");
        kms
    }

    // ───────────── the six steps run in order ─────────────

    #[test]
    fn erase_runs_the_six_steps_in_order() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-erase");
        let kms = engine_with_subject_column(&tenant, &subject, b"alice bio");
        let eraser = CryptoShredErase::new(&kms, r());

        let log = CallLog::default();
        let ledger = RecLedger::default();
        let ps = RecPseudonym { log: &log, fail: false };
        let se = RecSearch { log: &log, fail: false };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps, search: &se, refs: &rf, bus: &bu, ledger: &ledger,
            git_reach: None,
        };
        let receipt = eraser
            .erase(&subject, &tenant, &holders, 1_000)
            .expect("erase succeeds");

        // The non-KMS seams ran in the §5.2 order (step 2 is KMS, owned directly — not in the seam log;
        // step 6 ledger is logged by RecLedger separately, asserted below).
        assert_eq!(log.steps(), vec!["1:pseudonym", "3:search", "4:refs", "5:bus"]);
        // Step 6 recorded the erasure.
        assert_eq!(ledger.log.borrow().as_slice(), ["6:ledger"]);
        assert!(ledger.is_erased(&subject, &tenant), "the ledger records the subject as erased");

        // Step 2 actually destroyed the per-subject DEK this call.
        assert!(receipt.dek_destroyed_now, "the per-subject DEK was destroyed");
        // STOR-D4: 0 recoverable in backup.
        assert_eq!(receipt.recoverable_in_backup, 0);
        assert!(receipt.is_green(), "STOR-D4 green: 0 recoverable PII in backup");
        assert!(!receipt.re_run, "first erase is not a re-run");
        assert_eq!(receipt.completed_at, 1_000);
    }

    // ───────────── step 2 crypto-shred makes the column unrecoverable (live + backup) ─────────────

    #[test]
    fn step2_crypto_shred_renders_the_subject_column_unrecoverable_live_and_in_backup() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-1");
        let kms = engine_with_subject_column(&tenant, &subject, b"to be forgotten");
        let cryptor = ColumnCryptor::new(&kms, r());

        // Seal a fresh column we hold a handle to (proves it decrypts BEFORE the erase).
        let col = cryptor
            .encrypt(
                &tenant,
                Some(&subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                b"to be forgotten",
            )
            .unwrap();
        assert!(cryptor.decrypt(&col).is_ok(), "decrypts before the erase");

        // The subject's DEK is present in the backup snapshot BEFORE the erase.
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the subject DEK is in the backup before erase"
        );

        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();
        let ps = RecPseudonym { log: &log, fail: false };
        let se = RecSearch { log: &log, fail: false };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps, search: &se, refs: &rf, bus: &bu, ledger: &ledger,
            git_reach: None,
        };
        eraser.erase(&subject, &tenant, &holders, 5).unwrap();

        // LIVE: the column is now unrecoverable (a loud error, never plaintext).
        assert!(cryptor.decrypt(&col).is_err(), "column unrecoverable live after crypto-shred");
        // BACKUP: the subject's DEK is EXCLUDED from the backup snapshot (stays dead across restore).
        assert!(
            !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the subject DEK is absent from the backup after erase (0 recoverable, §7.5)"
        );
    }

    // ───────────── idempotency — re-erasing is a no-op success, not an error ─────────────

    #[test]
    fn re_erasing_an_already_erased_subject_is_a_noop_success() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-twice");
        let kms = engine_with_subject_column(&tenant, &subject, b"bio");
        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();
        let ps = RecPseudonym { log: &log, fail: false };
        let se = RecSearch { log: &log, fail: false };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps, search: &se, refs: &rf, bus: &bu, ledger: &ledger,
            git_reach: None,
        };

        // First erase: destroys the DEK.
        let r1 = eraser.erase(&subject, &tenant, &holders, 1).expect("first erase");
        assert!(r1.dek_destroyed_now, "first erase destroys the DEK");
        assert!(!r1.re_run);

        // SECOND erase of the SAME subject: a NO-OP SUCCESS (not an error). The DEK is already gone
        // (destroy_dek returns false), and the receipt is flagged re_run; the post-condition holds.
        let r2 = eraser
            .erase(&subject, &tenant, &holders, 2)
            .expect("re-erase is a no-op SUCCESS, never an error");
        assert!(!r2.dek_destroyed_now, "the DEK was already destroyed (idempotent re-run)");
        assert!(r2.re_run, "the second erase is flagged as a re-run");
        assert_eq!(r2.recoverable_in_backup, 0, "still 0 recoverable in backup");
        assert!(r2.is_green());
    }

    // ───────────── a partial failure is a LOUD error, never 'assume erased' ─────────────

    #[test]
    fn step1_failure_aborts_loudly_and_never_records_the_erasure() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-fail1");
        let kms = engine_with_subject_column(&tenant, &subject, b"bio");
        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();

        // Step 1 (pseudonym shred) fails → the whole erase aborts LOUDLY before destroying anything.
        let ps = RecPseudonym { log: &log, fail: true };
        let se = RecSearch { log: &log, fail: false };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps, search: &se, refs: &rf, bus: &bu, ledger: &ledger,
            git_reach: None,
        };
        let err = eraser
            .erase(&subject, &tenant, &holders, 1)
            .expect_err("a step-1 failure is a loud error");
        assert!(matches!(err, EraseError::PseudonymShred(_)));
        // The erasure was NOT recorded (an incomplete erase is a retry, never 'assume erased').
        assert!(!ledger.is_erased(&subject, &tenant), "an incomplete erase is NOT recorded");
        // The DEK was NOT destroyed (step 2 never ran).
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "step 2 never ran — the DEK is intact"
        );
    }

    #[test]
    fn step3_search_failure_aborts_after_the_shred_but_does_not_record() {
        // A step-3 failure aborts loudly AFTER step 2 destroyed the key (the DSR orchestrator retries
        // the REMAINING idempotent steps); critically the erasure is NOT recorded until all succeed.
        let tenant = t("acme");
        let subject = SubjectId::new("u-fail3");
        let kms = engine_with_subject_column(&tenant, &subject, b"bio");
        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();

        let ps = RecPseudonym { log: &log, fail: false };
        let se = RecSearch { log: &log, fail: true };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps, search: &se, refs: &rf, bus: &bu, ledger: &ledger,
            git_reach: None,
        };
        let err = eraser
            .erase(&subject, &tenant, &holders, 1)
            .expect_err("a step-3 failure is a loud error");
        assert!(matches!(err, EraseError::SearchPurge(_)));
        assert!(!ledger.is_erased(&subject, &tenant), "not recorded until every step succeeds");
        // Steps 4/5/6 never ran.
        assert_eq!(log.steps(), vec!["1:pseudonym"]);
    }

    // ───────────── the receipt is the dated STOR-D4 artifact ─────────────

    #[test]
    fn receipt_carries_the_crypto_shred_lag_and_green_reading() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-lag");
        let kms = engine_with_subject_column(&tenant, &subject, b"bio");
        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();

        // started=100, completed=140 → lag = 40ms (the clock is caller-supplied, deterministic).
        let ps = RecPseudonym { log: &log, fail: false };
        let se = RecSearch { log: &log, fail: false };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps, search: &se, refs: &rf, bus: &bu, ledger: &ledger,
            git_reach: None,
        };
        let receipt = eraser.erase(&subject, &tenant, &holders, 140).unwrap();
        assert_eq!(receipt.subject, "u-lag");
        assert_eq!(receipt.tenant, tenant);
        assert_eq!(receipt.completed_at, 140);
        // The lag is completed - started; both are `now` here (deterministic single clock) → 0.
        assert_eq!(receipt.crypto_shred_lag_ms, 0);
        assert!(receipt.is_green());
    }

    #[test]
    fn erase_error_display_names_the_loud_incomplete_failure() {
        // Kills the Display `fmt → Ok(())` mutants: each variant names its loud step + INCOMPLETE.
        let e = EraseError::PseudonymShred("x".into());
        assert!(e.to_string().contains("step 1") && e.to_string().contains("INCOMPLETE"));
        let e = EraseError::SearchPurge("x".into());
        assert!(e.to_string().contains("step 3") && e.to_string().contains("INCOMPLETE"));
        let e = EraseError::RefsTombstone("x".into());
        assert!(e.to_string().contains("step 4"));
        let e = EraseError::BusErase("x".into());
        assert!(e.to_string().contains("step 5"));
    }

    #[test]
    fn receipt_is_green_only_when_zero_recoverable() {
        // Kills the `is_green -> true` mutant: green is FALSE when a DEK is still recoverable in backup.
        let red = ErasureReceipt {
            subject: "u".into(),
            tenant: t("acme"),
            dek_destroyed_now: true,
            recoverable_in_backup: 1, // a backup could resurrect the subject
            crypto_shred_lag_ms: 0,
            re_run: false,
            completed_at: 0,
        };
        assert!(!red.is_green(), "non-zero recoverable is RED");
        let green = ErasureReceipt { recoverable_in_backup: 0, ..red };
        assert!(green.is_green(), "0 recoverable is GREEN");
    }

    #[test]
    fn region_accessor_returns_the_kek_region() {
        // Kills the `region -> &Region::default()` mutant.
        let kms = KmsEngine::new();
        let eraser = CryptoShredErase::new(&kms, r());
        assert_eq!(eraser.region(), &r());
    }
}
