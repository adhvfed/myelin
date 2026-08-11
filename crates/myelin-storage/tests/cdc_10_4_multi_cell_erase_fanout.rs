use myelin_control_plane::{CellDsrReceipt, MultiCellDsrReceiptSet};
use myelin_events::Timestamp;
use myelin_gdpr::ErasureMethod;
use myelin_storage::BusErase;
use myelin_storage::{
    CellEraseContext, ColumnCryptor, CryptoShredErase, EpochMillis, EraseError, EraseHolders,
    ErasureLedgerSink, KekId, KmsEngine, MultiCellEraseFanOut, MultiCellEraseReceiptSet,
    PseudonymShred, RefsTombstone, SearchPurge, SubjectId,
};
use myelin_tenancy::{ArtifactRef, CellId, OpaqueSubjectId, Region, TenantId};
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
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    let cryptor = ColumnCryptor::new(&kms, region());
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

fn lower_to_control_plane(set: &MultiCellEraseReceiptSet) -> MultiCellDsrReceiptSet {
    let subject = OpaqueSubjectId::from_ref(ArtifactRef(set.subject.artifact_ref().0.clone()));
    let receipts = set
        .receipts
        .iter()
        .map(|r| CellDsrReceipt {
            cell: r.cell.clone(),
            subject: subject.clone(),
            receipt: format!(
                "storage-erase:{}:{}:destroyed={}:recoverable={}",
                r.cell.as_str(),
                r.receipt.subject,
                r.receipt.dek_destroyed_now,
                r.receipt.recoverable_in_backup
            ),
        })
        .collect();
    MultiCellDsrReceiptSet {
        subject,
        tenant: set.tenant.clone(),
        fan_out_cells: set.fan_out_cells.clone(),
        receipts,
        ran_at: Timestamp(format!("t={}", set.ran_at)),
    }
}

#[test]
fn cdc_10_4_storage_fan_out_lowers_to_control_plane_with_agreeing_zeros() {
    let tenant = TenantId::from_token("01J0ACME");
    let subj = SubjectId::new("p1");
    let kms_b = cell_kms(&tenant, &subj, b"b");
    let kms_c = cell_kms(&tenant, &subj, b"c");
    let kms_d = cell_kms(&tenant, &subj, b"d");
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

    let storage_set = fanout.fan_out(
        &subj,
        &tenant,
        &cell("cell-b"),
        &[cell("cell-c"), cell("cell-d")],
        1_000,
    );
    let cp_set = lower_to_control_plane(&storage_set);

    assert_eq!(
        storage_set.cells_missed(),
        cp_set.cells_missed(),
        "storage and control-plane cells_missed AGREE"
    );
    assert_eq!(storage_set.cells_missed(), 0);
    assert_eq!(
        storage_set.is_complete(),
        cp_set.is_complete(),
        "storage and control-plane is_complete AGREE"
    );
    assert!(cp_set.is_complete());
    assert_eq!(
        cp_set.fan_out_cells, storage_set.fan_out_cells,
        "same cell set"
    );
    assert_eq!(
        cp_set.receipts.len(),
        storage_set.receipts.len(),
        "same receipt count"
    );
}

#[test]
fn cdc_10_4_a_missed_cell_reads_red_on_both_legs() {
    let tenant = TenantId::from_token("01J0ACME");
    let subj = SubjectId::new("p1");
    let kms_b = cell_kms(&tenant, &subj, b"b");
    let led_b = Ledger::default();

    let mut fanout = MultiCellEraseFanOut::new();
    fanout.register(
        cell("cell-b"),
        CellEraseContext::new(CryptoShredErase::new(&kms_b, region()), ok_holders(&led_b)),
    );
    let storage_set = fanout.fan_out(&subj, &tenant, &cell("cell-b"), &[cell("cell-c")], 1);
    let cp_set = lower_to_control_plane(&storage_set);

    assert_eq!(storage_set.cells_missed(), 1, "storage misses cell-c");
    assert_eq!(
        cp_set.cells_missed(),
        1,
        "control plane ALSO sees cell-c missed"
    );
    assert!(!storage_set.is_complete());
    assert!(
        !cp_set.is_complete(),
        "RED lowers to RED - the tripwire is consistent"
    );
}
