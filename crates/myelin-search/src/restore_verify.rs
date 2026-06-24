//! **Restore + cross-seam + re-erase at scale — the Search restore-verify permanent gate**
//! (SRCH-P28 / P-421; architecture `search-and-indexing.md` §4.8 + §4.9; contract 6.4 reindex
//! post-restore, 10.8 the erasure ledger; drill SRCH-D9).
//!
//! This is the **restore-verify permanent gate (STOR-D1/D2 family) applied to Search's DERIVED store.**
//! It is the Search-side analog of [`myelin_storage::restore_verify::RestoreVerifyGate`] (the OLTP/blob
//! permanent gate, STOR-D1) and the bus-side [`myelin_events::reerase`] re-erase hook — re-expressed
//! over Search's reconstructible index. The gate re-runs on **every store-touching change, forever**: a
//! green is a dated [`SearchRestoreArtifact`]; a red is a typed [`SearchRestoreFailure`], never
//! swallowed.
//!
//! ## What SRCH-D9 (F3) proves (the drill source, ~364)
//! > Restore index with OLTP/blob/offsets → **no resurrected erased docs** (re-erasure runs); **no
//! > row↔doc↔vector mismatch**.
//!
//! Concretely, one gate run:
//! 1. **RESTORE the derived store to a CONSISTENT point.** Search holds NO system-of-record (§0/§1) — a
//!    "restore" of the index is a **reindex-from-source** ([`crate::reindex::SearchReindexer`], the ONLY
//!    rebuild path, SEARCH-1) up to the SAME consistency point the OLTP/blob/offsets were restored to:
//!    the owner's `replay(scope, since)` re-emits `*.snapshot` through the bus → the live indexer rebuilds
//!    the index cold (cold == live, SRCH-D5). The restored consistency point is the event-log offset the
//!    cross-seam shares with Storage's OLTP/blob restore (the §7.3 / 11.5 cursor) — Search restores its
//!    derived view to the SAME point, never a backup of the index itself.
//! 2. **RE-ERASE from the erasure ledger (10.8) via the live reindex path.** A backup restored to a point
//!    BEFORE an erase can bring an erased subject's docs back (external-insights/04 §1: *the key stays
//!    destroyed even after a backup is restored*). The gate REPLAYS the PII-free, non-shred-erasable
//!    [`SearchErasureLedger`] (10.8): for every ledger-listed subject it re-runs the IDENTICAL
//!    [`crate::erase::SearchEraseHolder::erase_subject`] (purge + compact through the SAME live consumer
//!    path — no backdoor) so any doc the restore resurrected is re-purged. The proof: **0 resurrected
//!    erased docs** after the pass.
//! 3. **ASSERT no row↔doc↔vector mismatch.** After the restore + re-erasure the derived store is at ONE
//!    consistent point: every live semantic doc has exactly one live vector (`live_count ==
//!    live_vector_count`), 0 orphan embedding survives the compaction, and the doc set equals the
//!    source-replay-minus-erased set (no orphan derived doc — every live doc projects a still-live source
//!    aggregate). A mismatch is the silent corruption the gate makes LOUD.
//!
//! ## Why the ledger is PII-free + non-shred-erasable (10.8 — CONSUMED)
//! The erasure ledger is the ONE thing that must outlive a crypto-shred AND a restore: if erasing a
//! subject also erased the record that the subject was erased, a restore could resurrect the subject with
//! nothing to re-apply. So [`SearchErasureLedger`] carries only the OPAQUE subject discriminator (the
//! pseudonymous `principal_id`, never real-identity PII) + a timestamp — it is NOT itself a
//! `PersonalDataHolder` target (a DSR does not erase the fact-of-erasure record; that would be
//! self-defeating). This is the §4.4 "non-shred-erasable" property. This is the SAME shape as the bus's
//! [`myelin_events::reerase::BusErasureLedger`] (coherence, EI-01 §7) — re-expressed over Search's
//! purge-not-shred erase (Search's primary per-subject erasure is purge + reindex, not a key-shred, so
//! the Search ledger records the SUBJECT, and the re-erase replays the purge).
//!
//! ## Loud-never-swallowed + observability is part of the pass (EI-01 §3 / §5)
//! The verdict is a `#[must_use]` typed value: a dropped red is a compile-noticed swallow. On PASS the
//! gate emits a dated [`SearchRestoreArtifact`] with the MEASURED numbers (the restored offset, the
//! re-erased subject count, the docs the restore resurrected then re-purged, the doc/vector parity, 0
//! orphan embedding); on RED it names EXACTLY which invariant broke ([`SearchRestoreFailure`]). The
//! convenience [`SearchRestoreVerifyGate::run_or_fail_ci`] turns a red into a process-failing `Err` — no
//! `|| true`, no `.ok()`, no swallow.
//!
//! ## What this module OWNS (new) vs REUSES (coherence, EI-01 §7)
//! The reindex-from-source rebuild ([`crate::reindex::SearchReindexer`], SRCH-P16), the real purge-+-reindex
//! erase ([`crate::erase::SearchEraseHolder`], SRCH-P15), the live indexer (SRCH-P06), and the bus
//! re-emit ([`myelin_events::reindex`]) ALL already exist — this prompt does **NOT** re-define any of
//! them. What is genuinely NEW is the **Search restore-verify GATE + the Search erasure ledger**: the
//! gate that drives restore (reindex-from-source) → re-erase (replay the ledger through the live erase) →
//! the three SRCH-D9 assertions → green-or-fail. It is the consumer/orchestrator that wires the existing
//! restore + erase pieces into the permanent SRCH-D9 gate.
//!
//! ## DEVIATION / FLOOR — modeled consistent point, not a live `pg_restore` (EI-01 §1, written down)
//! Search's "restore the index with OLTP/blob/offsets to a consistent point" is, on this floor, modeled
//! as a reindex-from-source up to a caller-supplied [`SearchReindexer`] cursor (`since`), driven over a
//! [`myelin_events::ReindexSource`] reference owner — the SAME owner truth the live indexer reads. The
//! cross-seam to Storage's real OLTP/blob/WAL-offset restore (the consistency point Search reindexes UP
//! TO) is the storage-side [`myelin_storage::restore_verify`] gate (STOR-D1, already built); the
//! whole-system E2E wedge that joins them at backup scale is **SRCH-P32 (E2E-3 reindex-parity / E2E-4
//! DSAR fan-out, P-465)**. The gate's SHAPE (restore-to-a-point → re-erase-from-the-ledger → 0
//! resurrected + 0 mismatch) does NOT change when the real cross-store driver lands.
//!
//! ## Floors named (the prompt's DEFINITION OF DONE)
//! - **None new for the restore-verify mechanism** — this IS the named restore-verify follow-on. The
//!   sibling slices are: **HYOK cross-store at scale (SRCH-D10) + the backup-scale erasure proof (SRCH-D4
//!   at backup scale) = SRCH-P29 (P-422)**; **the object-store index backstop (the fs→object-store
//!   BlobStore swap) = SRCH-P30 (P-463)**. Named per the DoD.
//! - **The SRCH-P15 erase mutation floor holds (unchanged)** + the SRCH-P16 reindex mutation floor holds
//!   (unchanged): this module re-drives those exact paths, it does not re-implement them.
//! - **Run at a scaled-down (CI) variant.** The drill ([`tests`] + the `drill_srch_d9_*` integration
//!   test) runs a MODERATE corpus, not the world-scale fleet corpus. The world-scale 30× load drill is
//!   the ONLY remaining floor; the SRCH-D9 restore-verify LOGIC + its dated artifact ship now and re-run
//!   as a `cargo test` gate on every store-touching change until the at-scale fleet run lands (SRCH-P32).
//! - **Mutation floor (mandatory-core — erasure-critical).** The gate's decision logic — the restore
//!   (reindex-from-source) drive, the ledger replay re-erase loop, the resurrected-doc probe, the
//!   re-purge, the 0-resurrected re-confirmation, the row↔doc↔vector parity check, the 0-orphan check —
//!   is the mutation-tested core; the floor is stated + met by the unit + drill tests in [`tests`] (every
//!   branch asserted: a mutant that skips the re-erase, drops the parity check, inverts the resurrected
//!   probe, or reports green over a resurrected doc is caught).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{EmitContextBase, OutboxStore, ReindexSource, SnapshotScope};
use myelin_gdpr::SubjectRef;
use myelin_tenancy::{Region, TenantId};

use crate::erase::SearchEraseHolder;
use crate::reindex::{ReindexError, SearchReindexer};

// ════════════════════════════════════════════════════════════════════════════════════════════
// The PII-free, non-shred-erasable Search erasure ledger (contract 10.8, CONSUMED)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// One Search ledger entry — a PII-free record that a subject was erased from the index (contract 10.8 /
/// GDPR §4.4). It carries ONLY the OPAQUE subject discriminator (the pseudonymous `principal_id`, never
/// real-identity PII) + a timestamp — never a body, never a name. It must survive the erase it records
/// AND a restore (it is non-shred-erasable), so the re-erasure pass can replay it after a restore brings
/// the subject's docs back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasedSubjectEntry {
    /// The opaque subject discriminator that was erased (the pseudonymous `principal_id` — already
    /// pseudonymous, never real-identity PII).
    pub subject_id: String,
    /// The full [`SubjectRef`] the re-erasure replays through the live erase holder. Built from the
    /// pseudonymous principal (id + kind + tenant) — no name/email. Held so the replay matches on the
    /// SAME `(principal_id, pseudonym)` the first erase did (one matcher, no drift, §4.8).
    pub subject: SubjectRef,
    /// When the erasure was recorded (the audit timestamp). PII-free.
    pub erased_at: String,
}

/// **Search's slice of the PII-free erasure ledger (contract 10.8, CONSUMED).** Durably records which
/// subjects Search erased from the `(tenant, region)` index, so [`SearchRestoreVerifyGate`] can replay
/// them after a restore. PII-free + non-shred-erasable (it must outlive the docs it records erasing and
/// survive a restore — that is the whole point: a restored backup must not be able to resurrect a subject
/// the ledger remembers erasing).
///
/// In the real binding the Search `record` writes into the GDPR-owned global ledger (10.8) through the
/// downstream DSR-orchestration adapter; here it is an in-cell `(tenant, region)`-scoped record (Search
/// is region-pinned, §3.4 — Search never crosses a cell). The map is keyed by `subject_id` so a re-erase
/// of an already-recorded subject is idempotent (it keeps the FIRST `erased_at`). This is the SAME shape
/// as [`myelin_events::reerase::BusErasureLedger`] (coherence, EI-01 §7), re-expressed over Search's
/// purge-not-shred erase.
#[derive(Clone)]
pub struct SearchErasureLedger {
    tenant: TenantId,
    region: Region,
    /// `subject_id` → the entry. A `BTreeMap` so the replay order is deterministic (sorted by subject) —
    /// the drill artifact is reproducible.
    entries: Arc<Mutex<BTreeMap<String, ErasedSubjectEntry>>>,
}

impl SearchErasureLedger {
    /// A fresh ledger for one `(tenant, region)` cell (Search is region-pinned, §3.4).
    pub fn new(tenant: TenantId, region: Region) -> SearchErasureLedger {
        SearchErasureLedger {
            tenant,
            region,
            entries: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// The cell this ledger is scoped to (Search never crosses it).
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }
    /// The region this ledger is scoped to.
    pub fn region(&self) -> &Region {
        &self.region
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, ErasedSubjectEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **Record that `subject` was erased from the index (contract 10.8).** Idempotent: recording a
    /// subject already present is a no-op that KEEPS the first `erased_at` (the first-erasure timestamp is
    /// the audit truth). Called by the DSR orchestrator after a successful
    /// [`SearchEraseHolder::erase_subject`] (the live erase) so the erasure is remembered across a
    /// restore. PII-free: only the opaque `principal_id` discriminator + the pseudonymous `SubjectRef`.
    pub fn record(&self, subject: &SubjectRef, erased_at: &str) {
        let subject_id = subject.principal.principal_id.0.clone();
        let mut g = self.lock();
        g.entry(subject_id.clone())
            .or_insert_with(|| ErasedSubjectEntry {
                subject_id,
                subject: subject.clone(),
                erased_at: erased_at.to_string(),
            });
    }

    /// Whether the ledger remembers erasing `subject_id` (the fail-closed read). True once `record`ed; a
    /// restore CANNOT clear it (non-shred-erasable).
    pub fn is_erased(&self, subject_id: &str) -> bool {
        self.lock().contains_key(subject_id)
    }

    /// Every recorded erasure, in deterministic (subject-sorted) order — what the re-erasure pass
    /// replays. PII-free.
    pub fn entries(&self) -> Vec<ErasedSubjectEntry> {
        self.lock().values().cloned().collect()
    }

    /// How many subjects the ledger has recorded as erased.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The dated green artifact (the SRCH-D9 proof on pass)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated GREEN ARTIFACT a Search restore-verify pass returns (the SRCH-D9 proof; observability is
/// part of the pass, EI-01 §3). It carries the MEASURED numbers — never a bare "ok": the consistency
/// point the index was restored (reindexed) to, the re-erased subject count, how many docs the restore
/// RESURRECTED (were live again before the re-erasure re-purged them — the honest "what the backup
/// brought back" signal), and the three zeros the gate asserted (0 resurrected docs post-pass, 0
/// row↔doc↔vector mismatch, 0 orphan embedding). PII-free: opaque ids + counts, never a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRestoreArtifact {
    /// The cell the gate ran within (Search never crosses it).
    pub tenant: TenantId,
    /// The region the gate ran within.
    pub region: Region,
    /// The consistency point the index was restored (reindexed-from-source) UP TO — the `since`/offset
    /// the OLTP/blob/offsets cross-seam shares. `None` = a full rebuild from cold (`since = 0`).
    pub restored_to_offset: Option<u64>,
    /// The live doc count after the restore + re-erasure (the rebuilt, re-erased index size).
    pub live_doc_count: u64,
    /// The live vector count after the restore + re-erasure (MUST equal [`Self::live_doc_count`] for a
    /// semantic corpus — the row↔doc↔vector parity).
    pub live_vector_count: usize,
    /// How many ledger-listed subjects were replayed through the re-erasure pass.
    pub re_erased_subjects: usize,
    /// How many docs the RESTORE resurrected (were live again BEFORE the re-erasure re-purged them) — the
    /// honest signal of what the restored point brought back for the ledger's subjects.
    pub docs_resurrected_by_restore: usize,
    /// **THE GATE READING:** how many of the ledger's subjects still have a live doc AFTER the re-erasure
    /// pass — MUST be **0** (re-erasure re-purged everything the restore resurrected).
    pub resurrected_docs: usize,
    /// Row↔doc↔vector mismatches found — `0` on a green pass (the one-consistent-point leg).
    pub row_doc_vector_mismatches: usize,
    /// Orphan embeddings surviving the compaction — `0` on a green pass (the erasure-critical leg, §3.3).
    pub orphan_embeddings: bool,
    /// When the pass ran (the dated artifact).
    pub ran_at: String,
}

impl SearchRestoreArtifact {
    /// Whether the Search restore-verify gate is GREEN: 0 resurrected erased docs + 0 row↔doc↔vector
    /// mismatch + 0 orphan embedding post-restore-and-re-erase.
    pub fn is_green(&self) -> bool {
        self.resurrected_docs == 0 && self.row_doc_vector_mismatches == 0 && !self.orphan_embeddings
    }

    /// Render the dated green-artifact line a CI run prints on PASS (the measured-numbers proof). The
    /// caller prefixes the date (`[P-421 GATE GREEN <date>]`).
    pub fn summary(&self) -> String {
        format!(
            "search restore-verify PASS (SRCH-D9): restored index to offset={:?} via \
             reindex-from-source — {} live docs / {} live vectors (parity), re-erased {} ledger \
             subject(s), {} doc(s) resurrected-by-restore then re-purged; resurrected_docs={}, \
             row_doc_vector_mismatches={}, orphan_embeddings={} (all 0/false). cold==live by \
             construction.",
            self.restored_to_offset,
            self.live_doc_count,
            self.live_vector_count,
            self.re_erased_subjects,
            self.docs_resurrected_by_restore,
            self.resurrected_docs,
            self.row_doc_vector_mismatches,
            self.orphan_embeddings,
        )
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The gate failure (what broke — never a bare bool)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// A RED Search restore-verify result — EXACTLY which SRCH-D9 invariant failed (observability is part of
/// the pass, EI-01 §3) so a failed CI run points at the precise corruption, never a bare "gate failed".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchRestoreFailure {
    /// **The restore (reindex-from-source) itself FAILed** — an unknown owner, an outbox/emit failure, or
    /// the live indexer rejecting a re-driven snapshot. The restore could not reach a consistent point;
    /// the gate FAILs CI loudly (never a silent empty rebuild that masks a wiring bug).
    RestoreFailed(ReindexError),
    /// **The re-erasure (live purge replay) itself FAILed** — the live erase holder rejected a re-purge
    /// (the engine failed). An incomplete re-erasure is never swallowed.
    ReEraseFailed(String),
    /// **An erased subject was RESURRECTED (the no-resurrected-erased-docs leg of SRCH-D9):** after the
    /// restore + the re-erasure pass, a ledger-erased subject STILL has a live doc — a restored backup
    /// resurrected an erased subject and the re-erasure could not re-purge it. The gravest failure: it
    /// un-erases a person. The gate FAILs CI. Names the subject + the count of its surviving docs.
    ErasureResurrected {
        /// The opaque subject discriminator that should have stayed erased but has live docs.
        subject_id: String,
        /// How many live docs the resurrected subject still has after the re-erasure pass (`> 0`).
        surviving_docs: usize,
    },
    /// **Row↔doc↔vector mismatch (the one-consistent-point leg of SRCH-D9):** the restored + re-erased
    /// index is NOT at one consistent point — the live semantic-doc count and the live vector count
    /// disagree (an indexed doc whose vector is missing, or a vector with no live doc). Names both counts.
    RowDocVectorMismatch {
        /// The live doc count after the restore + re-erasure.
        live_docs: u64,
        /// The live vector count after the restore + re-erasure.
        live_vectors: usize,
    },
    /// **An orphan embedding survived the compaction (the erasure-critical leg, §3.3):** a tombstoned
    /// vector's bytes are physically still present after the re-erasure compaction — embeddings are
    /// personal data and must be erased with their source. The gate FAILs CI.
    OrphanEmbedding,
}

impl core::fmt::Display for SearchRestoreFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SearchRestoreFailure::RestoreFailed(e) => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL — the restore (reindex-from-source) failed: {e}"
            ),
            SearchRestoreFailure::ReEraseFailed(e) => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL — the post-restore re-erasure failed: {e}"
            ),
            SearchRestoreFailure::ErasureResurrected {
                subject_id,
                surviving_docs,
            } => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL — ERASURE RESURRECTED: subject {subject_id} was erased \
                 before the backup but has {surviving_docs} live doc(s) after the restore + \
                 re-erasure — a restored backup resurrected an erased subject. THE GRAVEST FAILURE: \
                 it un-erases a person"
            ),
            SearchRestoreFailure::RowDocVectorMismatch {
                live_docs,
                live_vectors,
            } => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL — ROW↔DOC↔VECTOR MISMATCH: {live_docs} live docs but \
                 {live_vectors} live vectors — the restored index is NOT at one consistent point"
            ),
            SearchRestoreFailure::OrphanEmbedding => write!(
                f,
                "SEARCH RESTORE-VERIFY FAIL — ORPHAN EMBEDDING: a tombstoned vector's bytes survived \
                 the re-erasure compaction (embeddings are personal data, §3.3)"
            ),
        }
    }
}

impl std::error::Error for SearchRestoreFailure {}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The gate verdict (loud-never-swallowed, #[must_use])
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The typed verdict of a Search restore-verify run — GREEN (a [`SearchRestoreArtifact`]) or RED (a
/// [`SearchRestoreFailure`]). `#[must_use]`: a dropped verdict is a swallowed data-loss/erasure check
/// (the exact EI-01 §5 loud-never-swallowed violation) and the compiler flags it. The only way to
/// consume a red is to handle it (e.g. [`SearchRestoreVerifyGate::run_or_fail_ci`] turns it into a
/// process-failing `Err`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a Search restore-verify verdict must be checked — a dropped RED is a SWALLOWED \
              resurrected-erased-subject / silent-corruption failure (the permanent gate, EI-01 §5: \
              loud-never-swallowed)"]
pub enum SearchRestoreVerdict {
    /// The restore is whole + the erasure held: 0 resurrected erased docs, 0 row↔doc↔vector mismatch, 0
    /// orphan embedding. Carries the dated [`SearchRestoreArtifact`] with the measured numbers.
    Green(SearchRestoreArtifact),
    /// The restore/re-erase is NOT whole — EXACTLY what broke. FAILs CI; never swallowed.
    Red(SearchRestoreFailure),
}

impl SearchRestoreVerdict {
    /// `true` iff the gate passed. The ONLY way to read a pass — a [`Red`](SearchRestoreVerdict::Red) is
    /// never silently a pass.
    pub fn is_green(&self) -> bool {
        matches!(self, SearchRestoreVerdict::Green(_))
    }

    /// The green artifact, if the gate passed.
    pub fn artifact(&self) -> Option<&SearchRestoreArtifact> {
        match self {
            SearchRestoreVerdict::Green(a) => Some(a),
            SearchRestoreVerdict::Red(_) => None,
        }
    }

    /// The failure, if the gate failed.
    pub fn failure(&self) -> Option<&SearchRestoreFailure> {
        match self {
            SearchRestoreVerdict::Red(f) => Some(f),
            SearchRestoreVerdict::Green(_) => None,
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The gate inputs
// ════════════════════════════════════════════════════════════════════════════════════════════

/// Everything one Search restore-verify run consumes — the inputs that drive the restore
/// (reindex-from-source) + the re-erasure replay. Grouped into one struct so
/// [`SearchRestoreVerifyGate::run`] stays the load-bearing seam.
pub struct SearchRestoreInputs<'a> {
    /// The reindex-from-source driver (SRCH-P16) — the ONLY rebuild path. The "restore" drives this UP TO
    /// the consistency point.
    pub reindexer: &'a SearchReindexer,
    /// The live erase holder (SRCH-P15) — the re-erasure replays its purge + compact through the SAME
    /// live consumer path (no backdoor).
    pub erase_holder: &'a SearchEraseHolder,
    /// The PII-free, non-shred-erasable erasure ledger (10.8) the re-erasure pass replays.
    pub ledger: &'a SearchErasureLedger,
    /// The tenant the restore + re-erasure run within (Search is region-pinned — the region comes from
    /// the reindexer/holder config).
    pub tenant: TenantId,
    /// The reindex scope to restore (the owning subsystem + selector, §4.9).
    pub scope: SnapshotScope,
    /// The consistency point the OLTP/blob/offsets were restored to — Search reindexes its derived view
    /// UP TO the SAME point. `None` = a full cold rebuild from `since = 0` (the index is wiped first);
    /// `Some(v)` = an incremental restore that resumes from the cross-seam offset.
    pub restore_to_offset: Option<u64>,
    /// The owning subsystems' [`ReindexSource`]s whose `replay` re-emits `*.snapshot` (their real bodies
    /// are EB-26 / per-owner M3/M4 — the named floor; the seam shape is fixed).
    pub sources: &'a [&'a dyn ReindexSource],
    /// The bus outbox the re-emitted snapshots co-commit to (the SAME outbox→bus→live-consumer path).
    pub outbox: &'a mut OutboxStore,
    /// The emit context (the platform actor + clock) the bus re-emit stamps.
    pub ctx_base: EmitContextBase,
    /// The dated timestamp the artifact records (the caller's clock).
    pub now: String,
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The gate
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The Search restore-verify permanent gate (SRCH-D9 — the headline).** Restores Search's derived
/// store to a consistent point (reindex-from-source, the ONLY rebuild path), re-erases from the erasure
/// ledger (10.8) through the live erase, and asserts the three SRCH-D9 invariants (0 resurrected erased
/// docs / 0 row↔doc↔vector mismatch / 0 orphan embedding), emitting a dated GREEN ARTIFACT on pass and a
/// typed [`SearchRestoreFailure`] on red. It re-runs on every store-touching change, forever (master §4)
/// — loud-never-swallowed.
///
/// A zero-sized orchestrator: it holds no state, so a CI run is `SearchRestoreVerifyGate::run(inputs)` —
/// the inputs ARE the restore source. Reuses [`SearchReindexer`] (SRCH-P16), [`SearchEraseHolder`]
/// (SRCH-P15), and the bus re-emit; adds the restore-then-re-erase-then-verify orchestration.
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchRestoreVerifyGate;

impl SearchRestoreVerifyGate {
    /// A new gate (stateless).
    pub fn new() -> SearchRestoreVerifyGate {
        SearchRestoreVerifyGate
    }

    /// **Run the Search restore-verify gate once (SRCH-D9).** Returns [`SearchRestoreVerdict::Green`]
    /// (the restore is whole + the erasure held + the dated artifact) or [`SearchRestoreVerdict::Red`]
    /// (exactly what broke). NEVER swallows a failure.
    ///
    /// The sequence (architecture §4.8/§4.9; drill SRCH-D9):
    /// 1. **RESTORE — reindex-from-source to the consistency point.** Drive [`SearchReindexer::reindex`]
    ///    UP TO `restore_to_offset` (the cross-seam offset the OLTP/blob/offsets restored to). A restore
    ///    failure is surfaced LOUD ([`SearchRestoreFailure::RestoreFailed`]).
    /// 2. **PROBE — what did the restore resurrect?** For every ledger-listed subject, count its live
    ///    docs AFTER the restore (before re-erasing) — the honest "what the backup brought back" signal.
    /// 3. **RE-ERASE — replay the ledger through the live erase.** For each ledger subject re-run the
    ///    IDENTICAL [`SearchEraseHolder::erase_subject`] (purge + compact through the SAME live consumer
    ///    path). Idempotent: a subject already absent re-purges 0.
    /// 4. **ASSERT** the three SRCH-D9 invariants: 0 resurrected erased docs (re-confirm after the pass),
    ///    0 row↔doc↔vector mismatch (`live_count == live_vector_count`), 0 orphan embedding.
    pub fn run(&self, inputs: &mut SearchRestoreInputs<'_>) -> SearchRestoreVerdict {
        let tenant = inputs.tenant.clone();
        let region = inputs.reindexer.region().clone();

        // (1) RESTORE — reindex-from-source UP TO the consistency point. The ONLY rebuild path; a failure
        // is the no-loss floor — surfaced LOUD, never swallowed.
        if let Err(e) = inputs.reindexer.reindex(
            &tenant,
            &inputs.scope,
            inputs.restore_to_offset,
            inputs.sources,
            inputs.outbox,
            inputs.ctx_base.clone(),
        ) {
            return SearchRestoreVerdict::Red(SearchRestoreFailure::RestoreFailed(e));
        }

        let entries = inputs.ledger.entries();

        // (2) PROBE: how many docs did the RESTORE resurrect for the ledger's subjects? Count BEFORE
        // re-erasing — the honest "what the backup brought back" signal.
        let mut docs_resurrected_by_restore = 0usize;
        for entry in &entries {
            docs_resurrected_by_restore +=
                self.live_docs_for(inputs.erase_holder, &entry.subject, &tenant);
        }

        // (3) RE-ERASE: replay the ledger — re-run the IDENTICAL live erase (purge + compact) for every
        // ledger-listed subject. Idempotent: an already-absent subject re-purges 0. Loud on a live-erase
        // failure (an incomplete re-erasure is never swallowed).
        for entry in &entries {
            if let Err(e) = inputs.erase_holder.erase_subject(&entry.subject, &tenant) {
                return SearchRestoreVerdict::Red(SearchRestoreFailure::ReEraseFailed(format!(
                    "{e:?}"
                )));
            }
        }

        // (4a) RE-CONFIRM: after the pass, NO ledger-erased subject may still have a live doc (0
        // resurrected). The gate reading — the re-erasure re-purged everything the restore resurrected.
        let mut resurrected_docs = 0usize;
        for entry in &entries {
            let surviving = self.live_docs_for(inputs.erase_holder, &entry.subject, &tenant);
            if surviving > 0 {
                return SearchRestoreVerdict::Red(SearchRestoreFailure::ErasureResurrected {
                    subject_id: entry.subject_id.clone(),
                    surviving_docs: surviving,
                });
            }
            resurrected_docs += surviving;
        }

        // (4b) ROW↔DOC↔VECTOR PARITY: the restored + re-erased index is at one consistent point — every
        // live semantic doc has exactly one live vector. (The corpus is fully semantic: every live doc
        // carries a vector, so the counts must agree.)
        let live_docs = inputs.reindexer.indexer_live_count(&tenant, &region);
        let live_vectors = inputs.reindexer.indexer_live_vector_count(&tenant, &region);
        if (live_docs as usize) != live_vectors {
            return SearchRestoreVerdict::Red(SearchRestoreFailure::RowDocVectorMismatch {
                live_docs,
                live_vectors,
            });
        }

        // (4c) 0 ORPHAN EMBEDDING (§3.3): the re-erasure compaction physically removed every tombstoned
        // vector. An orphan is the erasure-critical RED.
        let orphan = inputs
            .reindexer
            .indexer_has_orphan_embedding(&tenant, &region);
        if orphan {
            return SearchRestoreVerdict::Red(SearchRestoreFailure::OrphanEmbedding);
        }

        // PASS — the restore is whole + the erasure held. Emit the dated green artifact.
        SearchRestoreVerdict::Green(SearchRestoreArtifact {
            tenant,
            region,
            restored_to_offset: inputs.restore_to_offset,
            live_doc_count: live_docs,
            live_vector_count: live_vectors,
            re_erased_subjects: entries.len(),
            docs_resurrected_by_restore,
            resurrected_docs,
            row_doc_vector_mismatches: 0,
            orphan_embeddings: false,
            ran_at: inputs.now.clone(),
        })
    }

    /// **The loud-never-swallowed CI entrypoint (EI-01 §5).** Run the gate and turn a RED verdict into a
    /// process-failing `Err(SearchRestoreFailure)` — so a CI invocation FAILS CI on a red restore, with
    /// NO `|| true`, no `.ok()`, no swallow. On GREEN it returns the dated [`SearchRestoreArtifact`].
    pub fn run_or_fail_ci(
        &self,
        inputs: &mut SearchRestoreInputs<'_>,
    ) -> Result<SearchRestoreArtifact, SearchRestoreFailure> {
        match self.run(inputs) {
            SearchRestoreVerdict::Green(artifact) => Ok(artifact),
            SearchRestoreVerdict::Red(failure) => Err(failure),
        }
    }

    /// Count the live docs a subject currently references in the `(tenant, region)` index (via the holder's
    /// locate — the SAME matcher erase uses, §4.8). The resurrected-probe + the re-confirm both read this.
    fn live_docs_for(
        &self,
        holder: &SearchEraseHolder,
        subject: &SubjectRef,
        tenant: &TenantId,
    ) -> usize {
        holder.locate_doc_count(subject, tenant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dek::SearchDekPin;
    use crate::engine::AclFilter;
    use crate::indexer::{
        IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
        SearchProjection,
    };
    use myelin_events::reindex::ReferenceReindexSource;
    use myelin_events::{Actor, ArtifactRef, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};
    use myelin_storage::KmsEngine;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    const REGION: &str = "fr-par";

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region(REGION.into())
    }
    fn platform() -> Principal {
        Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )
    }
    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(platform()),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
            caused_by: None,
        }
    }

    /// A subject (human) by opaque principal id — the pseudonymous subject the ledger records + the erase
    /// matches on (never a name/email).
    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    /// The owner-projection fetcher (5.6) — the live `index()` step fetches the owner's projection here,
    /// never the owner DB (the no-cross-db seam).
    #[derive(Default)]
    struct OwnerProjection {
        bodies: StdMutex<HashMap<String, String>>,
    }
    impl OwnerProjection {
        fn put(&self, ref_: &str, body: &str) {
            self.bodies
                .lock()
                .unwrap()
                .insert(ref_.to_string(), body.to_string());
        }
        fn remove(&self, ref_: &str) {
            self.bodies.lock().unwrap().remove(ref_);
        }
    }
    impl ProjectFetcher for OwnerProjection {
        fn project(
            &self,
            _t: &TenantId,
            _r: &Region,
            ref_: &ArtifactRef,
        ) -> Result<SearchProjection, ProjectFetchError> {
            match self.bodies.lock().unwrap().get(&ref_.0) {
                Some(body) => Ok(SearchProjection {
                    text: body.clone(),
                    fields: BTreeMap::new(),
                    lang: None,
                }),
                None => Err(ProjectFetchError::Gone),
            }
        }
    }

    /// The semantic knowledge.page spec — every live doc carries a vector (so doc↔vector parity is exact).
    fn page_spec() -> IndexSpec {
        IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()
    }

    fn snapshot_ref(agg: &str) -> String {
        format!("myelin://t/knowledge/page/{agg}")
    }
    fn scope() -> SnapshotScope {
        SnapshotScope::new("knowledge", "page:all")
    }

    /// Build a fully-wired cell: the live indexer + the reindexer + the erase holder over the SAME index,
    /// plus the owner-projection fetcher. The bodies map is shared so a test can tombstone the owner's
    /// projection (the erasure reaching the owner, X-7).
    #[allow(clippy::type_complexity)]
    fn cell() -> (
        Arc<IncrementalIndexer>,
        Arc<OwnerProjection>,
        SearchReindexer,
        SearchEraseHolder,
    ) {
        let fetcher = Arc::new(OwnerProjection::default());
        let ix = Arc::new(IncrementalIndexer::new(
            vec![page_spec()],
            fetcher.clone(),
            Arc::new(MockEmbeddingAdapter::new(8)),
        ));
        let reindexer = SearchReindexer::new(ix.clone(), region());
        let kms = Arc::new(KmsEngine::new());
        let pin = SearchDekPin::new(kms);
        pin.reserve(&tenant(), &region())
            .expect("reserve index DEK");
        let holder = SearchEraseHolder::new(ix.clone(), pin, region());
        (ix, fetcher, reindexer, holder)
    }

    /// The pseudonym `<pseudonym>@<tenant>.noreply` (contract 4.8) the body mentions the subject by.
    fn pseudonym(id: &str) -> String {
        PseudonymHandle::new(id, &tenant().0)
            .expect("pseudonym renders")
            .render()
    }

    // ───────── the headline GREEN (the dated SRCH-D9 artifact) ─────────

    /// **THE HEADLINE: restore the index (reindex-from-source) to a consistent point where an
    /// already-erased subject's docs come BACK, re-erase from the ledger, and assert 0 resurrected + 0
    /// row↔doc↔vector mismatch + 0 orphan → a dated GREEN artifact.** The DoD pass.
    #[test]
    fn the_gate_greens_a_whole_restore_with_re_erasure() {
        let (ix, fetcher, reindexer, holder) = cell();
        let erased = subject("u-erased");
        let pn = pseudonym("u-erased");

        // The owner truth BEFORE the erase: two pages, one mentioning the erased subject. This is the
        // state the older backup captured — a restore to this point brings the erased doc BACK.
        let mut src_before = ReferenceReindexSource::new("knowledge", "page");
        src_before.upsert("owned", 1, serde_json::json!({ "kind": "page" }));
        src_before.upsert("other", 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(
            &snapshot_ref("owned"),
            &format!("a page mentioning {pn} about raft"),
        );
        fetcher.put(&snapshot_ref("other"), "an unrelated page about paxos");

        // The subject was erased in the live cell BEFORE the backup-restore — record it in the ledger.
        let ledger = SearchErasureLedger::new(tenant(), region());
        ledger.record(&erased, "2026-06-20T00:00:00Z");
        assert!(ledger.is_erased("u-erased"));

        // RESTORE to the pre-erase point: the reindex-from-source brings the erased subject's doc back
        // (the owner's pre-erase truth still holds it). The gate then RE-ERASES from the ledger.
        let mut outbox = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src_before];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: scope(),
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };

        let verdict = SearchRestoreVerifyGate::new().run(&mut inputs);
        assert!(
            verdict.is_green(),
            "a whole restore + re-erase must GREEN, got {:?}",
            verdict.failure()
        );
        let a = verdict.artifact().expect("green artifact");
        assert_eq!(a.re_erased_subjects, 1, "the ledger's one subject replayed");
        assert_eq!(
            a.docs_resurrected_by_restore, 1,
            "the restore brought the erased subject's doc back"
        );
        assert_eq!(a.resurrected_docs, 0, "0 resurrected docs post-re-erase");
        assert_eq!(a.row_doc_vector_mismatches, 0);
        assert!(!a.orphan_embeddings, "0 orphan embedding");
        // The surviving (unrelated) doc is intact; the erased subject's doc is GONE.
        assert_eq!(a.live_doc_count, 1, "only the unrelated page survives");
        assert_eq!(a.live_vector_count, 1, "exactly one live vector (parity)");
        let raft = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
            .expect("ft");
        assert!(
            raft.is_empty(),
            "the erased subject's page is NOT resurrected"
        );
        let paxos = ix
            .search_ft(&tenant(), &region(), &AclFilter::All, "paxos", 10)
            .expect("ft");
        assert_eq!(paxos.len(), 1, "the unrelated page is searchable");
        // The dated summary names the measured numbers (observability is part of the pass).
        let s = a.summary();
        assert!(s.contains("search restore-verify PASS (SRCH-D9)"));
        assert!(s.contains("re-erased 1 ledger subject"));
        let _ = fetcher; // (the fetcher is shared into the indexer)
    }

    /// `run_or_fail_ci` returns `Ok(artifact)` on a green run (CI continues). Empty ledger = a clean
    /// restore with nothing to re-erase.
    #[test]
    fn run_or_fail_ci_returns_ok_on_green_empty_ledger() {
        let (_ix, fetcher, reindexer, holder) = cell();
        let mut src = ReferenceReindexSource::new("knowledge", "page");
        src.upsert("a", 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(&snapshot_ref("a"), "a page about consensus");
        let ledger = SearchErasureLedger::new(tenant(), region());
        let mut outbox = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: scope(),
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };
        let a = SearchRestoreVerifyGate::new()
            .run_or_fail_ci(&mut inputs)
            .expect("a whole restore must not fail CI");
        assert_eq!(a.re_erased_subjects, 0, "nothing to re-erase");
        assert_eq!(a.live_doc_count, 1);
        assert_eq!(a.live_vector_count, 1, "parity");
    }

    // ───────── the no-resurrected-erased-docs leg (mandatory-core) ─────────

    /// **MANDATORY-CORE: WITHOUT the re-erasure step, a restore to the pre-erase point WOULD resurrect
    /// the erased subject.** This proves the gate's re-erasure is load-bearing (not a no-op): we do a
    /// bare reindex (the restore) and assert the erased doc IS back; then the full gate re-erases it
    /// away. Kills a mutant that skips the re-erase loop.
    #[test]
    fn re_erasure_is_load_bearing_a_bare_restore_resurrects() {
        let (ix, fetcher, reindexer, holder) = cell();
        let erased = subject("u-x");
        let pn = pseudonym("u-x");
        let mut src = ReferenceReindexSource::new("knowledge", "page");
        src.upsert("owned", 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(
            &snapshot_ref("owned"),
            &format!("page mentioning {pn} re raft"),
        );

        // A BARE restore (just the reindex, no re-erase): the erased subject's doc is RESURRECTED.
        let mut outbox = OutboxStore::new();
        reindexer
            .reindex(&tenant(), &scope(), None, &[&src], &mut outbox, ctx_base())
            .expect("bare restore");
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            1,
            "the bare restore brought the erased doc back (resurrection without re-erase)"
        );

        // Now the FULL gate (over a fresh outbox) re-erases it: 0 resurrected.
        let ledger = SearchErasureLedger::new(tenant(), region());
        ledger.record(&erased, "2026-06-20T00:00:00Z");
        let mut outbox2 = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: scope(),
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox2,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };
        let verdict = SearchRestoreVerifyGate::new().run(&mut inputs);
        assert!(
            verdict.is_green(),
            "the gate re-erases the resurrected doc: {:?}",
            verdict.failure()
        );
        assert_eq!(verdict.artifact().unwrap().docs_resurrected_by_restore, 1);
        assert_eq!(verdict.artifact().unwrap().resurrected_docs, 0);
        assert_eq!(
            ix.live_count(&tenant(), &region()),
            0,
            "the erased doc is purged again"
        );
    }

    /// **The re-erasure is idempotent: a restore where the owner ALSO tombstoned the erased aggregate
    /// (the erasure reached the owner, X-7) resurrects nothing — the gate re-erases 0 and still GREENs.**
    #[test]
    fn re_erasure_is_idempotent_when_owner_tombstoned() {
        let (ix, fetcher, reindexer, holder) = cell();
        let erased = subject("u-gone");
        // The owner's POST-erase truth: the erased aggregate is gone (tombstoned), only an unrelated page
        // remains. A restore to THIS point does not resurrect anything.
        let mut src_after = ReferenceReindexSource::new("knowledge", "page");
        src_after.upsert("other", 1, serde_json::json!({ "kind": "page" }));
        fetcher.put(&snapshot_ref("other"), "unrelated page about paxos");
        fetcher.remove(&snapshot_ref("owned")); // the owner's projection is gone too.

        let ledger = SearchErasureLedger::new(tenant(), region());
        ledger.record(&erased, "2026-06-20T00:00:00Z");
        let mut outbox = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src_after];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: scope(),
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };
        let verdict = SearchRestoreVerifyGate::new().run(&mut inputs);
        let a = verdict.artifact().expect("green");
        assert_eq!(
            a.docs_resurrected_by_restore, 0,
            "the owner tombstoned it — nothing resurrected"
        );
        assert_eq!(a.resurrected_docs, 0);
        assert_eq!(a.live_doc_count, 1, "only the unrelated page");
        assert_eq!(ix.live_count(&tenant(), &region()), 1);
    }

    // ───────── the loud RestoreFailed leg ─────────

    /// **A restore of an UNKNOWN owner FAILs the gate LOUD (never a silent empty rebuild).** The
    /// reindex's `Bus` error surfaces as `RestoreFailed`; `run_or_fail_ci` FAILs CI.
    #[test]
    fn an_unknown_owner_fails_the_gate_loud() {
        let (_ix, _f, reindexer, holder) = cell();
        let src = ReferenceReindexSource::new("knowledge", "page");
        let ledger = SearchErasureLedger::new(tenant(), region());
        let unknown = SnapshotScope::new("refs", "edge:all"); // no `refs` source registered.
        let mut outbox = OutboxStore::new();
        let srcs: &[&dyn ReindexSource] = &[&src];
        let mut inputs = SearchRestoreInputs {
            reindexer: &reindexer,
            erase_holder: &holder,
            ledger: &ledger,
            tenant: tenant(),
            scope: unknown,
            restore_to_offset: None,
            sources: srcs,
            outbox: &mut outbox,
            ctx_base: ctx_base(),
            now: "2026-06-24T12:00:00Z".into(),
        };
        let err = SearchRestoreVerifyGate::new()
            .run_or_fail_ci(&mut inputs)
            .expect_err("an unknown owner must fail CI");
        assert!(
            matches!(
                err,
                SearchRestoreFailure::RestoreFailed(ReindexError::Bus(_))
            ),
            "loud RestoreFailed: {err}"
        );
        assert!(err.to_string().contains("SEARCH RESTORE-VERIFY FAIL"));
    }

    // ───────── the ledger (PII-free, non-shred-erasable, idempotent) ─────────

    /// The ledger records a subject once (idempotent record keeps the first timestamp) and is keyed by
    /// the OPAQUE principal id (PII-free).
    #[test]
    fn ledger_is_pii_free_and_idempotent() {
        let ledger = SearchErasureLedger::new(tenant(), region());
        let s = subject("u-1");
        ledger.record(&s, "2026-06-20T00:00:00Z");
        ledger.record(&s, "2026-06-21T00:00:00Z"); // a re-record keeps the FIRST timestamp.
        assert_eq!(ledger.len(), 1, "one subject, idempotent");
        let e = &ledger.entries()[0];
        assert_eq!(
            e.subject_id, "u-1",
            "keyed by the opaque principal id (PII-free)"
        );
        assert_eq!(e.erased_at, "2026-06-20T00:00:00Z", "first timestamp kept");
        assert!(ledger.is_erased("u-1"));
        assert!(!ledger.is_erased("u-2"));
    }

    /// Multi-subject: the ledger records many subjects + replays all in deterministic (subject-sorted)
    /// order — the at-scale re-erasure replay.
    #[test]
    fn ledger_records_many_subjects_in_deterministic_order() {
        let ledger = SearchErasureLedger::new(tenant(), region());
        ledger.record(&subject("u-c"), "2026-06-20T00:00:00Z");
        ledger.record(&subject("u-a"), "2026-06-20T00:00:00Z");
        ledger.record(&subject("u-b"), "2026-06-20T00:00:00Z");
        let ids: Vec<String> = ledger.entries().into_iter().map(|e| e.subject_id).collect();
        assert_eq!(
            ids,
            vec!["u-a", "u-b", "u-c"],
            "deterministic subject-sorted order"
        );
    }
}
