use std::collections::BTreeMap;

use myelin_tenancy::{CellId, OpaqueSubjectId, TenantId};

use crate::encryption::SubjectId;
use crate::erase::{CryptoShredErase, EpochMillis, EraseError, EraseHolders, ErasureReceipt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellEraseReceipt {
    pub cell: CellId,
    pub receipt: ErasureReceipt,
}

impl CellEraseReceipt {
    pub fn is_green(&self) -> bool {
        self.receipt.is_green()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCellEraseReceiptSet {
    pub subject: OpaqueSubjectId,
    pub tenant: TenantId,
    pub fan_out_cells: Vec<CellId>,
    pub receipts: Vec<CellEraseReceipt>,
    pub ran_at: EpochMillis,
}

impl MultiCellEraseReceiptSet {
    pub fn cells_missed(&self) -> usize {
        self.fan_out_cells
            .iter()
            .filter(|c| !self.receipts.iter().any(|r| &r.cell == *c))
            .count()
    }

    pub fn is_complete(&self) -> bool {
        self.cells_missed() == 0
            && self.receipts.len() == self.fan_out_cells.len()
            && self.receipts.iter().all(CellEraseReceipt::is_green)
    }

    pub fn recoverable_in_backup(&self) -> usize {
        self.receipts
            .iter()
            .map(|r| r.receipt.recoverable_in_backup)
            .sum()
    }

    pub fn all_re_run(&self) -> bool {
        !self.receipts.is_empty() && self.receipts.iter().all(|r| r.receipt.re_run)
    }

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

pub struct CellEraseContext<'a> {
    eraser: CryptoShredErase<'a>,
    holders: EraseHolders<'a>,
}

impl<'a> CellEraseContext<'a> {
    pub fn new(eraser: CryptoShredErase<'a>, holders: EraseHolders<'a>) -> CellEraseContext<'a> {
        CellEraseContext { eraser, holders }
    }

    fn erase(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        now: EpochMillis,
    ) -> Result<ErasureReceipt, EraseError> {
        self.eraser.erase(subject, tenant, &self.holders, now)
    }
}

#[derive(Default)]
pub struct MultiCellEraseFanOut<'a> {
    cells: BTreeMap<CellId, CellEraseContext<'a>>,
}

impl<'a> MultiCellEraseFanOut<'a> {
    pub fn new() -> MultiCellEraseFanOut<'a> {
        MultiCellEraseFanOut {
            cells: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, cell: CellId, ctx: CellEraseContext<'a>) {
        self.cells.insert(cell, ctx);
    }

    pub fn registered_cells(&self) -> Vec<CellId> {
        self.cells.keys().cloned().collect()
    }

    pub fn fan_out(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        home_cell: &CellId,
        member_cells: &[CellId],
        now: EpochMillis,
    ) -> MultiCellEraseReceiptSet {
        let mut fan_out_cells: Vec<CellId> = Vec::new();
        for c in std::iter::once(home_cell).chain(member_cells.iter()) {
            if !fan_out_cells.contains(c) {
                fan_out_cells.push(c.clone());
            }
        }

        let mut receipts = Vec::with_capacity(fan_out_cells.len());
        for cell in &fan_out_cells {
            if let Some(ctx) = self.cells.get(cell) {
                if let Ok(receipt) = ctx.erase(subject, tenant, now) {
                    receipts.push(CellEraseReceipt {
                        cell: cell.clone(),
                        receipt,
                    });
                }
            }
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

    fn cell_kms(tenant: &TenantId, subject: &SubjectId, plaintext: &[u8]) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()))
            .expect("seed the in-memory KEK");
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
        EraseHolders {
            pseudonym: Box::leak(Box::new(OkPseudonym)),
            search: Box::leak(Box::new(OkSearch)),
            refs: Box::leak(Box::new(OkRefs)),
            bus: Box::leak(Box::new(OkBus)),
            ledger,
            git_reach: None,
        }
    }

    #[test]
    fn fan_out_iterates_home_plus_member_cells_with_zero_missed() {
        let tenant = t("01J0ACME");
        let subj = SubjectId::new("u-multi");

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

        let set = fanout.fan_out(
            &subj,
            &tenant,
            &cell("cell-b"),
            &[cell("cell-c"), cell("cell-d")],
            1_000,
        );

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
        for rec in &set.receipts {
            assert!(
                rec.receipt.dek_destroyed_now,
                "{:?} destroyed the DEK",
                rec.cell
            );
            assert!(rec.is_green(), "{:?} is green (0 recoverable)", rec.cell);
        }
        let dek = DekId::new(tenant.clone(), KeyClass::Subject(subj.0.clone()));
        assert!(!kms_b.backup_snapshot().iter().any(|(d, _)| *d == dek));
        assert!(!kms_c.backup_snapshot().iter().any(|(d, _)| *d == dek));
        assert!(!kms_d.backup_snapshot().iter().any(|(d, _)| *d == dek));
    }

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

        let set = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[], 1);
        assert_eq!(set.fan_out_cells, vec![cell("cell-b")]);
        assert_eq!(set.cells_missed(), 0);
        assert!(set.is_complete());
    }

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
        assert_eq!(
            set.cells_missed(),
            1,
            "an incomplete in-cell erase reads as a MISSED cell"
        );
        assert!(!set.is_complete());
        assert!(
            !led_c.is_erased(&subj, &tenant),
            "an incomplete in-cell erase is NOT recorded"
        );
        let dek = DekId::new(tenant.clone(), KeyClass::Subject(subj.0.clone()));
        assert!(
            kms_c.backup_snapshot().iter().any(|(d, _)| *d == dek),
            "cell-c's DEK is intact (its erase aborted loudly)"
        );
    }

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

        let first = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[cell("cell-c")], 1);
        assert!(first.is_complete());
        assert!(first.receipts.iter().all(|r| r.receipt.dek_destroyed_now));
        assert!(!first.all_re_run(), "the first fan-out is not a re-run");

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

    #[test]
    fn summary_and_completeness_are_real_readings() {
        let tenant = t("01J0ACME");
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
                    recoverable_in_backup: 1,
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
