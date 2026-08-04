use myelin_gdpr::ErasureMethod;
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::BusErase;
use myelin_storage::{
    CellEraseContext, ColumnCryptor, CryptoShredErase, EpochMillis, EraseError, EraseHolders,
    ErasureLedgerSink, KekId, KmsEngine, MultiCellEraseFanOut, PseudonymShred, RefsTombstone,
    SearchPurge, SubjectId,
};
use myelin_tenancy::{CellId, Region, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

fn region() -> Region {
    Region("fr-par".into())
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
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    let cryptor = ColumnCryptor::new(&kms, region());
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

fn ok_holders(ledger: &Ledger) -> EraseHolders<'_> {
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
fn ga_d8_storage_multi_cell_erase_misses_zero_cells() {
    let tenant = TenantId::from_token("01J0ACME");
    let subj = SubjectId::new("p1");

    let kms_b = cell_kms(&tenant, &subj, b"p1 in cell-b");
    let kms_c = cell_kms(&tenant, &subj, b"p1 in cell-c");
    let kms_d = cell_kms(&tenant, &subj, b"p1 in cell-d");
    let led_b = Ledger::default();
    let led_c = Ledger::default();
    let led_d = Ledger::default();

    let mut fanout = MultiCellEraseFanOut::new();
    fanout.register(
        cell("cell-b"),
        CellEraseContext::new(CryptoShredErase::new(&kms_b, region()), ok_holders(&led_b)),
    );
    fanout.register(
        cell("cell-c"),
        CellEraseContext::new(CryptoShredErase::new(&kms_c, region()), ok_holders(&led_c)),
    );
    fanout.register(
        cell("cell-d"),
        CellEraseContext::new(CryptoShredErase::new(&kms_d, region()), ok_holders(&led_d)),
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
    let cells_missed = set.cells_missed();
    assert_eq!(cells_missed, 0, "0 cells missed (the GA-D8 zero)");
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
            "{:?} crypto-shredded the subject",
            rec.cell
        );
        assert!(rec.is_green(), "{:?} is green (0 recoverable)", rec.cell);
    }

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, cells_missed as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-445 GA-D8 GREEN 2026-06-24] storage multi-cell DSR erase: a multi-cell erasure iterated \
         {{home_cell}} ∪ member_cells = {} cells (cell-b ∪ cell-c, cell-d), each ran its six-step \
         crypto-shred against THAT cell's own keys, merged a COMPLETE per-cell receipt set ({} \
         receipts), cells_missed={} (the GA-D8 zero), recoverable_in_backup={}. {}",
        set.fan_out_cells.len(),
        set.receipts.len(),
        cells_missed,
        set.recoverable_in_backup(),
        set.summary(),
    );
}

#[test]
fn ga_d8_gate_is_not_vacuous_an_unreachable_cell_reads_red() {
    let tenant = TenantId::from_token("01J0ACME");
    let subj = SubjectId::new("p1");
    let kms_b = cell_kms(&tenant, &subj, b"x");
    let led_b = Ledger::default();

    let mut fanout = MultiCellEraseFanOut::new();
    fanout.register(
        cell("cell-b"),
        CellEraseContext::new(CryptoShredErase::new(&kms_b, region()), ok_holders(&led_b)),
    );
    let set = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[cell("cell-c")], 1);
    let cells_missed = set.cells_missed();
    assert_eq!(
        cells_missed, 1,
        "the unreachable cell is MISSED (not dropped)"
    );
    assert!(!set.is_complete(), "an incomplete fan-out is RED");

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, cells_missed as i64);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a missed cell in an erasure fan-out MUST read RED - the GA-D8 zero is a real tripwire"
    );
}

#[test]
fn ga_d8_storage_multi_cell_erase_is_idempotent() {
    let tenant = TenantId::from_token("01J0ACME");
    let subj = SubjectId::new("p1");
    let kms_b = cell_kms(&tenant, &subj, b"x");
    let kms_c = cell_kms(&tenant, &subj, b"y");
    let led_b = Ledger::default();
    let led_c = Ledger::default();

    let mut fanout = MultiCellEraseFanOut::new();
    fanout.register(
        cell("cell-b"),
        CellEraseContext::new(CryptoShredErase::new(&kms_b, region()), ok_holders(&led_b)),
    );
    fanout.register(
        cell("cell-c"),
        CellEraseContext::new(CryptoShredErase::new(&kms_c, region()), ok_holders(&led_c)),
    );

    let first = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[cell("cell-c")], 1);
    assert!(first.is_complete());
    assert!(!first.all_re_run());

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

    println!(
        "[P-445 GA-D8 idempotency GREEN 2026-06-24] storage multi-cell erase re-ran across {} cells: \
         every per-cell receipt flipped re_run, cells_missed=0, recoverable_in_backup=0 (a re-erase \
         is a no-op success per cell, never an error).",
        second.fan_out_cells.len()
    );
}
