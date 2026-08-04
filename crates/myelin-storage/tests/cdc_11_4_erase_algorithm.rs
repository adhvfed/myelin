use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    BusErase, ColumnCryptor, CryptoShredErase, EpochMillis, EraseError, EraseHolders,
    ErasureLedgerSink, KekId, KmsEngine, PseudonymShred, RefsTombstone, SearchPurge, SubjectId,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

fn region() -> Region {
    Region("eu-west".into())
}

#[derive(Default)]
struct OrchestratorWiring {
    order: RefCell<Vec<&'static str>>,
    erased: RefCell<BTreeSet<String>>,
}
impl PseudonymShred for OrchestratorWiring {
    fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.order.borrow_mut().push("1:pseudonym");
        Ok(())
    }
}
impl SearchPurge for OrchestratorWiring {
    fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.order.borrow_mut().push("3:search");
        Ok(())
    }
}
impl RefsTombstone for OrchestratorWiring {
    fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.order.borrow_mut().push("4:refs");
        Ok(())
    }
}
impl BusErase for OrchestratorWiring {
    fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.order.borrow_mut().push("5:bus");
        Ok(())
    }
}
impl ErasureLedgerSink for OrchestratorWiring {
    fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
        self.order.borrow_mut().push("6:ledger");
        self.erased.borrow_mut().insert(subject.0.clone());
    }
    fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
        self.erased.borrow().contains(&subject.0)
    }
}

fn engine_with_subject_column(tenant: &TenantId, subject: &SubjectId) -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()));
    let cryptor = ColumnCryptor::new(&kms, region());
    cryptor
        .encrypt(
            tenant,
            Some(subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"alice's free-text bio",
        )
        .expect("seal a per-subject column");
    kms
}

#[test]
fn cdc_11_4_dsr_orchestrator_calls_erase_and_the_six_steps_run_in_order() {
    let tenant = TenantId("acme".into());
    let subject = SubjectId::new("u-42");
    let kms = engine_with_subject_column(&tenant, &subject);
    let eraser = CryptoShredErase::new(&kms, region());

    let wiring = OrchestratorWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring,
        search: &wiring,
        refs: &wiring,
        bus: &wiring,
        ledger: &wiring,
        git_reach: None,
    };
    let receipt = eraser
        .erase(&subject, &tenant, &holders, 1_000)
        .expect("the DSR orchestrator's erase succeeds");

    assert_eq!(
        wiring.order.borrow().as_slice(),
        ["1:pseudonym", "3:search", "4:refs", "5:bus", "6:ledger"],
        "the six steps run in §5.2 order"
    );
    assert!(
        receipt.dek_destroyed_now,
        "step 2 destroyed the per-subject DEK"
    );
    assert_eq!(
        receipt.recoverable_in_backup, 0,
        "STOR-D4: 0 recoverable in backup"
    );
    assert!(receipt.is_green());
}

#[test]
fn cdc_11_4_erase_is_idempotent_for_the_orchestrator() {
    let tenant = TenantId("acme".into());
    let subject = SubjectId::new("u-retry");
    let kms = engine_with_subject_column(&tenant, &subject);
    let eraser = CryptoShredErase::new(&kms, region());
    let wiring = OrchestratorWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring,
        search: &wiring,
        refs: &wiring,
        bus: &wiring,
        ledger: &wiring,
        git_reach: None,
    };

    let r1 = eraser
        .erase(&subject, &tenant, &holders, 1)
        .expect("first erase");
    let r2 = eraser
        .erase(&subject, &tenant, &holders, 2)
        .expect("re-erase is a no-op SUCCESS");
    assert!(r1.dek_destroyed_now && !r2.dek_destroyed_now);
    assert!(r2.re_run, "the second pass is flagged as a re-run");
    assert!(r2.is_green());
}
