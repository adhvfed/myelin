//! The **storage-side multi-cell DSR erase fan-out** (P-ST-33 / global P-445; the FLOOR drill GA-D8,
//! storage half; contract 10.4 "the DSR fan-out iterates `member_cells`" + 11.4 "the erase reach").
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §2 "S-M5" (the multi-cell follow-on:
//! the DSR fan-out iterates `member_cells`; the FLOOR drill GA-D8 is now owed and run) + §5.2 (the
//! `erase(subject, tenant)` six-step algorithm this iterates, per cell; "the erase reach is VERIFIED,
//! not assumed"). Contract-index rows 10.4 (the DSR fan-out iterates `member_cells`) and 11.4 (the
//! crypto-shred erase reach). `external-insights/04-hard-problems.md` §1 (crypto-shred reaches every
//! holder incl. backups — here extended to reach every CELL). EI-01 §2 (the load-bearing zero — a
//! missed cell in an erasure fan-out is stop-the-bleeding) + §3 (a property does not exist until a
//! test forces the failure).
//!
//! ## What this prompt ships (P-ST-33) — the storage half of the multi-cell erase
//! [`MultiCellEraseFanOut`] extends the single-cell six-step crypto-shred erase ([`crate::erase`])
//! to a **multi-cell** tenant whose data lives across `{home_cell} ∪ member_cells`:
//!
//! - It iterates the deduplicated, deterministic cell set `{home_cell} ∪ member_cells`.
//! - For EACH cell, it runs that cell's own [`CryptoShredErase`] (each cell owns its own
//!   [`KmsEngine`] + region — a cell is a blast-radius boundary, so the per-subject DEK destroy is
//!   strictly cell-local: a destroy in cell C only ever touches C's keys).
//! - It merges one storage-side [`crate::erase::ErasureReceipt`] per cell into a
//!   [`MultiCellEraseReceiptSet`], and reports `cells_missed` — every cell the fan-out was supposed
//!   to iterate that did NOT produce a receipt (an unregistered / unreachable member cell). **0 cells
//!   missed** is the GA-D8 gate reading.
//! - It is **idempotent**: re-running the fan-out for an already-erased subject re-runs each cell's
//!   (idempotent) six-step erase — every per-cell receipt's `re_run` flips true on the second pass,
//!   the post-condition (0 recoverable in backup, every cell) still holds, and `cells_missed` stays 0.
//!
//! ## Relationship to the Tenancy/control-plane GA-D8 leg (no duplication — EI-01 §7)
//! The control plane already owns the **generic** cross-cell DSR fan-out orchestrator
//! (`myelin_control_plane::CrossCellDsrFanOut` + the `CellLocalEraser` seam, global P-430): it
//! dispatches an opaque per-cell erase across the bridge and merges opaque receipt tokens, with NO
//! storage knowledge. THIS module is the **storage leg** that sits BEHIND that seam: it is the real
//! crypto-shred erase that runs IN each cell and produces the storage `ErasureReceipt` the control
//! plane lowers to its opaque `CellDsrReceipt` token. The two are deliberately separate grains: the
//! control plane proves the cross-cell *orchestration* completeness (0 cells missed across the
//! bridge); this module proves the *storage* completeness (every cell's per-subject DEK is destroyed,
//! 0 recoverable in that cell's backup). The CDC pair `tests/cdc_10_4_multi_cell_erase_fanout.rs`
//! pins that the two `cells_missed` zeros agree (the storage leg lowers to the control-plane
//! `CellDsrReceipt` set without re-deriving completeness — the control plane is a dev-dependency, so
//! the bridge lives in that test, never in storage's production graph).
//!
//! ## GA-D8 — the gate (storage half)
//! `cells_missed == 0` AND every per-cell receipt is green (`recoverable_in_backup == 0`). A member
//! cell the fan-out could not reach (no registered eraser) is recorded as MISSED — never silently
//! dropped — so `cells_missed > 0` reads RED. The gate is a real tripwire (proven in the drill: an
//! unregistered member cell trips it red).
//!
//! ## Floors named (stubbed / deferred + the filling prompt) — VISION §3, prompt DoD
//! - **The full cross-HOLDER reach** (the erase reaches every H1–H18 holder — OLTP, object, log,
//!   OLAP, search, refs, bus, agent memory, notif history, authz tuples, caches/CDN, AND backups —
//!   per cell) is the **E2E-4 spine, P-ST-35 (global P-446)**. THIS prompt proves the per-CELL
//!   ITERATION completeness (0 cells missed); the every-holder completeness within a cell is the
//!   six-step seam set [`crate::erase::EraseHolders`] this drives, proven complete there.
//! - **The cross-cell transport** (the real per-cell erase endpoint over the P-CP-19 bridge) is the
//!   control-plane registry floor (global P-430) — here, as there, the registry holds in-process
//!   per-cell [`CryptoShredErase`] handles (the SAME seam; the wire is the named transport floor).

use std::collections::BTreeMap;

use myelin_tenancy::{CellId, OpaqueSubjectId, TenantId};

use crate::encryption::SubjectId;
use crate::erase::{CryptoShredErase, EpochMillis, EraseError, EraseHolders, ErasureReceipt};

/// A single cell's storage-side erase outcome within a multi-cell fan-out: WHICH cell, and the
/// storage [`ErasureReceipt`] that cell's six-step crypto-shred produced (PII-free: an opaque subject
/// id + the per-subject-DEK-destroy proof + the `0 recoverable in backup` reading). One of these per
/// cell that the fan-out reached; a cell it could NOT reach is absent and counted by
/// [`MultiCellEraseReceiptSet::cells_missed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellEraseReceipt {
    /// The cell this erase ran IN (opaque routing handle, PII-free).
    pub cell: CellId,
    /// The storage-side six-step erasure receipt this cell produced (the STOR-D4 artifact for THIS
    /// cell: the per-subject DEK destroyed + 0 recoverable in this cell's backup).
    pub receipt: ErasureReceipt,
}

impl CellEraseReceipt {
    /// `true` iff this cell's erase leg is GREEN: 0 of the subject's per-subject DEKs recoverable from
    /// this cell's backup snapshot (the crypto-shred reached this cell's backups by construction).
    pub fn is_green(&self) -> bool {
        self.receipt.is_green()
    }
}

/// **The merged per-cell erase receipt set — the storage half of the GA-D8 green artifact.** The
/// fan-out iterated `{home_cell} ∪ member_cells` (contract 10.4) and merged one [`CellEraseReceipt`]
/// per cell it reached. A COMPLETE set has one receipt per fan-out cell, **0 cells missed**, and every
/// per-cell receipt green (0 recoverable in that cell's backup). PII-free throughout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCellEraseReceiptSet {
    /// The opaque subject the multi-cell erase forgot (survives erasure — a PII-free handle).
    pub subject: OpaqueSubjectId,
    /// The tenant the erase ran under (the partition key).
    pub tenant: TenantId,
    /// The cells the fan-out iterated: `{home_cell} ∪ member_cells`, deduplicated, deterministic
    /// order. `cells_missed` is measured against THIS set (it is the set the erase had to cover).
    pub fan_out_cells: Vec<CellId>,
    /// One [`CellEraseReceipt`] per cell that produced a receipt (in `fan_out_cells` order). A cell
    /// the fan-out could not reach (no registered eraser) is ABSENT here — and counted by
    /// [`Self::cells_missed`] (never silently dropped).
    pub receipts: Vec<CellEraseReceipt>,
    /// The fan-out run timestamp (the dated artifact).
    pub ran_at: EpochMillis,
}

impl MultiCellEraseReceiptSet {
    /// **The number of cells MISSED by the fan-out (GA-D8: MUST be 0).** The set-difference
    /// `fan_out_cells − {cells with a receipt}`: every cell the fan-out was supposed to iterate that
    /// did NOT produce a receipt. The single most load-bearing GA-D8 number — a missed cell in an
    /// erasure fan-out is stop-the-bleeding (EI-01 §2).
    pub fn cells_missed(&self) -> usize {
        self.fan_out_cells
            .iter()
            .filter(|c| !self.receipts.iter().any(|r| &r.cell == *c))
            .count()
    }

    /// **`true` iff the receipt set is COMPLETE (the GA-D8 gate reading):** one receipt per fan-out
    /// cell, **0 cells missed**, AND every per-cell receipt green (0 recoverable in that cell's
    /// backup). The gate reads THIS — completeness is BOTH "no cell skipped" and "every reached cell
    /// actually destroyed the key".
    pub fn is_complete(&self) -> bool {
        self.cells_missed() == 0
            && self.receipts.len() == self.fan_out_cells.len()
            && self.receipts.iter().all(CellEraseReceipt::is_green)
    }

    /// The total number of per-subject DEKs STILL recoverable from ANY cell's backup after the fan-out
    /// (summed across cells). MUST be **0** (every cell crypto-shredded the subject). A non-zero value
    /// is a RED drill: some cell's backup could resurrect the subject.
    pub fn recoverable_in_backup(&self) -> usize {
        self.receipts
            .iter()
            .map(|r| r.receipt.recoverable_in_backup)
            .sum()
    }

    /// `true` iff EVERY per-cell receipt is an idempotent re-run (the subject was already erased in
    /// every cell). Used by the idempotency drill to assert a second fan-out is a no-op across cells.
    pub fn all_re_run(&self) -> bool {
        !self.receipts.is_empty() && self.receipts.iter().all(|r| r.receipt.re_run)
    }

    /// A one-line dated PII-free summary for the GA-D8 green artifact (EI-01 §3 — observability is
    /// part of the pass). Names the opaque subject + tenant + the fan-out cell count + the receipt
    /// count + the cells-missed zero + the recoverable-in-backup zero + the verdict.
    pub fn summary(&self) -> String {
        format!(
            "GA-D8 storage multi-cell erase [t={}]: subject={} tenant={} fan_out_cells={} \
             receipts={} cells_missed={} recoverable_in_backup={} -> {}",
            self.ran_at,
            self.subject.artifact_ref().0,
            self.tenant.as_str(),
            self.fan_out_cells.len(),
            self.receipts.len(),
            self.cells_missed(),
            self.recoverable_in_backup(),
            if self.is_complete() { "GREEN" } else { "RED" },
        )
    }
}

/// One member cell's storage erase context: the cell's own [`CryptoShredErase`] (which owns that
/// cell's [`KmsEngine`] + region — a cell is a blast-radius boundary, so each holds its own keys) plus
/// the cell-local cross-holder seams the erase drives ([`EraseHolders`]). The fan-out runs THIS cell's
/// erase against THIS cell's keys; nothing crosses the cell boundary except the PII-free receipt.
pub struct CellEraseContext<'a> {
    eraser: CryptoShredErase<'a>,
    holders: EraseHolders<'a>,
}

impl<'a> CellEraseContext<'a> {
    /// Build a cell's erase context from its [`CryptoShredErase`] (over that cell's KMS engine +
    /// region) and the cell-local [`EraseHolders`] seams the erase drives.
    pub fn new(eraser: CryptoShredErase<'a>, holders: EraseHolders<'a>) -> CellEraseContext<'a> {
        CellEraseContext { eraser, holders }
    }

    /// Run THIS cell's six-step crypto-shred erase for `subject` / `tenant` at `now`. Returns the
    /// cell's storage [`ErasureReceipt`], or a LOUD [`EraseError`] (the cell's erase is incomplete —
    /// the fan-out treats this exactly as a missed cell would be: NOT a completed receipt).
    fn erase(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        now: EpochMillis,
    ) -> Result<ErasureReceipt, EraseError> {
        self.eraser.erase(subject, tenant, &self.holders, now)
    }
}

/// **The storage-side multi-cell DSR erase fan-out (P-ST-33 / GA-D8).** Holds one
/// [`CellEraseContext`] per reachable cell (keyed by opaque [`CellId`]) and drives the crypto-shred
/// erase across `{home_cell} ∪ member_cells` (contract 10.4), merging a complete per-cell receipt set.
///
/// It owns NO cross-cell store and reaches into NO cell's keys directly — each cell's erase runs
/// against THAT cell's own [`CryptoShredErase`] / [`KmsEngine`] (the cell-local discipline at erase
/// grain). A cell with no registered context is a MISSED cell (counted, never dropped).
#[derive(Default)]
pub struct MultiCellEraseFanOut<'a> {
    /// The per-cell erase contexts, keyed by opaque [`CellId`]. A `BTreeMap` so the iteration order is
    /// deterministic (a deterministic merged receipt set).
    cells: BTreeMap<CellId, CellEraseContext<'a>>,
}

impl<'a> MultiCellEraseFanOut<'a> {
    /// A fresh, empty fan-out (no cells registered yet).
    pub fn new() -> MultiCellEraseFanOut<'a> {
        MultiCellEraseFanOut {
            cells: BTreeMap::new(),
        }
    }

    /// Register the erase context for `cell` (that cell's own [`CryptoShredErase`] + cell-local
    /// [`EraseHolders`]). In production each cell exposes its erase endpoint over the P-CP-19 bridge;
    /// on this floor the registry holds the in-process per-cell handles (the SAME seam — the wire is
    /// the named transport floor, exactly as the control-plane bridge registry).
    pub fn register(&mut self, cell: CellId, ctx: CellEraseContext<'a>) {
        self.cells.insert(cell, ctx);
    }

    /// The cells currently registered (reachable), in deterministic order. Used by the drill to assert
    /// which cells the fan-out CAN reach versus the `{home_cell} ∪ member_cells` it MUST reach.
    pub fn registered_cells(&self) -> Vec<CellId> {
        self.cells.keys().cloned().collect()
    }

    /// **`fan_out(subject, tenant, home_cell, member_cells, now)` — the storage multi-cell erase
    /// fan-out mechanism (contract 10.4; GA-D8).** Iterate `{home_cell} ∪ member_cells` (deduplicated,
    /// deterministic order), run each reachable cell's six-step crypto-shred erase, and merge the
    /// per-cell receipts into a [`MultiCellEraseReceiptSet`].
    ///
    /// **Completeness defence (the load-bearing zero):** a cell with NO registered context is recorded
    /// honestly in `fan_out_cells` but contributes NO receipt — so [`MultiCellEraseReceiptSet::cells_missed`]
    /// counts it (never a silently-dropped cell). A cell whose erase returns a LOUD [`EraseError`]
    /// (an incomplete in-cell erase) ALSO contributes no receipt — it is a missed cell, not a partial
    /// claim (a partial erase is a retry, never "assume erased"; the in-cell erase already refuses to
    /// record an incomplete erasure to its ledger).
    ///
    /// **Idempotent:** re-running for an already-erased subject re-runs each cell's idempotent erase;
    /// every per-cell receipt flips `re_run = true`, the post-condition still holds, `cells_missed`
    /// stays 0.
    pub fn fan_out(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        home_cell: &CellId,
        member_cells: &[CellId],
        now: EpochMillis,
    ) -> MultiCellEraseReceiptSet {
        // {home_cell} ∪ member_cells — the home cell is ALWAYS in the fan-out set (a subject's home
        // cell must be erased even when member_cells does not list it). Deduplicated + deterministic
        // so a cell is never double-erased and the merged set is reproducible.
        let mut fan_out_cells: Vec<CellId> = Vec::new();
        for c in std::iter::once(home_cell).chain(member_cells.iter()) {
            if !fan_out_cells.contains(c) {
                fan_out_cells.push(c.clone());
            }
        }

        let mut receipts = Vec::with_capacity(fan_out_cells.len());
        for cell in &fan_out_cells {
            if let Some(ctx) = self.cells.get(cell) {
                // The erase runs IN this cell, against this cell's own keys. Only a COMPLETED receipt
                // counts; an incomplete in-cell erase (a loud EraseError) yields NO receipt, so the
                // cell reads as MISSED (never a partial "assume erased").
                if let Ok(receipt) = ctx.erase(subject, tenant, now) {
                    receipts.push(CellEraseReceipt {
                        cell: cell.clone(),
                        receipt,
                    });
                }
            }
            // A cell with no context is a MISSED cell — counted by `cells_missed`, never dropped.
        }

        MultiCellEraseReceiptSet {
            subject: OpaqueSubjectId::from_ref(myelin_tenancy::ArtifactRef(subject.0.clone())),
            tenant: tenant.clone(),
            fan_out_cells,
            receipts,
            ran_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::ColumnCryptor;
    use crate::erase::{BusErase, ErasureLedgerSink, PseudonymShred, RefsTombstone, SearchPurge};
    use crate::kms::{DekId, KekId, KeyClass, KmsEngine};
    use myelin_gdpr::ErasureMethod;
    use myelin_tenancy::{ArtifactRef, Region};
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }
    fn r() -> Region {
        Region("fr-par".to_string())
    }
    fn cell(s: &str) -> CellId {
        CellId::from_token(s)
    }

    // ── always-succeed cell-local seams (the in-cell six-step erase drives these) ──
    struct OkPseudonym;
    impl PseudonymShred for OkPseudonym {
        fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    struct OkSearch;
    impl SearchPurge for OkSearch {
        fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    struct OkRefs;
    impl RefsTombstone for OkRefs {
        fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    struct OkBus;
    impl BusErase for OkBus {
        fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Ok(())
        }
    }
    /// A pseudonym seam that FAILS (to drive an in-cell INCOMPLETE erase → a missed cell).
    struct FailPseudonym;
    impl PseudonymShred for FailPseudonym {
        fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            Err(EraseError::PseudonymShred("cell id store down".into()))
        }
    }

    #[derive(Default)]
    struct Ledger {
        erased: RefCell<BTreeSet<String>>,
    }
    impl ErasureLedgerSink for Ledger {
        fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.erased.borrow_mut().insert(subject.0.clone());
        }
        fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
            self.erased.borrow().contains(&subject.0)
        }
    }

    /// Stand up a per-cell KMS engine with the tenant KEK + a sealed per-subject column, so each
    /// cell's erase has a real key to destroy and a real backup to probe.
    fn cell_kms(tenant: &TenantId, subject: &SubjectId, plaintext: &[u8]) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()));
        let cryptor = ColumnCryptor::new(&kms, r());
        cryptor
            .encrypt(
                tenant,
                Some(subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                plaintext,
            )
            .expect("seal a per-subject column in this cell");
        kms
    }

    fn ok_holders<'a>(ledger: &'a Ledger) -> EraseHolders<'a> {
        // Leak the always-ok seams (test-only; they are zero-sized and live for the test).
        EraseHolders {
            pseudonym: Box::leak(Box::new(OkPseudonym)),
            search: Box::leak(Box::new(OkSearch)),
            refs: Box::leak(Box::new(OkRefs)),
            bus: Box::leak(Box::new(OkBus)),
            ledger,
            git_reach: None,
        }
    }

    // ───────────── GA-D8: the fan-out iterates {home_cell} ∪ member_cells, 0 missed ─────────────

    #[test]
    fn fan_out_iterates_home_plus_member_cells_with_zero_missed() {
        let tenant = t("01J0ACME");
        let subj = SubjectId::new("u-multi");

        // Three cells, each with its OWN KMS engine (a cell is a key-blast boundary).
        let kms_b = cell_kms(&tenant, &subj, b"alice in cell-b");
        let kms_c = cell_kms(&tenant, &subj, b"alice in cell-c");
        let kms_d = cell_kms(&tenant, &subj, b"alice in cell-d");
        let led_b = Ledger::default();
        let led_c = Ledger::default();
        let led_d = Ledger::default();

        let mut fanout = MultiCellEraseFanOut::new();
        fanout.register(
            cell("cell-b"),
            CellEraseContext::new(CryptoShredErase::new(&kms_b, r()), ok_holders(&led_b)),
        );
        fanout.register(
            cell("cell-c"),
            CellEraseContext::new(CryptoShredErase::new(&kms_c, r()), ok_holders(&led_c)),
        );
        fanout.register(
            cell("cell-d"),
            CellEraseContext::new(CryptoShredErase::new(&kms_d, r()), ok_holders(&led_d)),
        );

        // home = cell-b; member_cells = {cell-c, cell-d}.
        let set = fanout.fan_out(
            &subj,
            &tenant,
            &cell("cell-b"),
            &[cell("cell-c"), cell("cell-d")],
            1_000,
        );

        // The fan-out covered {home} ∪ member_cells = 3 cells; one receipt per cell; 0 missed.
        assert_eq!(
            set.fan_out_cells.len(),
            3,
            "{{home}} ∪ member_cells = 3 cells"
        );
        assert_eq!(set.receipts.len(), 3, "one receipt per cell");
        assert_eq!(set.cells_missed(), 0, "0 cells missed (the GA-D8 zero)");
        assert_eq!(
            set.recoverable_in_backup(),
            0,
            "0 recoverable in any cell's backup"
        );
        assert!(
            set.is_complete(),
            "the merged receipt set is COMPLETE + green"
        );
        // Every cell actually destroyed the subject's per-subject DEK this pass.
        for rec in &set.receipts {
            assert!(
                rec.receipt.dek_destroyed_now,
                "{:?} destroyed the DEK",
                rec.cell
            );
            assert!(rec.is_green(), "{:?} is green (0 recoverable)", rec.cell);
        }
        // The destroy is strictly cell-local: cell-b destroyed it in cell-b's KMS, cell-c in cell-c's.
        let dek = DekId::new(tenant.clone(), KeyClass::Subject(subj.0.clone()));
        assert!(!kms_b.backup_snapshot().iter().any(|(d, _)| *d == dek));
        assert!(!kms_c.backup_snapshot().iter().any(|(d, _)| *d == dek));
        assert!(!kms_d.backup_snapshot().iter().any(|(d, _)| *d == dek));
    }

    // ───────────── the home cell is ALWAYS erased, even absent from member_cells ─────────────

    #[test]
    fn home_cell_is_always_in_the_fan_out_even_when_member_cells_omits_it() {
        let tenant = t("01J0ACME");
        let subj = SubjectId::new("u-home");
        let kms_b = cell_kms(&tenant, &subj, b"home data");
        let led_b = Ledger::default();

        let mut fanout = MultiCellEraseFanOut::new();
        fanout.register(
            cell("cell-b"),
            CellEraseContext::new(CryptoShredErase::new(&kms_b, r()), ok_holders(&led_b)),
        );

        // member_cells is EMPTY — but the home cell must still be erased.
        let set = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[], 1);
        assert_eq!(set.fan_out_cells, vec![cell("cell-b")]);
        assert_eq!(set.cells_missed(), 0);
        assert!(set.is_complete());
    }

    // ───────────── dedup: a cell listed in BOTH home and member is erased ONCE ─────────────

    #[test]
    fn a_cell_in_both_home_and_member_is_erased_exactly_once() {
        let tenant = t("01J0ACME");
        let subj = SubjectId::new("u-dup");
        let kms_b = cell_kms(&tenant, &subj, b"x");
        let kms_c = cell_kms(&tenant, &subj, b"y");
        let led_b = Ledger::default();
        let led_c = Ledger::default();

        let mut fanout = MultiCellEraseFanOut::new();
        fanout.register(
            cell("cell-b"),
            CellEraseContext::new(CryptoShredErase::new(&kms_b, r()), ok_holders(&led_b)),
        );
        fanout.register(
            cell("cell-c"),
            CellEraseContext::new(CryptoShredErase::new(&kms_c, r()), ok_holders(&led_c)),
        );

        // home = cell-b, member_cells lists cell-b AGAIN + cell-c → dedup to {cell-b, cell-c}.
        let set = fanout.fan_out(
            &subj,
            &tenant,
            &cell("cell-b"),
            &[cell("cell-b"), cell("cell-c")],
            1,
        );
        assert_eq!(
            set.fan_out_cells,
            vec![cell("cell-b"), cell("cell-c")],
            "the duplicate home cell is iterated ONCE"
        );
        assert_eq!(set.receipts.len(), 2);
        assert_eq!(set.cells_missed(), 0);
    }

    // ───────────── the gate is NOT vacuous: an unreachable member cell reads RED ─────────────

    #[test]
    fn an_unregistered_member_cell_is_missed_and_reads_red() {
        let tenant = t("01J0ACME");
        let subj = SubjectId::new("u-miss");
        let kms_b = cell_kms(&tenant, &subj, b"x");
        let led_b = Ledger::default();

        let mut fanout = MultiCellEraseFanOut::new();
        fanout.register(
            cell("cell-b"),
            CellEraseContext::new(CryptoShredErase::new(&kms_b, r()), ok_holders(&led_b)),
        );
        // cell-c is NOT registered — unreachable.
        let set = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[cell("cell-c")], 1);
        assert_eq!(
            set.fan_out_cells.len(),
            2,
            "both cells are in the must-cover set"
        );
        assert_eq!(
            set.receipts.len(),
            1,
            "only the home cell produced a receipt"
        );
        assert_eq!(
            set.cells_missed(),
            1,
            "the unreachable cell is MISSED (not dropped)"
        );
        assert!(!set.is_complete(), "an incomplete fan-out is RED");
    }

    // ───────────── an in-cell INCOMPLETE erase is a missed cell, never 'assume erased' ─────────────

    #[test]
    fn an_in_cell_incomplete_erase_is_a_missed_cell_not_a_partial_claim() {
        let tenant = t("01J0ACME");
        let subj = SubjectId::new("u-fail");
        let kms_b = cell_kms(&tenant, &subj, b"x");
        let kms_c = cell_kms(&tenant, &subj, b"y");
        let led_b = Ledger::default();
        let led_c = Ledger::default();

        let mut fanout = MultiCellEraseFanOut::new();
        fanout.register(
            cell("cell-b"),
            CellEraseContext::new(CryptoShredErase::new(&kms_b, r()), ok_holders(&led_b)),
        );
        // cell-c's pseudonym shred FAILS → its in-cell erase is INCOMPLETE (a loud EraseError).
        let fail_holders = EraseHolders {
            pseudonym: Box::leak(Box::new(FailPseudonym)),
            search: Box::leak(Box::new(OkSearch)),
            refs: Box::leak(Box::new(OkRefs)),
            bus: Box::leak(Box::new(OkBus)),
            ledger: &led_c,
            git_reach: None,
        };
        fanout.register(
            cell("cell-c"),
            CellEraseContext::new(CryptoShredErase::new(&kms_c, r()), fail_holders),
        );

        let set = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[cell("cell-c")], 1);
        // cell-c is in the must-cover set but produced NO receipt → MISSED (not a partial claim).
        assert_eq!(
            set.cells_missed(),
            1,
            "an incomplete in-cell erase reads as a MISSED cell"
        );
        assert!(!set.is_complete());
        // cell-c's ledger never recorded the erasure (an incomplete erase is a retry).
        assert!(
            !led_c.is_erased(&subj, &tenant),
            "an incomplete in-cell erase is NOT recorded"
        );
        // cell-c's DEK is INTACT (step 2 never ran — the loud abort happened at step 1).
        let dek = DekId::new(tenant.clone(), KeyClass::Subject(subj.0.clone()));
        assert!(
            kms_c.backup_snapshot().iter().any(|(d, _)| *d == dek),
            "cell-c's DEK is intact (its erase aborted loudly)"
        );
    }

    // ───────────── idempotency: a second fan-out is a no-op across cells ─────────────

    #[test]
    fn re_running_the_fan_out_is_a_noop_success_across_cells() {
        let tenant = t("01J0ACME");
        let subj = SubjectId::new("u-idem");
        let kms_b = cell_kms(&tenant, &subj, b"x");
        let kms_c = cell_kms(&tenant, &subj, b"y");
        let led_b = Ledger::default();
        let led_c = Ledger::default();

        let mut fanout = MultiCellEraseFanOut::new();
        fanout.register(
            cell("cell-b"),
            CellEraseContext::new(CryptoShredErase::new(&kms_b, r()), ok_holders(&led_b)),
        );
        fanout.register(
            cell("cell-c"),
            CellEraseContext::new(CryptoShredErase::new(&kms_c, r()), ok_holders(&led_c)),
        );

        // First fan-out: destroys every cell's DEK; no cell is a re-run.
        let first = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[cell("cell-c")], 1);
        assert!(first.is_complete());
        assert!(first.receipts.iter().all(|r| r.receipt.dek_destroyed_now));
        assert!(!first.all_re_run(), "the first fan-out is not a re-run");

        // Second fan-out of the SAME subject: a no-op SUCCESS — every cell re-runs (idempotent),
        // every per-cell receipt flips re_run, 0 still recoverable, 0 cells missed.
        let second = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[cell("cell-c")], 2);
        assert_eq!(second.cells_missed(), 0, "the re-run still misses 0 cells");
        assert_eq!(
            second.recoverable_in_backup(),
            0,
            "still 0 recoverable after the re-run"
        );
        assert!(second.is_complete(), "the re-run is still complete + green");
        assert!(
            second.all_re_run(),
            "every cell's second erase is an idempotent re-run"
        );
        assert!(
            second.receipts.iter().all(|r| !r.receipt.dek_destroyed_now),
            "no DEK was destroyed the second pass (already gone)"
        );
    }

    // ───────────── registered_cells reflects the registered set (kills the vec![] mutant) ─────────────

    #[test]
    fn registered_cells_lists_exactly_the_registered_cells() {
        let tenant = t("01J0ACME");
        let subj = SubjectId::new("u-reg");
        let kms_b = cell_kms(&tenant, &subj, b"x");
        let kms_c = cell_kms(&tenant, &subj, b"y");
        let led_b = Ledger::default();
        let led_c = Ledger::default();
        let mut fanout = MultiCellEraseFanOut::new();
        assert!(
            fanout.registered_cells().is_empty(),
            "empty before any register"
        );
        fanout.register(
            cell("cell-b"),
            CellEraseContext::new(CryptoShredErase::new(&kms_b, r()), ok_holders(&led_b)),
        );
        fanout.register(
            cell("cell-c"),
            CellEraseContext::new(CryptoShredErase::new(&kms_c, r()), ok_holders(&led_c)),
        );
        assert_eq!(
            fanout.registered_cells(),
            vec![cell("cell-b"), cell("cell-c")],
            "registered_cells lists exactly the two registered cells (deterministic order)"
        );
    }

    // ───────────── the summary + completeness predicates are not vacuous ─────────────

    #[test]
    fn summary_and_completeness_are_real_readings() {
        let tenant = t("01J0ACME");
        // A RED set: a fan-out cell with no receipt.
        let red = MultiCellEraseReceiptSet {
            subject: OpaqueSubjectId::from_ref(ArtifactRef("u-sum".into())),
            tenant: tenant.clone(),
            fan_out_cells: vec![cell("cell-b"), cell("cell-c")],
            receipts: vec![],
            ran_at: 7,
        };
        assert_eq!(red.cells_missed(), 2);
        assert!(!red.is_complete());
        assert!(red.summary().contains("RED"), "a red set summarises RED");
        assert!(red.summary().contains("cells_missed=2"));
        assert!(
            !red.all_re_run(),
            "an empty receipt set is not 'all re-run'"
        );

        // A receipt whose backup STILL has a recoverable DEK is NOT green even if 0 cells missed.
        let leaky = MultiCellEraseReceiptSet {
            subject: OpaqueSubjectId::from_ref(ArtifactRef("u-sum".into())),
            tenant,
            fan_out_cells: vec![cell("cell-b")],
            receipts: vec![CellEraseReceipt {
                cell: cell("cell-b"),
                receipt: ErasureReceipt {
                    subject: "u-sum".into(),
                    tenant: t("01J0ACME"),
                    dek_destroyed_now: true,
                    recoverable_in_backup: 1, // a backup could resurrect the subject
                    crypto_shred_lag_ms: 0,
                    re_run: false,
                    completed_at: 0,
                },
            }],
            ran_at: 8,
        };
        assert_eq!(leaky.cells_missed(), 0, "no cell missed");
        assert_eq!(leaky.recoverable_in_backup(), 1);
        assert!(
            !leaky.is_complete(),
            "0 cells missed but a recoverable DEK → NOT complete (completeness is BOTH)"
        );
        assert!(leaky.summary().contains("RED"));
    }
}
