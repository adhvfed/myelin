//! Contract 11.4 CDC pair — the crypto-shred `erase(subject, tenant)` six-step algorithm
//! (P-ST-09 / global P-099, the erase-algorithm half of row 11.4).
//!
//! Row 11.4's erase-algorithm half is the storage-side MECHANISM behind the DSR orchestrator. This
//! CDC pair pins the seam between:
//!   - the **PROVIDER** = `myelin-storage` — [`CryptoShredErase::erase`] (the six steps in §5.2
//!     order, idempotent, loud-on-partial-failure), step 2 (`KMS.destroy(per_subject_DEK)`) owned
//!     in-crate, the cross-holder steps (1/3/4/5/6) driven through the seam traits;
//!   - the **CONSUMER** = the DSR ORCHESTRATOR (GDPR 10.1/10.11) — it wires the real subsystem
//!     holders behind the five seams ([`PseudonymShred`] → Id, [`SearchPurge`] → Search,
//!     [`RefsTombstone`] → Refs, [`BusErase`] → Bus, [`ErasureLedgerSink`] → the 10.8 ledger) and
//!     calls `erase(subject, tenant)`, expecting the six steps to run in order and the per-subject
//!     ciphertext to become unrecoverable (live + backup).
//!
//! If the algorithm's step set/order, its idempotency, or its 0-recoverable post-condition drifts,
//! this stops passing — exactly the consumer-driven contract the DSR orchestrator depends on.

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

// ── The CONSUMER's wiring: the DSR orchestrator's five seam adapters (deterministic doubles). ──

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

    // The DSR orchestrator wires ONE struct as all five seams and calls erase.
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

    // The six steps ran in the §5.2 order (step 2 = KMS, owned; observed via dek_destroyed_now).
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
    // The orchestrator MUST be able to retry a (partially-)failed DSR without an error on the second
    // pass — re-erasing an already-erased subject is a no-op success (per-effect idempotency).
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
