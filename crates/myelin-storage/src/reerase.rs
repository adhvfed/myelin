//! # Post-restore re-erasure (STOR-D3) + the cell-kill RTO drill (STOR-D2)
//!
//! **Prompt:** P-ST-14 → global **P-100** (M1). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §7.5 (*Post-restore re-erasure
//! GD-14 — every restore runs a mandatory re-erasure pass against the erasure ledger (10.8): for
//! each erasure completed AFTER the backup's point-in-time, re-apply it (re-destroy the per-subject
//! DEK, re-delete the pseudonym map, re-purge+reindex Search, re-tombstone Refs, re-emit `*.erased`);
//! assert 0 resurrected subjects. The erasure ledger is itself NOT crypto-shred-erasable (it must
//! survive to drive re-erasure) and holds no PII*), §7.1 (*RTO target ≤ 1 h/tenant, ≤ 4 h/cell*),
//! §7.6 (*the backup-window-vs-erasure-SLA residual, `[OPEN → LEGAL]`*).
//! **Contract-index:** row **11.5** (`post_restore_reerase` + the RTO targets — completing the
//! headline). Consumed: 10.8 (the GDPR erasure ledger driving re-erasure), the P-ST-09
//! [`crate::erase`] algorithm (re-applied per erasure).
//!
//! ## The load-bearing idea (§7.5 — why a restore can RESURRECT an erased person)
//! A backup is a point-in-time T. Suppose a subject is erased (crypto-shredded) at offset
//! `e > T` — AFTER the backup was taken. The backup still holds that subject's *pre-erasure* per-
//! subject DEK (the key was live when the backup was taken). The before-the-backup invariant the
//! restore-verify gate already holds (P-061: a key crypto-shredded BEFORE the backup is excluded
//! from the snapshot) does **NOT** cover this case — the erasure had not happened yet at T. Without
//! a re-erasure pass, restoring T would bring that DEK back to life and **un-erase the person** — a
//! GDPR violation and the gravest possible data-handling failure. The fix (§7.5): every restore runs
//! a mandatory re-erasure pass that re-applies every erasure the ledger records as completed AFTER
//! the restore's PIT.
//!
//! This is the mirror image of the gate's before-the-backup leg: P-061 keeps a *pre-T* erasure dead
//! (exclude-from-backup); this prompt re-applies every *post-T* erasure (re-erase-after-restore).
//! Together they make a restore **never resurrect an erased subject, whenever the erasure happened**.
//!
//! ## What this module OWNS (new) vs what it REUSES (coherence, EI-01 §7)
//! The crypto-shred [`crate::erase::CryptoShredErase`] six-step algorithm (P-099), its cross-holder
//! [`crate::erase::EraseHolders`] seams, the [`crate::restore::RestoreReport`] (P-060), the
//! restore-verify [`crate::restore_verify::RestoreVerifyGate`] + its [`crate::restore_verify::ErasureLedger`]
//! before-the-backup seam (P-061), the KMS crypto-shred backup exclusion ([`crate::kms`], P-058), and
//! the harness RTO model (`myelin_harness::restore::RtoGrain` / `RestoreOutcome`, P-056) ALL already
//! exist. Per the coherence rule this prompt does **NOT** re-define any of them — it REUSES them:
//! the re-erasure pass RE-RUNS [`crate::erase::CryptoShredErase::erase`] (the same idempotent
//! algorithm — re-erasing is a no-op success, the property P-099 proved) for each post-PIT erasure;
//! the cell-kill RTO drill records onto the harness `RestoreRtoSecs` signal. What is genuinely NEW:
//! - **[`PostRestoreErasureLedger`]** — the §7.5 / 10.8 ledger seam keyed by *completion offset* (so
//!   the pass can select the erasures completed AFTER a restore's PIT T). The full GDPR-owned
//!   erasure ledger (10.8, P-GA-15 / P-115) is co-built in this band and wires the real binding;
//!   this is the storage-side seam the re-erasure pass drives + an in-memory implementation.
//! - **[`ReErasePass`]** — the mandatory post-restore re-erasure pass: select the post-PIT erasures,
//!   re-apply each (re-destroy the per-subject DEK + re-run the five cross-holder seams + re-emit
//!   `*.erased`), and assert **0 resurrected subjects** ([`ReEraseReport::resurrected_count`] == 0).
//! - **[`CellKillRestore`]** — the cell-kill RTO measurement model (begin-restore → ready, per
//!   grain), producing the measured `restore_time_per_tenant/cell` seconds the STOR-D2 RTO drill
//!   asserts ≤ the thresholds-file bound.
//!
//! ## Wired into the restore-verify gate (every restore re-erases BY CONSTRUCTION)
//! [`crate::restore_verify::RestoreVerifyGate::run_with_reerase`] (added by this prompt) drives the
//! restore, runs the existing three §7.4 assertions, and THEN runs [`ReErasePass`] against the
//! post-PIT ledger — so a restore that resurrects a post-T-erased subject FAILs the gate
//! ([`crate::restore_verify::GateFailure::ErasureResurrected`]). This is the §7.5 "wire this into the
//! restore-verify gate so every restore re-erases by construction" requirement. The original
//! [`crate::restore_verify::RestoreVerifyGate::run`] is preserved (the before-the-backup-only path);
//! the new entrypoint is the post-PIT-aware one.
//!
//! ## DEVIATION / FLOOR — modeled offsets + in-memory ledger, not the live GDPR ledger (EI-01 §1)
//! The real GDPR-owned erasure ledger (10.8, tamper-evident hash-chain, P-GA-15 / P-115) is co-built
//! in this band; storage cannot depend on `myelin-gdpr-service` (an upward DAG edge). So the ledger
//! is a SEAM ([`PostRestoreErasureLedger`]) the GDPR ledger wires, with an in-memory implementation
//! ([`InMemoryPostPitLedger`]) the pass + drill drive. The re-erasure MECHANISM (select post-PIT,
//! re-apply each, assert 0 resurrected) does **not** change shape when the real ledger lands: that
//! ledger will *populate* the seam off its hash-chained records; the pass reads identically. The
//! per-erasure completion *offset* models the §7.3 cross-seam cursor (the same `WalOffset` the
//! restore lands at) so "completed after the backup's PIT" is an exact, assertable comparison.
//!
//! ## FLOORS NAMED (the prompt's DEFINITION OF DONE)
//! - **The RTO numbers (≤ 1 h-tenant / ≤ 4 h-cell)** are the proposed defaults-to-beat — MEASURED by
//!   the STOR-D2 cell-kill drill here (against `rpo_rto.rto_tenant_max_mins` /
//!   `rto_cell_max_mins` in the versioned `thresholds.toml`, never hardcoded) — and **re-confirmed at
//!   cell scale in M5 (P-ST-30)**. Named per the DoD.
//! - **The §7.6 backup-window-vs-erasure-SLA residual number** is `[OPEN → LEGAL]` — a DPO-ratified,
//!   documented number, not a silent gap. The narrow residual (a key backed up before destruction)
//!   is closed by §7.5 (this re-erasure pass) + "shredded keys excluded from backup" (P-058/P-061);
//!   the exposure window is bounded by the retention period + this pass. Recorded here in writing as
//!   the named follow-on (the NUMBER → counsel/DPO; the MECHANISM ships now).
//! - **The real GDPR erasure-ledger binding** (10.8, the tamper-evident hash-chain) is P-GA-15 /
//!   P-115; this prompt ships the seam it drives + the in-memory model the pass/drill exercise.
//! - **The real `pg_restore` + WAL-replay + cell-kill provisioning driver** is the P-S12/P-S15 floor;
//!   the re-erasure + RTO-measurement mechanism ships now and does not change shape when it lands.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2; prompt TESTS field)
//! The re-erasure pass (select-post-PIT + re-apply-each + assert-0-resurrected) is mandatory-core:
//! the load-bearing decisions are *only the erasures completed AFTER the PIT are re-applied*, *each
//! re-apply is idempotent (a no-op re-erase is success)*, and *a resurrected subject is a LOUD 0
//! assertion*. The achieved score is stated in the P-100 report
//! (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/reerase.rs`).

use std::collections::BTreeMap;

use myelin_tenancy::TenantId;

use crate::backup::{EpochSecs, WalOffset};
use crate::encryption::SubjectId;
use crate::erase::{CryptoShredErase, EpochMillis, EraseError, EraseHolders, ErasureReceipt};
use crate::kms::{DekId, KeyClass, KmsEngine};
use crate::restore::RestoreReport;

// ───────────────────────────── the erasure-ledger record (PII-free, 10.8) ─────────────────────────────

/// One completed-erasure record in the §7.5 / 10.8 erasure ledger — **PII-free** (an opaque subject
/// id + tenant + the cross-seam offset the erasure completed at). It is the durable, NON-shred-
/// erasable record that DRIVES re-erasure: it must survive the crypto-shred it records AND a restore,
/// so a restored older copy can be re-erased from it (§7.5 — the ledger holds no PII and is not itself
/// crypto-shred-erasable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureRecord {
    /// The opaque subject id that was erased (already pseudonymous — never real-identity PII).
    pub subject: SubjectId,
    /// The tenant the erasure ran within.
    pub tenant: TenantId,
    /// **The cross-seam offset the erasure COMPLETED at** (the §7.3 cursor). An erasure with
    /// `completed_at_offset > T` (the restore's PIT) is one a restore of T would RESURRECT — it is
    /// the set the re-erasure pass re-applies. PII-free.
    pub completed_at_offset: WalOffset,
}

impl ErasureRecord {
    /// A new erasure-ledger record (opaque subject + tenant + the completion offset).
    pub fn new(
        subject: SubjectId,
        tenant: TenantId,
        completed_at_offset: WalOffset,
    ) -> ErasureRecord {
        ErasureRecord {
            subject,
            tenant,
            completed_at_offset,
        }
    }
}

// ───────────────────────────── the post-restore erasure-ledger seam (10.8) ─────────────────────────────

/// **The §7.5 / 10.8 erasure-ledger seam keyed by completion offset.** The re-erasure pass asks the
/// ledger for every erasure **completed AFTER the restore's PIT T** — the set a restore of T would
/// resurrect. The real binding is the GDPR-owned tamper-evident erasure ledger (10.8, P-GA-15 /
/// P-115), co-built in this band; storage drives it through this trait (it cannot depend on
/// `myelin-gdpr-service` without an upward DAG edge). The ledger is PII-free and NOT itself
/// crypto-shred-erasable (it must survive to drive re-erasure).
pub trait PostRestoreErasureLedger {
    /// Every erasure the ledger records as **completed AFTER offset `pit`** (the restore's
    /// point-in-time T) — the erasures a restore of `pit` would resurrect, which the re-erasure pass
    /// re-applies. Records completed at-or-before `pit` are NOT returned (a pre-T erasure is already
    /// dead in the backup by construction — P-058/P-061). The returned order is the re-apply order.
    fn erasures_completed_after(&self, pit: WalOffset) -> Vec<ErasureRecord>;
}

/// An in-memory [`PostRestoreErasureLedger`] (the floor the pass + drill drive; the real GDPR ledger
/// is the seam binding, P-GA-15 / P-115). Holds the PII-free [`ErasureRecord`]s; selection is the
/// exact `completed_at_offset > pit` comparison the §7.5 pass needs.
///
/// **MR-009b W6b — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature = "test-support"))]`).**
/// The always-compiled PRODUCTION [`PostRestoreErasureLedger`] is the durable
/// [`crate::reerase_durable::DurablePostPitLedger`] over the non-shred-erasable `post_pit_erasure_ledger`
/// table (migration `0052`) — a durable impl drops into `ReErasePass::run`'s `&dyn` seam with zero
/// caller change. The `no-in-memory-durable-store` scanner strips this `test-support`-gated
/// `records: Vec<…>` holder, so the production graph presents only the durable ledger.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, Default)]
pub struct InMemoryPostPitLedger {
    records: Vec<ErasureRecord>,
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryPostPitLedger {
    /// An empty ledger.
    pub fn new() -> InMemoryPostPitLedger {
        InMemoryPostPitLedger::default()
    }

    /// Record a completed erasure (PII-free). Recorded in completion order.
    pub fn record(&mut self, record: ErasureRecord) -> &mut Self {
        self.records.push(record);
        self
    }

    /// The number of erasure records held.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// `true` iff the ledger holds no records.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl PostRestoreErasureLedger for InMemoryPostPitLedger {
    fn erasures_completed_after(&self, pit: WalOffset) -> Vec<ErasureRecord> {
        // ONLY the erasures completed strictly AFTER the PIT — a pre-or-at-T erasure is already dead
        // in the backup (P-058/P-061); re-applying it would be a harmless no-op, but the §7.5 pass is
        // precise: it re-applies exactly the post-T set the restore could resurrect.
        self.records
            .iter()
            .filter(|r| r.completed_at_offset > pit)
            .cloned()
            .collect()
    }
}

// ───────────────────────────── the re-erasure report (the STOR-D3 artifact) ─────────────────────────────

/// One re-applied erasure in a [`ReEraseReport`] — the receipt the re-run produced + whether the
/// subject was found resurrected (its DEK present in the restored set before the re-apply).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReErasedSubject {
    /// The opaque subject id re-erased.
    pub subject: SubjectId,
    /// The tenant the re-erasure ran within.
    pub tenant: TenantId,
    /// `true` iff the subject's per-subject DEK was present in the restored copy BEFORE the re-apply
    /// (i.e. the restore HAD resurrected it, and the pass re-killed it). `false` iff the key was
    /// already absent (a defensively idempotent re-apply — nothing to re-kill). Either way the
    /// post-condition holds: after the pass the key is destroyed.
    pub was_resurrected_before_reapply: bool,
    /// The receipt the re-applied [`CryptoShredErase::erase`] returned (the §5.2 algorithm re-run).
    pub receipt: ErasureReceipt,
}

/// The dated artifact the post-restore re-erasure pass returns — the STOR-D3 PROOF that **0 subjects
/// are resurrected** after the restore. It names the restore's PIT, the re-applied erasures, and the
/// `resurrected_count` — the number of post-PIT-erased subjects STILL recoverable AFTER the pass,
/// which MUST be **0** (the §7.5 gate reading). A non-zero count is a RED drill: the restore un-erased
/// a person and the re-erasure pass did not re-kill them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReEraseReport {
    /// The restore's point-in-time T the re-erasure ran against (every re-applied erasure completed
    /// AFTER this).
    pub restored_to_offset: WalOffset,
    /// The subjects re-erased (each a re-run receipt). Empty iff no erasure was completed after T.
    pub re_erased: Vec<ReErasedSubject>,
    /// **THE STOR-D3 GATE READING:** how many post-PIT-erased subjects are STILL recoverable (their
    /// per-subject DEK present in the restored copy) AFTER the re-erasure pass — MUST be **0**. A
    /// non-zero value un-erases a person: the gravest failure.
    pub resurrected_count: u64,
}

impl ReEraseReport {
    /// Whether the re-erasure pass is GREEN: **0 resurrected subjects** (§7.5). The ONLY way to read
    /// a pass — a non-zero `resurrected_count` is never silently a pass.
    pub fn is_green(&self) -> bool {
        self.resurrected_count == 0
    }

    /// The number of erasures re-applied (the subjects the restore could have resurrected).
    pub fn re_erased_count(&self) -> usize {
        self.re_erased.len()
    }

    /// `true` iff `subject` (within `tenant`) was re-erased by this pass.
    pub fn re_erased_subject(&self, subject: &SubjectId, tenant: &TenantId) -> bool {
        self.re_erased
            .iter()
            .any(|s| &s.subject == subject && &s.tenant == tenant)
    }
}

// ───────────────────────────── the post-restore re-erasure pass (§7.5) ─────────────────────────────

/// **The mandatory post-restore re-erasure pass (storage.md §7.5 / GD-14 — the headline).**
///
/// After a [`crate::restore::restore_to_offset`] lands a copy at PIT T, this pass re-applies every
/// erasure the [`PostRestoreErasureLedger`] records as completed AFTER T (the set the restore could
/// resurrect): for each, it RE-RUNS the P-099 [`CryptoShredErase`] six-step algorithm (re-destroy the
/// per-subject DEK, re-shred the pseudonym map, re-purge+reindex Search, re-tombstone Refs, re-emit
/// `*.erased`, re-record the receipt) and then ASSERTS the subject's per-subject DEK is gone from the
/// restored copy — **0 resurrected subjects**.
///
/// It is **idempotent by construction**: the re-applied [`CryptoShredErase::erase`] is itself
/// idempotent (P-099 — a re-erase is a no-op success, not an error), so re-running it on an already-
/// erased subject simply re-affirms the post-condition. Running the pass TWICE yields the same
/// `resurrected_count == 0`.
///
/// It borrows the SAME [`KmsEngine`] the restored copy's encrypted columns resolve DEKs through (never
/// a parallel key store — so the re-destroy reaches exactly the resurrected ciphertext) and re-uses
/// the [`CryptoShredErase`] mechanism — never a second erase implementation.
pub struct ReErasePass<'a> {
    eraser: CryptoShredErase<'a>,
    engine: &'a KmsEngine,
}

impl<'a> ReErasePass<'a> {
    /// Build the re-erasure pass over the restored copy's KMS engine + the region the tenant KEKs
    /// live in. Reuses [`CryptoShredErase`] (the P-099 algorithm) — never a second eraser.
    pub fn new(engine: &'a KmsEngine, region: myelin_tenancy::Region) -> ReErasePass<'a> {
        ReErasePass {
            eraser: CryptoShredErase::new(engine, region),
            engine,
        }
    }

    /// **Run the post-restore re-erasure pass against a [`RestoreReport`] (§7.5).**
    ///
    /// 1. Ask the `ledger` for every erasure completed AFTER `report.restored_to_offset` (the PIT) —
    ///    the set the restore could resurrect.
    /// 2. For each: record whether its per-subject DEK is present in the restored copy (a resurrected
    ///    subject), then RE-RUN [`CryptoShredErase::erase`] (the idempotent §5.2 algorithm) through
    ///    the cross-holder `holders` seams to re-destroy the DEK + re-purge derived stores + re-emit
    ///    `*.erased`.
    /// 3. After re-applying ALL of them, count how many post-PIT-erased subjects are STILL recoverable
    ///    (DEK present) — the `resurrected_count`, which MUST be **0**.
    ///
    /// `now` is the caller-supplied clock the re-applied receipts stamp (deterministic; no hidden
    /// global time). Returns a [`ReEraseReport`] (`resurrected_count == 0` on green) or a LOUD
    /// [`EraseError`] if a re-applied erasure step failed (an incomplete re-erasure is a retry, never
    /// "assume re-erased").
    pub fn run(
        &self,
        report: &RestoreReport,
        ledger: &dyn PostRestoreErasureLedger,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<ReEraseReport, EraseError> {
        let pit = report.restored_to_offset;
        // (1) Select EXACTLY the erasures completed AFTER the PIT — the resurrection-risk set.
        let post_pit = ledger.erasures_completed_after(pit);

        let mut re_erased = Vec::with_capacity(post_pit.len());
        for record in &post_pit {
            // (2a) Was the subject resurrected? Its per-subject DEK present in the restored copy means
            // the restore brought a now-supposed-to-be-erased key back to life.
            let subject_dek = DekId::new(
                record.tenant.clone(),
                KeyClass::Subject(record.subject.0.clone()),
            );
            let was_resurrected = self.dek_present(&subject_dek);

            // (2b) RE-APPLY the erasure: re-run the P-099 six-step algorithm (re-destroy DEK + re-run
            // the cross-holder seams + re-emit `*.erased`). Idempotent — a no-op if already dead.
            let receipt = self
                .eraser
                .erase(&record.subject, &record.tenant, holders, now)?;

            re_erased.push(ReErasedSubject {
                subject: record.subject.clone(),
                tenant: record.tenant.clone(),
                was_resurrected_before_reapply: was_resurrected,
                receipt,
            });
        }

        // (3) Assert 0 resurrected: after re-applying ALL of them, NO post-PIT-erased subject's DEK
        // may still be recoverable. (Each `erase` already verifies `recoverable_in_backup == 0`; this
        // is the LIVE restored-copy reading the §7.5 gate names — the key is gone from the copy too.)
        let resurrected_count = post_pit
            .iter()
            .filter(|record| {
                let dek = DekId::new(
                    record.tenant.clone(),
                    KeyClass::Subject(record.subject.0.clone()),
                );
                self.dek_present(&dek)
            })
            .count() as u64;

        Ok(ReEraseReport {
            restored_to_offset: pit,
            re_erased,
            resurrected_count,
        })
    }

    /// `true` iff a per-subject DEK is present (live or in the backup snapshot) in the restored copy's
    /// KMS engine — a resurrected key. A destroyed DEK is absent from both, so after a re-apply this is
    /// `false` (the §7.5 0-resurrected post-condition).
    fn dek_present(&self, dek: &DekId) -> bool {
        self.engine.backup_snapshot().iter().any(|(d, _)| d == dek)
    }
}

// ───────────────────────────── the cell-kill RTO model (STOR-D2 RTO half) ─────────────────────────────

/// The grain a cell-kill RTO is measured at (storage.md §7.1 RTO targets): per-tenant (≤ 1 h) or
/// per-cell (≤ 4 h). Mirrors the harness `RtoGrain` (the drill records onto the harness
/// `RestoreRtoSecs` signal); kept storage-native so the runtime model does not depend on the harness
/// (a dev-only crate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtoGrain {
    /// Per-tenant recovery (the `rto_tenant_max_mins` bound — ≤ 1 h default-to-beat).
    Tenant,
    /// Per-cell recovery (the `rto_cell_max_mins` bound — ≤ 4 h default-to-beat).
    Cell,
}

impl RtoGrain {
    /// The PII-free telemetry label for the grain (the `{grain}` label on `RestoreRtoSecs`).
    pub fn label(self) -> &'static str {
        match self {
            RtoGrain::Tenant => "tenant",
            RtoGrain::Cell => "cell",
        }
    }
}

/// **The cell-kill RTO measurement (STOR-D2 RTO half, §7.1).** Models the recovery-TIME objective: the
/// wall-clock from "begin restore" (the cell is killed; restore starts from the archive, P-059) to
/// "consistent, ready copy" (the restore landed + the post-restore re-erasure pass ran). The drill
/// asserts the measured per-grain RTO ≤ the thresholds-file bound (≤ 1 h-tenant / ≤ 4 h-cell), never
/// hardcoded.
///
/// On the real floor the elapsed time is the actual `pg_restore` + WAL-replay + reindex-from-source +
/// re-erase wall-clock (the P-S12/P-S15 driver); modeled here as the caller-supplied phase durations
/// so the RTO is an exact, assertable number (the same shape the harness `RestoreOutcome` uses).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellKillRestore {
    /// The grain this recovery is measured at (tenant or cell).
    pub grain: RtoGrain,
    /// The wall-clock (seconds) the restore began (the cell was killed at-or-before this).
    pub began_at: EpochSecs,
    /// The wall-clock (seconds) the restored copy was consistent AND ready (the restore landed + the
    /// re-erasure pass completed). MUST be `>= began_at`.
    pub ready_at: EpochSecs,
}

impl CellKillRestore {
    /// Build a cell-kill recovery measurement from the begin/ready timestamps. `ready_at` is clamped
    /// to be `>= began_at` (recovery cannot finish before it starts) — a defensively monotone clock.
    pub fn new(grain: RtoGrain, began_at: EpochSecs, ready_at: EpochSecs) -> CellKillRestore {
        CellKillRestore {
            grain,
            began_at,
            ready_at: ready_at.max(began_at),
        }
    }

    /// **The MEASURED RTO (the STOR-D2 number):** seconds from begin-restore to consistent-ready
    /// copy. The drill asserts this ≤ the per-grain `rto_*_max_mins` bound (from `thresholds.toml`).
    pub fn rto_secs(&self) -> EpochSecs {
        self.ready_at.saturating_sub(self.began_at)
    }

    /// `true` iff the measured RTO is within `bound_secs` (the per-grain threshold). The drill reads
    /// `bound_secs` from the versioned thresholds file (never hardcoded).
    pub fn within_bound(&self, bound_secs: EpochSecs) -> bool {
        self.rto_secs() <= bound_secs
    }
}

/// The per-grain measured RTO set across a cell-kill drill (the `restore_time_per_tenant/cell`
/// telemetry). Built by the STOR-D2 drill; carries the measured number the drill asserts ≤ bound for
/// each grain.
#[derive(Clone, Debug, Default)]
pub struct CellKillRtoReport {
    rto_secs: BTreeMap<&'static str, EpochSecs>,
}

impl CellKillRtoReport {
    /// An empty report.
    pub fn new() -> CellKillRtoReport {
        CellKillRtoReport::default()
    }

    /// Record a measured cell-kill recovery (keyed by its grain label).
    pub fn record(&mut self, recovery: &CellKillRestore) -> &mut Self {
        self.rto_secs
            .insert(recovery.grain.label(), recovery.rto_secs());
        self
    }

    /// The measured RTO for a grain, if recorded.
    pub fn rto_for(&self, grain: RtoGrain) -> Option<EpochSecs> {
        self.rto_secs.get(grain.label()).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{ContinuousArchiver, WalSegment};
    use crate::encryption::ColumnCryptor;
    use crate::erase::{BusErase, ErasureLedgerSink, PseudonymShred, RefsTombstone, SearchPurge};
    use crate::kms::KekId;
    use crate::restore::{restore_to_offset, BlobPresence, SourceLog};
    use myelin_gdpr::ErasureMethod;
    use myelin_tenancy::Region;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    fn t(s: &str) -> TenantId {
        TenantId(s.into())
    }
    fn r() -> Region {
        Region("eu-west".into())
    }

    // ── recording test doubles for the cross-holder re-erasure seams (re-run on each re-apply) ──

    #[derive(Default)]
    struct RecSeams {
        calls: RefCell<Vec<String>>,
        erased: RefCell<BTreeSet<String>>,
    }
    impl PseudonymShred for RecSeams {
        fn shred_pseudonym(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.calls.borrow_mut().push(format!("pseudonym:{}", s.0));
            Ok(())
        }
    }
    impl SearchPurge for RecSeams {
        fn purge_and_reindex(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.calls.borrow_mut().push(format!("search:{}", s.0));
            Ok(())
        }
    }
    impl RefsTombstone for RecSeams {
        fn tombstone(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.calls.borrow_mut().push(format!("refs:{}", s.0));
            Ok(())
        }
    }
    impl BusErase for RecSeams {
        fn erase_inline_pii(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            // The `*.erased` re-emit (the §7.5 step).
            self.calls.borrow_mut().push(format!("erased:{}", s.0));
            Ok(())
        }
    }
    impl ErasureLedgerSink for RecSeams {
        fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.erased.borrow_mut().insert(subject.0.clone());
        }
        fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
            self.erased.borrow().contains(&subject.0)
        }
    }

    fn holders(seams: &RecSeams) -> EraseHolders<'_> {
        EraseHolders {
            pseudonym: seams,
            search: seams,
            refs: seams,
            bus: seams,
            ledger: seams,
            git_reach: None,
        }
    }

    /// A reachable archiver (base at 0, tail at `tail`).
    fn reachable_archiver(tail: WalOffset) -> ContinuousArchiver {
        let mut a = ContinuousArchiver::new();
        a.archive_segment(WalSegment {
            end_offset: 0,
            committed_at: 0,
        })
        .unwrap();
        a.take_base_backup(1);
        a.archive_segment(WalSegment {
            end_offset: tail,
            committed_at: 10,
        })
        .unwrap();
        a
    }

    /// Stand up a KMS engine + seal a per-subject column so the restored copy HAS a resurrectable DEK.
    fn engine_with_subject(tenant: &TenantId, subject: &SubjectId) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()));
        let cryptor = ColumnCryptor::new(&kms, r());
        cryptor
            .encrypt(
                tenant,
                Some(subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                b"to be re-erased",
            )
            .expect("seal a per-subject column");
        kms
    }

    // ───────── the ledger selects EXACTLY the post-PIT erasures ─────────

    /// **The ledger returns ONLY erasures completed AFTER the PIT** (the resurrection-risk set). An
    /// erasure at-or-before T is already dead in the backup (P-058/P-061) and is NOT returned. Kills
    /// the mutant that flips `>` to `>=`/`<` or returns all records.
    #[test]
    fn ledger_selects_only_post_pit_erasures() {
        let mut ledger = InMemoryPostPitLedger::new();
        ledger
            .record(ErasureRecord::new(SubjectId::new("pre"), t("acme"), 50)) // ≤ T → not returned
            .record(ErasureRecord::new(SubjectId::new("at"), t("acme"), 100)) // == T → not returned
            .record(ErasureRecord::new(SubjectId::new("post"), t("acme"), 140)); // > T → returned

        let after = ledger.erasures_completed_after(100);
        let ids: Vec<&str> = after.iter().map(|r| r.subject.0.as_str()).collect();
        assert_eq!(ids, vec!["post"], "only the post-PIT erasure is selected");
    }

    // ───────── the re-erasure pass re-applies a RESURRECTED subject to 0 recoverable ─────────

    /// **MANDATORY-CORE: a subject erased AFTER the backup's PIT is RESURRECTED by a restore of T, and
    /// the re-erasure pass re-erases it to 0 recoverable** (the §7.5 / STOR-D3 headline). The restored
    /// copy holds the pre-erasure DEK (it was live at T); the pass re-destroys it; 0 resurrected.
    #[test]
    fn reerase_re_kills_a_resurrected_post_pit_subject() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-erased-after-backup");
        // The restored copy's KMS has the subject's DEK alive (the restore of T brought it back —
        // the erasure happened at offset 140, AFTER the backup PIT T=100).
        let kms = engine_with_subject(&tenant, &subject);
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the restored copy RESURRECTED the subject's DEK (it was live at the backup PIT)"
        );

        // The restore landed at T=100.
        let arch = reachable_archiver(300);
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

        let seams = RecSeams::default();
        let pass = ReErasePass::new(&kms, r());
        let rep = pass
            .run(&report, &ledger, &holders(&seams), 1_000)
            .expect("the re-erasure pass succeeds");

        // The subject was found resurrected and re-erased.
        assert_eq!(rep.re_erased_count(), 1);
        assert!(rep.re_erased_subject(&subject, &tenant));
        assert!(
            rep.re_erased[0].was_resurrected_before_reapply,
            "the subject WAS resurrected by the restore (its DEK was live at T)"
        );
        // STOR-D3: 0 resurrected after the pass.
        assert_eq!(rep.resurrected_count, 0, "0 resurrected subjects (§7.5)");
        assert!(rep.is_green());
        // The DEK is gone from the restored copy now (re-killed).
        assert!(
            !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the resurrected DEK is re-destroyed by the pass"
        );
        // The cross-holder seams re-ran (re-purge Search, re-tombstone Refs, re-emit `*.erased`).
        let calls = seams.calls.borrow();
        assert!(calls.contains(&"search:u-erased-after-backup".to_string()));
        assert!(calls.contains(&"refs:u-erased-after-backup".to_string()));
        assert!(
            calls.contains(&"erased:u-erased-after-backup".to_string()),
            "`*.erased` re-emitted"
        );
    }

    /// **The re-erasure pass is IDEMPOTENT** — running it TWICE yields the same `resurrected_count ==
    /// 0` (the re-applied erase is itself a no-op success on the second run, P-099). Kills a mutant
    /// that errors on a second re-apply.
    #[test]
    fn reerase_pass_is_idempotent() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-twice");
        let kms = engine_with_subject(&tenant, &subject);
        let arch = reachable_archiver(300);
        let report = restore_to_offset(
            &arch,
            100,
            &[],
            &BlobPresence::new(),
            &SourceLog::new(),
            &kms,
        )
        .unwrap();
        let mut ledger = InMemoryPostPitLedger::new();
        ledger.record(ErasureRecord::new(subject.clone(), tenant.clone(), 140));
        let seams = RecSeams::default();
        let pass = ReErasePass::new(&kms, r());

        let first = pass.run(&report, &ledger, &holders(&seams), 1).unwrap();
        assert!(first.is_green());
        assert!(
            first.re_erased[0].was_resurrected_before_reapply,
            "first pass re-killed it"
        );

        // SECOND pass: the DEK is already gone (a no-op re-apply); still 0 resurrected.
        let second = pass.run(&report, &ledger, &holders(&seams), 2).unwrap();
        assert_eq!(
            second.resurrected_count, 0,
            "idempotent: still 0 resurrected"
        );
        assert!(
            !second.re_erased[0].was_resurrected_before_reapply,
            "the second pass found the key already gone (no resurrection to re-kill)"
        );
        assert!(second.is_green());
    }

    /// The ledger len/is_empty accessors + the report's `re_erased_subject` lookup are exact (kills the
    /// accessor mutants — these feed the CDC + drill assertions, so they are load-bearing reads).
    #[test]
    fn ledger_and_report_accessors_are_exact() {
        let mut ledger = InMemoryPostPitLedger::new();
        assert!(ledger.is_empty(), "a fresh ledger is empty");
        assert_eq!(ledger.len(), 0);
        ledger.record(ErasureRecord::new(SubjectId::new("a"), t("acme"), 10));
        ledger.record(ErasureRecord::new(SubjectId::new("b"), t("acme"), 20));
        assert!(!ledger.is_empty(), "a populated ledger is not empty");
        assert_eq!(ledger.len(), 2, "len counts every recorded erasure");

        let report = ReEraseReport {
            restored_to_offset: 100,
            re_erased: vec![ReErasedSubject {
                subject: SubjectId::new("present"),
                tenant: t("acme"),
                was_resurrected_before_reapply: true,
                receipt: crate::erase::ErasureReceipt {
                    subject: "present".into(),
                    tenant: t("acme"),
                    dek_destroyed_now: true,
                    recoverable_in_backup: 0,
                    crypto_shred_lag_ms: 0,
                    re_run: false,
                    completed_at: 0,
                },
            }],
            resurrected_count: 0,
        };
        // Both the subject AND the tenant must match (kills the `&& -> ||` + `-> true` mutants).
        assert!(report.re_erased_subject(&SubjectId::new("present"), &t("acme")));
        assert!(
            !report.re_erased_subject(&SubjectId::new("absent"), &t("acme")),
            "a non-re-erased subject is not reported (kills the `-> true` mutant)"
        );
        assert!(
            !report.re_erased_subject(&SubjectId::new("present"), &t("other-tenant")),
            "the tenant must ALSO match (kills the `&& -> ||` mutant)"
        );
    }

    /// A subject erased BEFORE the backup is NOT in the post-PIT set, so the pass does NOT re-apply it
    /// (it is already dead by construction — P-058/P-061). The pass re-applies ONLY the post-T set.
    #[test]
    fn a_pre_pit_erasure_is_not_re_applied() {
        let kms = KmsEngine::new();
        let arch = reachable_archiver(300);
        let report = restore_to_offset(
            &arch,
            100,
            &[],
            &BlobPresence::new(),
            &SourceLog::new(),
            &kms,
        )
        .unwrap();
        let mut ledger = InMemoryPostPitLedger::new();
        // Erased at offset 60 — BEFORE the backup PIT T=100. Not a resurrection risk.
        ledger.record(ErasureRecord::new(SubjectId::new("pre"), t("acme"), 60));
        let seams = RecSeams::default();
        let pass = ReErasePass::new(&kms, r());
        let rep = pass.run(&report, &ledger, &holders(&seams), 1).unwrap();
        assert_eq!(
            rep.re_erased_count(),
            0,
            "a pre-PIT erasure is not re-applied"
        );
        assert!(rep.is_green());
        assert!(seams.calls.borrow().is_empty(), "no re-erasure ran");
    }

    /// `is_green` is FALSE when a subject is still recoverable (kills the `is_green -> true` mutant).
    #[test]
    fn report_is_green_only_when_zero_resurrected() {
        let red = ReEraseReport {
            restored_to_offset: 100,
            re_erased: vec![],
            resurrected_count: 1, // a person un-erased
        };
        assert!(!red.is_green(), "a resurrected subject is RED");
        let green = ReEraseReport {
            resurrected_count: 0,
            ..red
        };
        assert!(green.is_green(), "0 resurrected is GREEN");
    }

    // ───────── the cell-kill RTO model (STOR-D2 RTO half) ─────────

    /// **The cell-kill RTO is the begin→ready wall-clock, asserted within the per-grain bound.** A
    /// tenant recovery in 40 min is within the 60-min bound; a cell recovery in 3 h within the 4-h
    /// bound. The drill reads the bounds from `thresholds.toml`.
    #[test]
    fn cell_kill_rto_is_measured_per_grain() {
        // A tenant recovery: killed at t=0, ready at t=2400s (40 min).
        let tenant_recovery = CellKillRestore::new(RtoGrain::Tenant, 0, 2_400);
        assert_eq!(tenant_recovery.rto_secs(), 2_400);
        assert!(
            tenant_recovery.within_bound(3_600),
            "40 min ≤ 1 h tenant bound"
        );
        assert!(
            !tenant_recovery.within_bound(1_800),
            "40 min exceeds a 30-min bound (the gate bites)"
        );

        // A cell recovery: killed at t=0, ready at t=10800s (3 h).
        let cell_recovery = CellKillRestore::new(RtoGrain::Cell, 0, 10_800);
        assert_eq!(cell_recovery.rto_secs(), 10_800);
        assert!(cell_recovery.within_bound(14_400), "3 h ≤ 4 h cell bound");

        let mut report = CellKillRtoReport::new();
        report.record(&tenant_recovery).record(&cell_recovery);
        assert_eq!(report.rto_for(RtoGrain::Tenant), Some(2_400));
        assert_eq!(report.rto_for(RtoGrain::Cell), Some(10_800));
    }

    /// The RTO clock is defensively monotone: a `ready_at` before `began_at` is clamped (recovery
    /// cannot finish before it starts) → RTO 0, never a negative/underflowed huge number.
    #[test]
    fn rto_clock_is_monotone() {
        let recovery = CellKillRestore::new(RtoGrain::Tenant, 100, 50);
        assert_eq!(
            recovery.rto_secs(),
            0,
            "ready clamped to began → RTO 0, never underflow"
        );
    }

    /// The grain labels are stable (kills a `label -> ""`/swapped mutant — the telemetry label is
    /// load-bearing for the `RestoreRtoSecs{grain}` signal).
    #[test]
    fn rto_grain_labels_are_stable() {
        assert_eq!(RtoGrain::Tenant.label(), "tenant");
        assert_eq!(RtoGrain::Cell.label(), "cell");
    }
}
