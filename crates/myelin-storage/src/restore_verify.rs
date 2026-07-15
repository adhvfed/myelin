//! # THE HEADLINE: the CI-wired restore-verify gate (STOR-D1, the permanent gate)
//!
//! **Prompt:** P-ST-13 → global **P-061** (M1). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §7.4 (*Automated restore-verification
//! — the CI-wired durability gate: spin a clean target; restore T1/T2/T5; reindex T4/Search/Refs from
//! source to T; assert **no loss** (checksum parity), **cross-seam** (every restored row's blob hash
//! present + integrity-verified; derived == source-replay), and **erasure held** (a subject erased
//! before the backup is still erased after restore — §7.5). Green artifact on pass; RED gate fails
//! CI.*), §7.3 (the cross-seam consistency point = the per-aggregate outbox `seq` / event-log offset).
//! **Contract-index:** row **11.5** (the CI-wired restore-verify gate half — the restore machinery is
//! P-ST-12 / P-060 [`crate::restore`]; the post-restore re-erasure is P-ST-14 / P-100). Consumed: 2.6
//! (reindex-from-source for the derived rebuild — [`crate::restore::ReindexFromSource`]).
//!
//! ## What a permanent gate IS (master §4 / EI-01 §3 / §5)
//! This is **one of the two permanent gates** the whole run ratchets against (the other is the
//! sandbox-escape gate AG-D4/CI-T1). The restore-verify CI job re-runs on **every change touching a
//! store, forever** — it never moves to "done". The doctrine it is built to (EI-01 §3, read in full):
//! - **A backup that has never been restored is not a backup.** The gate does not check that a backup
//!   *exists*; it *restores* it and asserts the restored copy is whole.
//! - **Never weaken a threshold or invert an assertion to make a check pass.** A red gate becomes a
//!   dated "claimed, not proven" thresholds-file row — never a lowered bar. The verdict here is a
//!   `#[must_use]` typed value: a dropped red is a compile-noticed swallow, not a silent pass.
//! - **Loud-never-swallowed (EI-01 §5).** The gate's CI driver [`RestoreVerifyGate::run`] returns a
//!   hard [`GateVerdict`]; the convenience [`RestoreVerifyGate::run_or_fail_ci`] turns a red verdict
//!   into a process-failing `Err` — there is NO `|| true`, no `ok()`, no swallow. The drill test
//!   `stor_d1_gate_is_loud_never_swallowed` proves a red surfaces as `Err`, never a hidden pass.
//! - **Observability is part of the pass condition.** On PASS the gate emits a dated GREEN ARTIFACT
//!   ([`GreenArtifact`]) carrying the measured numbers (offset T, 0 dangling, 0 checksum mismatches,
//!   0 resurrected subjects); on RED it names EXACTLY which seam failed ([`GateFailure`]).
//!
//! ## The three assertions the gate makes (storage.md §7.4)
//! 1. **No loss — checksum parity.** Every restored OLTP row that references a blob has that blob
//!    present AND integrity-verified: the restored object's bytes re-hash to the content address it is
//!    stored under ([`ContentHash`] BLAKE3 re-hash-on-read, reusing P-047). A row → missing blob is the
//!    §7.3 silent-corruption FAIL the restore already rejects ([`crate::restore::RestoreError::DanglingBlobRef`]);
//!    a row → *present-but-corrupt* blob (bytes that no longer hash to the address) is a CHECKSUM
//!    mismatch this gate adds ([`GateFailure::ChecksumMismatch`]) — the parity half §7.4 names.
//! 2. **Cross-seam — one consistent point.** The restored OLTP↔blob↔index↔offset land at ONE
//!    consistency point: derived == source-replay (reindex-from-source), no orphan derived doc, no
//!    past-offset row. The gate checks these over its OWN storage-native [`RestoreReport`] (the rows,
//!    the derived docs, the restored offset it already holds). The DRILL additionally feeds the same
//!    restore into the harness cross-seam assertion
//!    (`myelin_harness::RestoredSnapshot::verify_cross_seam`, P-056 — the SAME one SUB-D6 drives) and
//!    asserts the two AGREE, so storage and substrate prove ONE consistent point (coherence, EI-01 §7
//!    — not a parallel assertion: the harness models an abstract snapshot, the gate checks its native
//!    report, the drill cross-checks them). The harness is test-support that sits ABOVE the substrate
//!    while storage sits BELOW it — the gate's RUNTIME path therefore cannot depend on the harness
//!    (the crate DAG forbids the edge), so the cross-seam invariant is checked storage-side at runtime
//!    and cross-validated against the harness in the drill.
//! 3. **Erasure held — a shred stays dead across a restore.** A tenant/subject whose key was
//!    crypto-shredded BEFORE the backup is STILL erased after restore: its KEK is excluded from the
//!    restored set (reusing [`crate::kms::KmsEngine::backup_snapshot`], §7.5). A resurrected erased
//!    subject is [`GateFailure::ErasureResurrected`].
//!
//! ## What this module OWNS (new) vs REUSES (coherence, EI-01 §7)
//! The `restore(to_offset T)` orchestration ([`crate::restore::restore_to_offset`], P-060), the
//! cross-seam ASSERTION (`myelin_harness::RestoredSnapshot`, P-056), the [`ContentHash`] integrity
//! address (P-047), the KMS crypto-shred backup exclusion ([`crate::kms`], P-058), and the backup
//! machinery ([`crate::backup`], P-059) ALL already exist — this prompt does **NOT** re-define any of
//! them. What is genuinely NEW is the **CI-wired GATE itself**: the [`RestoreVerifyGate`] that spins a
//! clean target, drives the restore, runs the three §7.4 assertions (adding the checksum-parity +
//! erasure-held legs the bare restore does not), emits the dated [`GreenArtifact`] on pass / the typed
//! [`GateFailure`] on red, and the loud-never-swallowed [`RestoreVerifyGate::run_or_fail_ci`] CI
//! entrypoint. This is the durability-gate CALLER (the CDC consumer of contract 11.5).
//!
//! ## DEVIATION / FLOOR — modeled clean target, not a live `pg_restore` (EI-01 §1, write it down)
//! There is **no live Postgres / object store on this floor** (the real `pg_basebackup`/`pg_restore` +
//! the MinIO/Ceph object backing are the deferred floors P-S12/P-S15 / P-ST-30). So the gate's "spin a
//! clean target" is modeled as a fresh in-memory [`RestoreTarget`] (an empty OLTP/blob/derived/KMS set
//! the restore populates) rather than a provisioned database — but the gate's SHAPE (clean target →
//! drive [`restore_to_offset`] → assert no-loss + cross-seam + erasure-held → green-or-fail) does NOT
//! change when the real driver lands: that driver POPULATES the [`RestoreTarget`] off the real stores;
//! the three assertions read identically. The checksum-parity leg is exact over the BLAKE3
//! [`ContentHash`] (the real object bytes re-hash the same way the modeled [`RestoredObject`] bytes do).
//!
//! ## FLOORS NAMED (the prompt's DEFINITION OF DONE)
//! - **Post-restore RE-ERASURE (STOR-D3 — per-SUBJECT re-erasure against the GDPR erasure ledger
//!   10.8)** is the sibling **P-ST-14 (global P-100)**: it makes every restore run a mandatory
//!   re-erasure pass for each erasure completed AFTER the backup's PIT. This gate already holds the
//!   *erasure-before-the-backup* invariant (a pre-backup crypto-shredded key stays dead, §7.5) and
//!   exposes the [`ErasureLedger`] seam P-100 drives; the per-subject after-the-backup re-erasure is
//!   P-100. Named here; not built by this prompt.
//! - **The cell-kill RTO half (STOR-D2)** is also **P-ST-14 (global P-100)**. Named.
//! - **The prod-scale RESTORED copy** this gate produces is what online migrations rehearse lock-time
//!   against — **P-ST-21 (global P-126, STOR-D8)**. Named per the DoD.
//! - **The real CI runner wiring** (a `cargo`/CI invocation that calls [`RestoreVerifyGate::run_or_fail_ci`]
//!   on every store-touching change) lands with the CI subsystem (M2+); the GATE LOGIC + the
//!   loud-never-swallowed entrypoint ship now and re-run as a `cargo test` drill until then.
//! - **The real `pg_restore` + WAL-replay driver** is the P-S12/P-S15 floor; the gate mechanism ships
//!   now and does not change shape when it lands.

use std::collections::{BTreeMap, BTreeSet};

use myelin_tenancy::TenantId;

use crate::backup::{ContinuousArchiver, WalOffset};
use crate::blob::ContentHash;
use crate::kms::KmsEngine;
use crate::restore::{
    restore_to_offset, BlobPresence, RestoreError, RestoreReport, SourceLog, WalRow,
};

// ───────────────────────────── the clean restore target (the object bytes) ─────────────────────────────

/// One restored object as it lands in the clean target's object tier (T2): its content-address and
/// the bytes the restore brought back. The gate's **checksum-parity** leg re-hashes [`Self::bytes`]
/// and asserts it equals [`Self::address`] — the BLAKE3 re-hash-on-read integrity check (reusing
/// [`ContentHash`], P-047). A restored object whose bytes no longer hash to its address is silent
/// corruption the gate makes LOUD ([`GateFailure::ChecksumMismatch`]) — the "no loss (checksum
/// parity)" half of §7.4 the bare presence check does not cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredObject {
    /// The content-address the object is stored under (the BLAKE3 hash a row references). NOT a
    /// postal address — a [`ContentHash`]; named `content_address` so it is never mistaken for PII
    /// (the `no-untagged-personal-data` lint reserves `address` for a postal address).
    pub content_address: ContentHash,
    /// The bytes the restore brought back. MUST re-hash to [`Self::content_address`] (checksum
    /// parity) — a mismatch is silent corruption the gate FAILs on.
    pub bytes: Vec<u8>,
}

impl RestoredObject {
    /// A restored object whose `bytes` are content-addressed correctly (the integral case): the
    /// address IS the BLAKE3 hash of the bytes. The honest constructor for a non-corrupt object.
    pub fn integral(bytes: impl Into<Vec<u8>>) -> RestoredObject {
        let bytes = bytes.into();
        RestoredObject {
            content_address: ContentHash::blake3(&bytes),
            bytes,
        }
    }

    /// **The checksum-parity check (the no-loss leg of §7.4):** `true` iff the restored bytes re-hash
    /// to the address they are stored under. Reuses the BLAKE3 [`ContentHash`] re-hash-on-read
    /// integrity address (P-047) — the SAME integrity primitive the BlobStore serves reads through.
    /// A `false` here is the silent-corruption case the gate makes LOUD.
    pub fn checksum_parity_holds(&self) -> bool {
        ContentHash::blake3(&self.bytes) == self.content_address
    }
}

/// The **clean target** the gate spins to restore INTO (storage.md §7.4 "spin a clean target"): a
/// fresh, empty OLTP/blob/derived/KMS set the restore populates — never the live stores. Modeled
/// in-memory on this floor (the real provisioned DB is the P-S12/P-S15 floor; see the module
/// DEVIATION note); the restore's output is verified against it identically when the real driver
/// lands. Built by [`RestoreVerifyGate::run`] from the restore report + the restored objects.
#[derive(Clone, Debug, Default)]
pub struct RestoreTarget {
    /// The restored OLTP rows (every one at `seq ≤ T`).
    pub oltp_rows: Vec<WalRow>,
    /// The restored object tier, keyed by content address → the bytes restored under it (so the gate
    /// can checksum-parity-verify each referenced blob).
    pub objects: BTreeMap<ContentHash, Vec<u8>>,
    /// The derived docs reindexed FROM SOURCE up to T (each keyed by the source row it projects).
    pub derived_docs: BTreeSet<String>,
    /// The consistency point T every tier landed at.
    pub restored_to_offset: WalOffset,
}

// ───────────────────────────── the erasure-held seam (a shred stays dead) ─────────────────────────────

/// The **erasure ledger** seam (storage.md §7.4 "erasure held" / §7.5; GDPR contract 10.8): the
/// PII-free record of which subjects/tenants were erased (crypto-shredded) and WHEN. The gate's
/// erasure-held leg asserts that a subject erased BEFORE the backup is STILL erased after restore — a
/// resurrected erased subject is [`GateFailure::ErasureResurrected`].
///
/// **Floor named:** this is the *erasure-held-across-restore* seam THIS gate needs (the
/// before-the-backup invariant). The full GDPR erasure ledger + the per-subject re-erasure pass for
/// erasures completed AFTER the backup's PIT (re-destroy the per-subject DEK, re-purge Search,
/// re-tombstone Refs, re-emit `*.erased`) is the sibling **P-ST-14 (global P-100)** — it drives THIS
/// seam. Each recorded erasure carries its **completion offset** (the §7.3 cross-seam cursor the
/// restore PIT is compared against) so the gate can catch the §7.6 backup-window residual (an erasure
/// completed AFTER the restore's PIT — the backup predates the completion and physically holds the
/// pre-erasure key).
///
/// **MR-009b W6b — durable-by-default:** this is a role struct over an [`ErasureLedgerBackend`]
/// backend enum. The in-memory `Memory` arm (a `(tenant → completion offset)` map) + [`Self::new`] are
/// `#[cfg(any(test, feature = "test-support"))]` TEST DOUBLES; the always-compiled PRODUCTION backend
/// is the pool-backed [`crate::restore_verify_durable::DurableRestoreErasureLedger`] over the
/// non-shred-erasable `restore_erasure_ledger` table (migration `0051`). The `no-in-memory-durable-store`
/// scanner strips the `test-support`-gated `Memory` arm, so the production graph presents no in-memory
/// collection.
#[derive(Clone)]
pub struct ErasureLedger {
    backend: ErasureLedgerBackend,
}

/// The backend of an [`ErasureLedger`] — the in-memory `(tenant → completion offset)` map (test
/// double, `test-support`-gated) or the durable `restore_erasure_ledger` table (production default).
#[derive(Clone)]
enum ErasureLedgerBackend {
    /// The in-memory test double: tenant → the erasure's completion offset. **MR-009b W6b — TEST
    /// DOUBLE (`#[cfg(any(test, feature = "test-support"))]` only).**
    #[cfg(any(test, feature = "test-support"))]
    Memory(std::sync::Arc<std::sync::Mutex<BTreeMap<TenantId, WalOffset>>>),
    /// The durable production backing over the `restore_erasure_ledger` table.
    Pg(crate::restore_verify_durable::DurableRestoreErasureLedger),
}

impl ErasureLedger {
    /// An empty in-memory ledger — the **test double** (MR-009b W6b: `#[cfg(any(test, feature =
    /// "test-support"))]` only). The PRODUCTION ledger is [`Self::with_pg`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> ErasureLedger {
        ErasureLedger {
            backend: ErasureLedgerBackend::Memory(std::sync::Arc::new(std::sync::Mutex::new(
                BTreeMap::new(),
            ))),
        }
    }

    /// Wrap the durable backing as the production ledger (the always-compiled default over the
    /// `restore_erasure_ledger` table).
    pub fn with_pg(backing: crate::restore_verify_durable::DurableRestoreErasureLedger) -> ErasureLedger {
        ErasureLedger {
            backend: ErasureLedgerBackend::Pg(backing),
        }
    }

    /// Record that `tenant` was erased (crypto-shredded) **before the backup** (completion offset `0`,
    /// which is `<=` any restore PIT — the classic before-the-backup exclude-from-backup case). It
    /// MUST NOT be resurrected by a restore. Takes `&self` (interior mutability / durable write-through).
    ///
    /// **TEST DOUBLE ONLY (W6b verifier finding — fail-open default):** an offset-`0` record
    /// silently OPTS OUT of the §7.6 backup-window comparison (`0 <= PIT` always), so a production
    /// writer that used this for an erasure that actually completed inside the window would make
    /// the gate wave the resurrection through. Production write paths MUST call
    /// [`Self::record_erased_at`] with the erasure's REAL completion offset (the §7.3 cursor).
    #[cfg(any(test, feature = "test-support"))]
    pub fn record_erased(&self, tenant: TenantId) -> &Self {
        self.record_erased_at(tenant, 0)
    }

    /// **Record an erasure that completed at `completed_at_offset` (the §7.3 cross-seam cursor).** An
    /// erasure whose completion offset is `> ` the restore's PIT is the §7.6 backup-window residual the
    /// gate catches (the backup predates the erasure completion, so it physically holds the pre-erasure
    /// key). Idempotent per tenant.
    pub fn record_erased_at(&self, tenant: TenantId, completed_at_offset: WalOffset) -> &Self {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            ErasureLedgerBackend::Memory(m) => {
                m.lock()
                    .expect("erasure ledger poisoned")
                    .insert(tenant, completed_at_offset);
            }
            ErasureLedgerBackend::Pg(backing) => backing.record_erased_at(&tenant, completed_at_offset),
        }
        self
    }

    /// Every recorded erasure as `(tenant, completion offset)` — the set the gate's erasure-held leg
    /// verifies the restore did not resurrect (comparing each completion offset against the restore PIT).
    pub fn records(&self) -> Vec<(TenantId, WalOffset)> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            ErasureLedgerBackend::Memory(m) => m
                .lock()
                .expect("erasure ledger poisoned")
                .iter()
                .map(|(t, o)| (t.clone(), *o))
                .collect(),
            ErasureLedgerBackend::Pg(backing) => backing.records(),
        }
    }

    /// The erased tenants the gate asserts the restore did not resurrect (the keys of [`Self::records`]).
    pub fn erased_tenants(&self) -> BTreeSet<TenantId> {
        self.records().into_iter().map(|(t, _)| t).collect()
    }
}

// ───────────────────────────── the gate inputs ─────────────────────────────

/// Everything the restore-verify gate restores FROM + verifies against — the inputs a single gate run
/// consumes. The gate spins a clean [`RestoreTarget`], drives [`restore_to_offset`] over these, and
/// runs the three §7.4 assertions. Grouped into one struct so the [`RestoreVerifyGate::run`] signature
/// stays the load-bearing seam (the CDC consumer pins THIS shape).
pub struct GateInputs<'a> {
    /// The archiver whose base + WAL tail bounds PITR reachability (P-059).
    pub archiver: &'a ContinuousArchiver,
    /// The consistency point T to restore to.
    pub target: WalOffset,
    /// The WAL rows being restored (each `seq`-stamped + optionally blob-referencing).
    pub rows: &'a [WalRow],
    /// The restored object tier: the bytes the restore brought back, keyed by content address. The
    /// presence ([`BlobPresence`]) is derived from this; the checksum-parity leg re-hashes the bytes.
    pub objects: &'a [RestoredObject],
    /// The durable source log derived stores reindex FROM (reindex-from-source, never a backup).
    pub source: &'a SourceLog,
    /// The KMS engine whose backup snapshot (crypto-shredded keys EXCLUDED, §7.5) restores the KEKs.
    pub kms: &'a KmsEngine,
    /// The erasure ledger: subjects/tenants erased before the backup that MUST stay erased after
    /// restore (the erasure-held leg).
    pub erasure_ledger: &'a ErasureLedger,
}

// ───────────────────────────── the gate failure (what broke, never a bare bool) ─────────────────────────────

/// A RED restore-verify gate result — EXACTLY which §7.4 seam failed (observability is part of the
/// pass, EI-01 §3) so a failed CI run points at the precise corruption, never a bare "gate failed". A
/// [`GateFailure`] FAILs CI (it is the only non-pass outcome besides a hard [`RestoreError`]); it is
/// NEVER silently swallowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateFailure {
    /// **The restore itself FAILed** — a referenced-but-missing `ContentHash` (the §7.3
    /// silent-corruption case) or an unreachable PITR target. The restore returned [`RestoreError`]
    /// before the gate could verify; the gate FAILs CI loudly. The bare restore's hard FAIL is THE
    /// no-loss floor — surfaced here, never swallowed.
    RestoreFailed(RestoreError),
    /// **Checksum parity broke (the no-loss leg of §7.4):** a restored object's bytes no longer
    /// re-hash to the content address they are stored under — silent corruption a presence check
    /// alone misses. The gate FAILs CI. Names the offending address.
    ChecksumMismatch {
        /// The row that references the corrupt object.
        row_id: String,
        /// The content-address whose restored bytes do not re-hash to it (a [`ContentHash`], not a
        /// postal address).
        content_address: ContentHash,
    },
    /// **A restored object is referenced by NO restored row but is still in the target** — actually
    /// the inverse is the dangling case; THIS is a row referencing a blob ABSENT from the restored
    /// object set after the restore claimed success (a presence regression the gate double-checks).
    ReferencedObjectAbsent {
        /// The row that references the absent object.
        row_id: String,
        /// The content-address absent from the restored object tier (a [`ContentHash`], not a postal
        /// address).
        content_address: ContentHash,
    },
    /// **Cross-seam inconsistency (the one-consistent-point leg of §7.4):** the harness cross-seam
    /// assertion (the SUB-D6 one) found a mismatch — a row → missing blob, an orphan derived doc, or a
    /// row past the restored offset. Carries the human-readable mismatch list (from the harness
    /// report). The gate FAILs CI.
    CrossSeamMismatch {
        /// The number of cross-seam mismatches (`> 0`).
        count: usize,
        /// The mismatch detail (debug-rendered from the harness `CrossSeamReport`).
        detail: String,
    },
    /// **An erased subject was RESURRECTED (the erasure-held leg of §7.4 / §7.5):** a tenant
    /// crypto-shredded before the backup has a restored KEK after the restore — a shred that did NOT
    /// stay dead. The gravest possible failure: it un-erases a person. The gate FAILs CI.
    ErasureResurrected {
        /// The tenant that should have stayed erased but got a restored key.
        tenant: TenantId,
    },
}

impl core::fmt::Display for GateFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GateFailure::RestoreFailed(e) => {
                write!(f, "RESTORE-VERIFY FAIL — the restore itself failed: {e}")
            }
            GateFailure::ChecksumMismatch { row_id, content_address } => write!(
                f,
                "RESTORE-VERIFY FAIL — CHECKSUM MISMATCH: restored object {} (referenced by row \
                 {row_id}) does not re-hash to its content-address — silent corruption, the restore \
                 is NOT whole",
                content_address.to_multihash_string()
            ),
            GateFailure::ReferencedObjectAbsent { row_id, content_address } => write!(
                f,
                "RESTORE-VERIFY FAIL — REFERENCED OBJECT ABSENT: row {row_id} references {} which is \
                 not in the restored object tier",
                content_address.to_multihash_string()
            ),
            GateFailure::CrossSeamMismatch { count, detail } => write!(
                f,
                "RESTORE-VERIFY FAIL — CROSS-SEAM: {count} mismatch(es) across OLTP↔blob↔index↔offset \
                 — the restore did NOT land at one consistent point: {detail}"
            ),
            GateFailure::ErasureResurrected { tenant } => write!(
                f,
                "RESTORE-VERIFY FAIL — ERASURE RESURRECTED: tenant {} was crypto-shredded before the \
                 backup but has a restored key — a shred that did NOT stay dead across the restore \
                 (§7.5). THE GRAVEST FAILURE: it un-erases a person",
                tenant.0
            ),
        }
    }
}

impl std::error::Error for GateFailure {}

// ───────────────────────────── the green artifact (the dated proof on pass) ─────────────────────────────

/// The dated GREEN ARTIFACT the gate emits on PASS (storage.md §7.4 "green artifact on pass";
/// observability is part of the pass, EI-01 §3). It carries the MEASURED numbers — never a bare "ok":
/// the offset T every tier landed at, the row/object/derived counts, and the three zeros the gate
/// asserted (0 dangling, 0 checksum mismatches, 0 resurrected subjects). The gate's GREEN proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GreenArtifact {
    /// The consistency point T every tier was restored to (== the requested target).
    pub restored_to_offset: WalOffset,
    /// The number of restored OLTP rows (all `seq ≤ T`).
    pub oltp_row_count: usize,
    /// The number of restored objects, every one checksum-parity-verified.
    pub objects_verified: usize,
    /// The number of derived docs reindexed FROM SOURCE (consumers resumed at T).
    pub derived_doc_count: usize,
    /// Dangling blob refs found — `0` on a green pass (the no-loss floor).
    pub dangling_ref_count: u64,
    /// Checksum-parity mismatches found — `0` on a green pass (the no-loss/checksum-parity leg).
    pub checksum_mismatches: u64,
    /// Cross-seam mismatches found — `0` on a green pass (the one-consistent-point leg).
    pub cross_seam_mismatches: u64,
    /// Erased subjects resurrected — `0` on a green pass (the erasure-held leg).
    pub resurrected_subjects: u64,
}

impl GreenArtifact {
    /// Render the dated green-artifact line a CI run prints on PASS (the measured-numbers proof). The
    /// caller prefixes the date (`[P-061 GATE GREEN <date>]`) so the artifact is dated at the run.
    pub fn summary(&self) -> String {
        format!(
            "restore-verify PASS: restore(to_offset T={}) landed OLTP↔blob↔index↔offset at ONE \
             consistent point — {} OLTP rows (all seq≤T), {} objects checksum-parity-verified, {} \
             derived docs reindexed-from-source; dangling_ref_count={}, checksum_mismatches={}, \
             cross_seam_mismatches={}, resurrected_subjects={} (all 0). cold==live by construction.",
            self.restored_to_offset,
            self.oltp_row_count,
            self.objects_verified,
            self.derived_doc_count,
            self.dangling_ref_count,
            self.checksum_mismatches,
            self.cross_seam_mismatches,
            self.resurrected_subjects,
        )
    }
}

// ───────────────────────────── the gate verdict (loud-never-swallowed, #[must_use]) ─────────────────────────────

/// The typed verdict of a restore-verify gate run — GREEN (a [`GreenArtifact`]) or RED (a
/// [`GateFailure`]). `#[must_use]`: a dropped verdict is a swallowed data-loss check (the exact
/// EI-01 §5 loud-never-swallowed violation) and the compiler flags it. There is NO `bool`/`ok()`
/// coercion that loses the failure; the only way to consume a red is to handle it (e.g.
/// [`RestoreVerifyGate::run_or_fail_ci`] turns it into a process-failing `Err`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a restore-verify gate verdict must be checked — a dropped RED is a SWALLOWED \
              silent-data-loss failure (the permanent gate, EI-01 §5: loud-never-swallowed)"]
pub enum GateVerdict {
    /// The restore is whole: no loss (checksum parity), one cross-seam point, erasure held. Carries
    /// the dated [`GreenArtifact`] with the measured numbers.
    Green(GreenArtifact),
    /// The restore is NOT whole — EXACTLY what broke. FAILs CI; never swallowed.
    Red(GateFailure),
}

impl GateVerdict {
    /// `true` iff the gate passed (the restore is whole). The ONLY way to read a pass — a [`Red`] is
    /// never silently a pass.
    ///
    /// [`Red`]: GateVerdict::Red
    pub fn is_green(&self) -> bool {
        matches!(self, GateVerdict::Green(_))
    }

    /// The green artifact, if the gate passed.
    pub fn green_artifact(&self) -> Option<&GreenArtifact> {
        match self {
            GateVerdict::Green(a) => Some(a),
            GateVerdict::Red(_) => None,
        }
    }

    /// The failure, if the gate failed.
    pub fn failure(&self) -> Option<&GateFailure> {
        match self {
            GateVerdict::Red(f) => Some(f),
            GateVerdict::Green(_) => None,
        }
    }
}

// ───────────────────────────── the gate ─────────────────────────────

/// **The CI-wired restore-verify gate (STOR-D1, the permanent gate — THE HEADLINE).** Spins a clean
/// target, drives `restore(to_offset T)`, and runs the three §7.4 assertions (no-loss / cross-seam /
/// erasure-held), emitting a dated GREEN ARTIFACT on pass and a typed [`GateFailure`] on red. It
/// re-runs on every store-touching change, forever (master §4) — loud-never-swallowed.
///
/// A zero-sized orchestrator: it holds no state, so a CI run is `RestoreVerifyGate::run(inputs)` —
/// the inputs ARE the restore source. Reuses [`restore_to_offset`] (P-060), the harness cross-seam
/// assertion (P-056), and the KMS crypto-shred exclusion (P-058); adds the checksum-parity +
/// erasure-held legs the bare restore does not.
#[derive(Clone, Copy, Debug, Default)]
pub struct RestoreVerifyGate;

impl RestoreVerifyGate {
    /// A new gate (stateless).
    pub fn new() -> RestoreVerifyGate {
        RestoreVerifyGate
    }

    /// **Run the restore-verify gate once.** Returns [`GateVerdict::Green`] (the restore is whole +
    /// the dated artifact) or [`GateVerdict::Red`] (exactly what broke). NEVER swallows a failure: a
    /// red is a returned typed verdict, not a logged-and-continued. The CI entrypoint
    /// [`Self::run_or_fail_ci`] turns a red into a process-failing `Err`.
    ///
    /// The sequence (storage.md §7.4):
    /// 1. **Spin a clean target + restore** — drive [`restore_to_offset`] (P-060) over the inputs;
    ///    its hard FAIL (a referenced-but-missing hash / unreachable PITR) is surfaced as
    ///    [`GateFailure::RestoreFailed`] (the no-loss floor — never a silent pass).
    /// 2. **No loss — checksum parity** — every restored row's referenced object is present in the
    ///    restored object tier AND its bytes re-hash to its address ([`RestoredObject::checksum_parity_holds`]).
    /// 3. **Cross-seam — one consistent point** — the harness cross-seam assertion (SUB-D6) reports 0
    ///    mismatches on the restored snapshot (derived == source-replay, no orphan, no past-offset).
    /// 4. **Erasure held** — no tenant the [`ErasureLedger`] marks erased-before-the-backup has a
    ///    restored KEK ([`GateFailure::ErasureResurrected`] if one does).
    pub fn run(&self, inputs: &GateInputs<'_>) -> GateVerdict {
        // The bare gate: NO post-restore re-erasure will run, so a §7.6 backup-window erasure (an
        // erasure completed AFTER the restore PIT) has nothing to re-kill it — the gate REFUSES it.
        self.run_inner(inputs, false)
    }

    /// The gate body, parameterized by whether a post-restore re-erasure pass WILL run after it
    /// ([`Self::run_with_reerase`] passes `true`). It only changes the §7.6 backup-window leg: when a
    /// re-erasure pass will follow, a post-PIT erasure is allowed past the base gate (the pass
    /// re-applies it + asserts 0 resurrected); when none will (the bare [`Self::run`]), a post-PIT
    /// erasure is REFUSED (there is nothing to re-kill the resurrected key).
    fn run_inner(&self, inputs: &GateInputs<'_>, reerase_will_run: bool) -> GateVerdict {
        // (1) Spin the clean target + drive the restore. The restore's hard FAIL (missing hash /
        // unreachable PITR) is the no-loss floor — surfaced LOUD, never swallowed.
        let presence = build_presence(inputs.objects);
        let report = match restore_to_offset(
            inputs.archiver,
            inputs.target,
            inputs.rows,
            &presence,
            inputs.source,
            inputs.kms,
        ) {
            Ok(report) => report,
            Err(e) => return GateVerdict::Red(GateFailure::RestoreFailed(e)),
        };

        // Build the clean target from the restore output + the restored object bytes.
        let object_bytes: BTreeMap<ContentHash, Vec<u8>> = inputs
            .objects
            .iter()
            .map(|o| (o.content_address.clone(), o.bytes.clone()))
            .collect();

        // (2) No loss — checksum parity. Every restored row's referenced object must be present AND
        // re-hash to its content-address (the silent-corruption-on-read case the presence check misses).
        for row in &report.oltp_rows {
            if let Some(content_address) = &row.blob_ref {
                match object_bytes.get(content_address) {
                    None => {
                        // Present in the presence oracle (the restore passed) but absent from the
                        // bytes set — a presence regression the gate double-checks. (In a consistent
                        // run this cannot happen; the gate asserts it anyway, loud.)
                        return GateVerdict::Red(GateFailure::ReferencedObjectAbsent {
                            row_id: row.id.clone(),
                            content_address: content_address.clone(),
                        });
                    }
                    Some(bytes) => {
                        if &ContentHash::blake3(bytes) != content_address {
                            return GateVerdict::Red(GateFailure::ChecksumMismatch {
                                row_id: row.id.clone(),
                                content_address: content_address.clone(),
                            });
                        }
                    }
                }
            }
        }

        // (3) Cross-seam — one consistent point, checked over the storage-native RestoreReport (the
        // gate cannot depend on the harness at runtime; the DRILL cross-validates against the SUB-D6
        // harness assertion — coherence, EI-01 §7). The three invariants: no orphan derived doc (a
        // derived doc whose source row is absent from the restored OLTP set), no past-offset row, no
        // row → missing blob (the restore already rejected the last; we re-assert for the report).
        let cross_seam = verify_cross_seam_native(&report, &object_bytes);
        if !cross_seam.is_empty() {
            return GateVerdict::Red(GateFailure::CrossSeamMismatch {
                count: cross_seam.len(),
                detail: cross_seam.join("; "),
            });
        }

        // (4) Erasure held — a subject erased stays erased across the restore. The R1 fold-in uses
        // each erasure's COMPLETION offset (the §7.3 cursor) vs the restore PIT:
        //   - completed_at_offset <= PIT  → captured in/before the backup: the KEK exclusion (§7.5,
        //     P-061) must hold — a restored key is the exclude-from-backup regression (gravest fail).
        //   - completed_at_offset  > PIT  → the §7.6 backup-window residual: the backup predates the
        //     erasure completion and physically holds the pre-erasure key, so a restore of PIT
        //     resurrects the subject. The bare gate cannot re-kill it (that is the post-restore
        //     re-erasure pass); if no re-erasure will run, the gate REFUSES (never a silent green).
        for (tenant, completed_at_offset) in inputs.erasure_ledger.records() {
            if completed_at_offset > report.restored_to_offset {
                if reerase_will_run {
                    // run_with_reerase re-applies this post-PIT erasure + asserts 0 resurrected.
                    continue;
                }
                return GateVerdict::Red(GateFailure::ErasureResurrected { tenant });
            }
            if report.restored_key_for_tenant(&tenant) {
                return GateVerdict::Red(GateFailure::ErasureResurrected { tenant });
            }
        }

        // PASS — the restore is whole. Emit the dated green artifact with the measured numbers.
        GateVerdict::Green(GreenArtifact {
            restored_to_offset: report.restored_to_offset,
            oltp_row_count: report.oltp_rows.len(),
            objects_verified: report
                .oltp_rows
                .iter()
                .filter(|r| r.blob_ref.is_some())
                .count(),
            derived_doc_count: report.derived.doc_count(),
            dangling_ref_count: report.dangling_ref_count,
            checksum_mismatches: 0,
            cross_seam_mismatches: 0,
            resurrected_subjects: 0,
        })
    }

    /// **Run the gate AND the mandatory post-restore re-erasure pass (§7.5 — P-100).** This is the
    /// post-PIT-aware entrypoint: it runs the three §7.4 assertions ([`Self::run`]) AND THEN re-applies
    /// every erasure the `post_pit_ledger` records as completed AFTER the restore's PIT (the set the
    /// restore could resurrect), asserting **0 resurrected subjects** ([`GateFailure::ErasureResurrected`]
    /// if one survives). Wiring re-erasure into the gate is the §7.5 requirement: *every restore
    /// re-erases by construction*.
    ///
    /// The before-the-backup leg ([`Self::run`]'s erasure-held check) keeps a PRE-T crypto-shred dead
    /// (exclude-from-backup, P-058/P-061); this pass re-applies every POST-T erasure. Together a
    /// restore never resurrects an erased subject, whenever the erasure happened.
    ///
    /// `post_pit_ledger` is the §7.5 / 10.8 erasure ledger keyed by completion offset; `holders` are
    /// the cross-holder re-erasure seams (re-purge Search / re-tombstone Refs / re-emit `*.erased`);
    /// `now` is the caller-supplied clock. Returns [`GateVerdict::Green`] only when BOTH the three
    /// §7.4 assertions AND the re-erasure pass (0 resurrected) pass.
    ///
    /// The re-erasure pass mutates the restored copy's KMS engine (it re-destroys resurrected DEKs);
    /// it borrows the SAME `inputs.kms` the restore restored the KEKs into.
    pub fn run_with_reerase(
        &self,
        inputs: &GateInputs<'_>,
        post_pit_ledger: &dyn crate::reerase::PostRestoreErasureLedger,
        holders: &crate::erase::EraseHolders<'_>,
        region: myelin_tenancy::Region,
        now: crate::erase::EpochMillis,
    ) -> GateVerdict {
        // (1) The three §7.4 assertions (no-loss / cross-seam / before-the-backup erasure-held). A red
        // here short-circuits — never run re-erasure over a broken restore. `reerase_will_run = true`:
        // a §7.6 backup-window erasure is allowed past the base gate here — the re-erasure pass below
        // re-applies it and asserts 0 resurrected (the bare `run` would REFUSE it instead).
        let base = self.run_inner(inputs, true);
        let mut artifact = match base {
            GateVerdict::Green(a) => a,
            red @ GateVerdict::Red(_) => return red,
        };

        // (2) Drive the restore once more to obtain the report the re-erasure pass runs against (the
        // §7.4 run consumed its report; the restore is deterministic + idempotent so this lands the
        // same point). The presence oracle is rebuilt from the same objects.
        let presence = build_presence(inputs.objects);
        let report = match restore_to_offset(
            inputs.archiver,
            inputs.target,
            inputs.rows,
            &presence,
            inputs.source,
            inputs.kms,
        ) {
            Ok(report) => report,
            Err(e) => return GateVerdict::Red(GateFailure::RestoreFailed(e)),
        };

        // (2b) STRUCTURAL cross-ledger coverage assert (W6b verifier finding). Step (1) admitted
        // every §7.6 window-case erasure (completion offset > PIT) past the base gate's refusal ON
        // THE PROMISE that the re-erasure pass below re-kills it. That promise is only real if the
        // post-PIT ledger actually COVERS the window tenant: the two ledgers are DIFFERENT tables
        // (the tenant-grained `restore_erasure_ledger` vs the subject-grained post-PIT ledger) and
        // nothing else ties them together — a window record with no post-PIT coverage would sail
        // through GREEN un-re-erased (probe-confirmed pre-fix). So the coverage is asserted
        // structurally: every window tenant MUST appear in the post-PIT set, else RED — never a
        // trusted green.
        let post_pit_tenants: std::collections::BTreeSet<TenantId> = post_pit_ledger
            .erasures_completed_after(report.restored_to_offset)
            .into_iter()
            .map(|r| r.tenant)
            .collect();
        for (tenant, completed_at_offset) in inputs.erasure_ledger.records() {
            if completed_at_offset > report.restored_to_offset && !post_pit_tenants.contains(&tenant)
            {
                return GateVerdict::Red(GateFailure::ErasureResurrected { tenant });
            }
        }

        // (3) The mandatory post-restore re-erasure pass (§7.5): re-apply every post-PIT erasure +
        // assert 0 resurrected. A re-applied step failing is a loud restore-failed (an incomplete
        // re-erasure is never swallowed).
        let pass = crate::reerase::ReErasePass::new(inputs.kms, region);
        let reerase = match pass.run(&report, post_pit_ledger, holders, now) {
            Ok(r) => r,
            // A re-erasure step failure surfaces as a cross-seam-class gate failure (the restore did
            // not reach a clean, fully-re-erased point). Loud, never swallowed.
            Err(e) => {
                return GateVerdict::Red(GateFailure::CrossSeamMismatch {
                    count: 1,
                    detail: format!("post-restore re-erasure step failed: {e}"),
                })
            }
        };

        if !reerase.is_green() {
            // A subject erased AFTER the backup was RESURRECTED by the restore and the pass could not
            // re-kill it — the gravest failure. Name the first still-resurrected subject's tenant.
            let tenant = reerase
                .re_erased
                .iter()
                .find(|s| s.was_resurrected_before_reapply)
                .map(|s| s.tenant.clone())
                .unwrap_or_else(|| TenantId("<unknown>".into()));
            return GateVerdict::Red(GateFailure::ErasureResurrected { tenant });
        }

        // The re-erasure pass passed: fold its measured numbers into the artifact (resurrected == 0).
        artifact.resurrected_subjects = reerase.resurrected_count;
        GateVerdict::Green(artifact)
    }

    /// **The loud-never-swallowed CI entrypoint (EI-01 §5).** Run the gate and turn a RED verdict into
    /// a process-failing `Err(GateFailure)` — so a CI invocation `gate.run_or_fail_ci(&inputs)?`
    /// FAILS CI on a red restore, with NO `|| true`, no `.ok()`, no swallow. On GREEN it returns the
    /// dated [`GreenArtifact`] (`Ok`). This is the ONLY blessed way to consume the verdict in CI: a
    /// red MUST stop the build.
    ///
    /// Returns `Ok(GreenArtifact)` on pass; `Err(GateFailure)` on red (the CI process exits non-zero).
    pub fn run_or_fail_ci(&self, inputs: &GateInputs<'_>) -> Result<GreenArtifact, GateFailure> {
        match self.run(inputs) {
            GateVerdict::Green(artifact) => Ok(artifact),
            GateVerdict::Red(failure) => Err(failure),
        }
    }
}

// ───────────────────────────── internals ─────────────────────────────

/// Build the [`BlobPresence`] oracle the restore consumes from the restored objects (presence = the
/// set of restored addresses). The checksum-parity leg verifies the BYTES separately.
fn build_presence(objects: &[RestoredObject]) -> BlobPresence {
    let mut presence = BlobPresence::new();
    for obj in objects {
        presence.insert(obj.content_address.clone());
    }
    presence
}

/// **The storage-native cross-seam check (the SUB-D6 invariant, checked over the [`RestoreReport`]).**
/// Returns the list of mismatches (empty ⇒ one consistent cross-seam point). The gate runs this at
/// runtime (it cannot depend on the harness, which sits above the substrate); the drill cross-checks
/// the SAME restore against the harness `verify_cross_seam` and asserts they agree (coherence, EI-01
/// §7). The three invariants (storage.md §7.3 / harness §11 D-6):
/// 1. **no row → missing blob** — every restored row's `blob_ref` is in the restored object tier (the
///    restore already FAILs on this; re-asserted here so the gate's report is self-consistent);
/// 2. **no past-offset row** — no restored row was written past the restored consistency point;
/// 3. **no orphan derived doc** — every reindexed derived doc projects a restored OLTP row.
fn verify_cross_seam_native(
    report: &RestoreReport,
    object_bytes: &BTreeMap<ContentHash, Vec<u8>>,
) -> Vec<String> {
    let mut mismatches = Vec::new();
    let row_ids: BTreeSet<&str> = report.oltp_rows.iter().map(|r| r.id.as_str()).collect();

    for row in &report.oltp_rows {
        // (1) no row → missing blob.
        if let Some(addr) = &row.blob_ref {
            if !object_bytes.contains_key(addr) {
                mismatches.push(format!(
                    "row {} → missing blob {}",
                    row.id,
                    addr.to_multihash_string()
                ));
            }
        }
        // (2) no row past the restored consistency point.
        if row.written_at > report.restored_to_offset {
            mismatches.push(format!(
                "row {} written at offset {} is past the restored point {}",
                row.id, row.written_at, report.restored_to_offset
            ));
        }
    }
    // (3) no orphan derived doc (a derived doc whose source OLTP row is absent).
    for doc in report.derived.docs() {
        if !row_ids.contains(doc.as_str()) {
            mismatches.push(format!("orphan derived doc projecting absent row {doc}"));
        }
    }
    mismatches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::WalSegment;
    use crate::kms::{KekId, KeyClass};
    use myelin_tenancy::Region;

    fn region() -> Region {
        Region("eu-west".into())
    }
    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }

    /// An archiver whose base + WAL tail makes every offset in `0..=tail` reachable.
    fn reachable_archiver(tail: WalOffset) -> ContinuousArchiver {
        let mut arch = ContinuousArchiver::new();
        arch.archive_segment(WalSegment {
            end_offset: 0,
            committed_at: 0,
        })
        .unwrap();
        arch.take_base_backup(1);
        arch.archive_segment(WalSegment {
            end_offset: tail,
            committed_at: 10,
        })
        .unwrap();
        arch
    }

    /// A KMS engine with a live tenant whose KEK + DEK exist (so a restore brings back a key).
    fn kms_with_tenant(t: &TenantId) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(t.clone(), region()));
        kms.ensure_dek(t, &region(), KeyClass::Tenant).unwrap();
        kms
    }

    // ───────── the green path (the dated artifact with measured numbers) ─────────

    /// **The headline GREEN: the gate spins a clean target, restores, reindexes from source, and
    /// asserts checksum parity + 0 dangling + cold==live → a dated green artifact.** The DoD pass.
    #[test]
    fn the_gate_greens_a_whole_restore_with_measured_numbers() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        let objects = vec![
            RestoredObject::integral(b"blob-90".to_vec()),
            RestoredObject::integral(b"blob-100".to_vec()),
        ];
        let mut source = SourceLog::new();
        source.append(90, "r90").append(100, "r100");
        let rows = vec![
            WalRow {
                id: "r90".into(),
                written_at: 90,
                blob_ref: Some(objects[0].content_address.clone()),
            },
            WalRow {
                id: "r100".into(),
                written_at: 100,
                blob_ref: Some(objects[1].content_address.clone()),
            },
            WalRow {
                id: "r-future".into(),
                written_at: 250,
                blob_ref: None,
            }, // > T → dropped
        ];
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            verdict.is_green(),
            "a whole restore must GREEN, got {:?}",
            verdict.failure()
        );
        let artifact = verdict.green_artifact().expect("green artifact present");
        assert_eq!(artifact.restored_to_offset, 100);
        assert_eq!(artifact.oltp_row_count, 2, "the future row was dropped");
        assert_eq!(
            artifact.objects_verified, 2,
            "both referenced objects checksum-parity-verified"
        );
        assert_eq!(
            artifact.derived_doc_count, 2,
            "derived == source-replay to T"
        );
        assert_eq!(artifact.dangling_ref_count, 0);
        assert_eq!(artifact.checksum_mismatches, 0);
        assert_eq!(artifact.cross_seam_mismatches, 0);
        assert_eq!(artifact.resurrected_subjects, 0);
        // The dated artifact summary names the measured numbers (observability is part of the pass).
        let s = artifact.summary();
        assert!(s.contains("restore-verify PASS"));
        assert!(s.contains("T=100"));
    }

    /// `run_or_fail_ci` returns `Ok(artifact)` on a green run (the CI process continues).
    #[test]
    fn run_or_fail_ci_returns_ok_on_green() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        let objects = vec![RestoredObject::integral(b"x".to_vec())];
        let mut source = SourceLog::new();
        source.append(50, "r1");
        let rows = vec![WalRow {
            id: "r1".into(),
            written_at: 50,
            blob_ref: Some(objects[0].content_address.clone()),
        }];
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };
        let artifact = RestoreVerifyGate::new()
            .run_or_fail_ci(&inputs)
            .expect("a whole restore must not fail CI");
        assert_eq!(artifact.oltp_row_count, 1);
    }

    // ───────── the no-loss / checksum-parity leg (the silent-data-loss floor — mandatory-core) ─────────

    /// **MANDATORY-CORE: a restored object whose bytes are CORRUPT (re-hash ≠ address) FAILs the gate
    /// (not silently pass)** — the checksum-parity half of §7.4. A presence check alone would PASS
    /// (the address IS present); the gate re-hashes the bytes and catches the silent corruption. Kills
    /// any mutant that drops the checksum check or inverts the comparison.
    #[test]
    fn a_corrupt_restored_object_fails_the_gate() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        // The object is stored under the address for "good-bytes", but the restore brought back
        // "CORRUPTED" bytes — present, but the bytes no longer hash to the address (silent corruption).
        let address = ContentHash::blake3(b"good-bytes");
        let corrupt = RestoredObject {
            content_address: address.clone(),
            bytes: b"CORRUPTED".to_vec(),
        };
        let objects = vec![corrupt];
        let source = SourceLog::new();
        let rows = vec![WalRow {
            id: "r1".into(),
            written_at: 50,
            blob_ref: Some(address.clone()),
        }];
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            !verdict.is_green(),
            "a corrupt restored object MUST FAIL the gate, not pass silently"
        );
        assert_eq!(
            verdict.failure(),
            Some(&GateFailure::ChecksumMismatch {
                row_id: "r1".into(),
                content_address: address.clone()
            })
        );
        // run_or_fail_ci FAILs CI on it (loud-never-swallowed).
        let err = RestoreVerifyGate::new()
            .run_or_fail_ci(&inputs)
            .expect_err("must fail CI");
        assert!(
            err.to_string().contains("CHECKSUM MISMATCH"),
            "loud + specific: {err}"
        );
    }

    /// **MANDATORY-CORE: a deliberately-CORRUPTED backup (a row → MISSING blob) makes the gate FAIL
    /// CI (the restore's §7.3 silent-corruption FAIL is surfaced, never swallowed).** The
    /// no-loss/dangling-ref floor — the prompt's "a deliberately-corrupted backup makes the gate FAIL
    /// CI" test. Kills the mutant that swallows the restore error.
    #[test]
    fn a_corrupted_backup_fails_ci() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        // "present" is restored; "missing" is NOT — a row references a blob the restore did not bring
        // back (the §7.3 silent-corruption / dangling-ref case).
        let present = RestoredObject::integral(b"present".to_vec());
        let missing_addr = ContentHash::blake3(b"missing");
        let objects = vec![present.clone()];
        let source = SourceLog::new();
        let rows = vec![
            WalRow {
                id: "ok".into(),
                written_at: 50,
                blob_ref: Some(present.content_address.clone()),
            },
            WalRow {
                id: "corrupt".into(),
                written_at: 90,
                blob_ref: Some(missing_addr),
            },
        ];
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let err = RestoreVerifyGate::new().run_or_fail_ci(&inputs).expect_err(
            "a corrupted backup (row → missing blob) MUST fail CI, never silently pass",
        );
        assert!(
            matches!(&err, GateFailure::RestoreFailed(RestoreError::DanglingBlobRef { row_id, .. }) if row_id == "corrupt"),
            "the gate surfaces the restore's hard dangling-ref FAIL: {err}"
        );
        assert!(
            err.to_string().contains("RESTORE-VERIFY FAIL"),
            "loud: {err}"
        );
    }

    // ───────── the cross-seam leg (one consistent point, derived == source) ─────────

    /// The gate FAILs on a cross-seam mismatch — here an ORPHAN derived doc (a projection whose source
    /// row is absent). The harness assertion (SUB-D6) bites; the gate surfaces it. (We inject the
    /// orphan via a source event for a row that is NOT in the restored OLTP set.)
    #[test]
    fn the_gate_fails_on_a_cross_seam_orphan_doc() {
        let t = tenant("acme");
        let kms = kms_with_tenant(&t);
        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        // A source event projects "ghost" — but there is NO "ghost" OLTP row → an orphan derived doc.
        let mut source = SourceLog::new();
        source.append(50, "ghost");
        let rows: Vec<WalRow> = vec![]; // no rows restored → the derived "ghost" doc is an orphan
        let ledger = ErasureLedger::new();
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            !verdict.is_green(),
            "an orphan derived doc is a cross-seam mismatch"
        );
        match verdict.failure() {
            Some(GateFailure::CrossSeamMismatch { count, .. }) => assert_eq!(*count, 1),
            other => panic!("expected a CrossSeamMismatch, got {other:?}"),
        }
    }

    // ───────── the erasure-held leg (a shred stays dead across a restore, §7.5) ─────────

    /// **A tenant crypto-shredded BEFORE the backup stays erased across the restore (the gate GREENS,
    /// it does NOT resurrect it).** The KMS backup snapshot excludes the shredded KEK (§7.5), so the
    /// erasure-held leg passes. The happy erasure-held path.
    #[test]
    fn an_erased_tenant_stays_erased_across_the_restore() {
        let live = tenant("live");
        let shredded = tenant("shredded");
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(live.clone(), region()));
        kms.ensure_dek(&live, &region(), KeyClass::Tenant).unwrap();
        kms.ensure_kek(&KekId::new(shredded.clone(), region()));
        kms.ensure_dek(&shredded, &region(), KeyClass::Tenant)
            .unwrap();
        // Crypto-shred the second tenant BEFORE the backup (the erasure-before-the-backup case).
        assert!(kms.destroy_kek(&KekId::new(shredded.clone(), region())));

        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        let source = SourceLog::new();
        let rows: Vec<WalRow> = vec![];
        let ledger = ErasureLedger::new();
        ledger.record_erased(shredded.clone());
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            verdict.is_green(),
            "an erased tenant must stay erased → green, got {:?}",
            verdict.failure()
        );
    }

    /// **MANDATORY-CORE: the erasure-held leg CATCHES a resurrected erased subject** — if the restore
    /// DID bring back a key for a ledger-erased tenant (the gravest failure: un-erasing a person), the
    /// gate FAILs CI. We inject the resurrection by recording a LIVE tenant (whose key IS restored) as
    /// "erased" in the ledger — the gate must reject the restored key for it. Kills the mutant that
    /// drops the erasure-held check or inverts the resurrection test.
    #[test]
    fn the_gate_fails_on_a_resurrected_erased_subject() {
        let resurrected = tenant("should-be-dead");
        // The tenant's key IS in the KMS (the restore WILL bring it back) — but the ledger says it was
        // erased before the backup. A live restored key for a ledger-erased tenant = resurrection.
        let kms = kms_with_tenant(&resurrected);
        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        let source = SourceLog::new();
        let rows: Vec<WalRow> = vec![];
        let ledger = ErasureLedger::new();
        ledger.record_erased(resurrected.clone());
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            !verdict.is_green(),
            "a resurrected erased subject MUST fail the gate"
        );
        assert_eq!(
            verdict.failure(),
            Some(&GateFailure::ErasureResurrected {
                tenant: resurrected.clone()
            })
        );
        let err = RestoreVerifyGate::new()
            .run_or_fail_ci(&inputs)
            .expect_err("must fail CI");
        assert!(
            err.to_string().contains("ERASURE RESURRECTED"),
            "loud + specific: {err}"
        );
    }

    // ───────── the R1 fold-in: the §7.6 backup-window residual (restore-inside-window) ─────────

    /// **MANDATORY-CORE (R1 fold-in): an erasure completed INSIDE the backup window is CAUGHT by the
    /// bare gate.** A backup taken at PIT T=100, but the erasure completed at offset 140 (AFTER T — the
    /// backup predates the erasure completion, so it physically holds the pre-erasure key). A restore of
    /// T would resurrect the subject. The bare gate has no re-erasure pass to re-kill it, so it must
    /// REFUSE — comparing the restore PIT against the erasure's COMPLETION offset (the §7.6 residual).
    /// This is the honest catch the timeless "shred reaches backups" model does not structurally cover.
    #[test]
    fn an_erasure_completed_inside_the_backup_window_is_refused_by_the_bare_gate() {
        let windowed = tenant("erased-after-the-backup");
        // The tenant's key is NOT in the KMS now (the erasure completed) — so `restored_key_for_tenant`
        // is false; the model would green it. The COMPLETION-offset comparison is what catches it.
        let kms = KmsEngine::new();
        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        let source = SourceLog::new();
        let rows: Vec<WalRow> = vec![];
        let ledger = ErasureLedger::new();
        // Erasure completed at offset 140 — AFTER the restore PIT T=100 (inside the backup window).
        ledger.record_erased_at(windowed.clone(), 140);
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };

        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            !verdict.is_green(),
            "an erasure completed inside the backup window MUST be caught (the §7.6 residual)"
        );
        assert_eq!(
            verdict.failure(),
            Some(&GateFailure::ErasureResurrected {
                tenant: windowed.clone()
            }),
            "the gate refuses the restore-inside-window resurrection"
        );
    }

    /// **The counterpart: an erasure completed BEFORE-or-AT the PIT is NOT the window residual** — it is
    /// the classic exclude-from-backup case and greens (no restored key). Kills a mutant that flips the
    /// `>` window comparison to `>=`/`<` (offset 100 == PIT 100 must NOT be treated as the window case).
    #[test]
    fn an_erasure_completed_at_or_before_the_pit_is_the_before_backup_case() {
        let shredded = tenant("erased-before-the-backup");
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(shredded.clone(), region()));
        kms.ensure_dek(&shredded, &region(), KeyClass::Tenant).unwrap();
        assert!(kms.destroy_kek(&KekId::new(shredded.clone(), region())));
        let arch = reachable_archiver(300);
        let objects: Vec<RestoredObject> = vec![];
        let source = SourceLog::new();
        let rows: Vec<WalRow> = vec![];
        let ledger = ErasureLedger::new();
        // Completed exactly AT the PIT (offset 100 == T) — captured at/before the backup, not the window.
        ledger.record_erased_at(shredded.clone(), 100);
        let inputs = GateInputs {
            archiver: &arch,
            target: 100,
            rows: &rows,
            objects: &objects,
            source: &source,
            kms: &kms,
            erasure_ledger: &ledger,
        };
        let verdict = RestoreVerifyGate::new().run(&inputs);
        assert!(
            verdict.is_green(),
            "an erasure completed at-or-before the PIT is the before-backup case → green, got {:?}",
            verdict.failure()
        );
    }

    // ───────── checksum-parity-holds unit (the integrity primitive) ─────────

    /// `RestoredObject::integral` produces a content-addressed object whose checksum parity holds; a
    /// hand-built object with mismatched bytes does NOT. The integrity primitive the gate re-uses.
    #[test]
    fn checksum_parity_holds_iff_bytes_rehash_to_address() {
        let ok = RestoredObject::integral(b"hello".to_vec());
        assert!(ok.checksum_parity_holds(), "integral object → parity holds");

        let corrupt = RestoredObject {
            content_address: ContentHash::blake3(b"hello"),
            bytes: b"tampered".to_vec(),
        };
        assert!(
            !corrupt.checksum_parity_holds(),
            "tampered bytes → parity broken"
        );
    }
}
