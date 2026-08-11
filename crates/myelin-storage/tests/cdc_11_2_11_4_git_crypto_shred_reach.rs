use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    BlobShredReach, BusErase, ColumnCryptor, CryptoShredErase, EpochMillis, EraseError,
    EraseHolders, ErasureLedgerSink, GitCryptoShredReach, GitResidual, KekId, KeyClass, KmsEngine,
    PseudonymShred, RefsTombstone, SearchPurge, SubjectId,
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

fn engine_with_subject_and_blob_dek(tenant: &TenantId, subject: &SubjectId) -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    let cryptor = ColumnCryptor::new(&kms, region());
    cryptor
        .encrypt(
            tenant,
            Some(subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"alice's commit message body (free text)",
        )
        .expect("seal a per-subject column");
    kms.ensure_dek(tenant, &region(), KeyClass::Blob)
        .expect("blob dek");
    kms
}

#[test]
fn cdc_git_reach_runs_in_the_erase_crypto_shred_step_for_a_commit_author() {
    let tenant = TenantId("acme".into());
    let subject = SubjectId::new("u-commit-author");
    let kms = engine_with_subject_and_blob_dek(&tenant, &subject);
    let eraser = CryptoShredErase::new(&kms, region());
    let git_reach = GitCryptoShredReach::new(&kms, region());

    let wiring = OrchestratorWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring,
        search: &wiring,
        refs: &wiring,
        bus: &wiring,
        ledger: &wiring,
        git_reach: Some(&git_reach),
    };
    let receipt = eraser
        .erase(&subject, &tenant, &holders, 1_000)
        .expect("the commit author's erase succeeds");

    assert_eq!(
        wiring.order.borrow().as_slice(),
        ["1:pseudonym", "3:search", "4:refs", "5:bus", "6:ledger"],
    );
    assert!(
        receipt.dek_destroyed_now,
        "step 2 destroyed the per-subject DEK"
    );
    assert_eq!(
        receipt.recoverable_in_backup, 0,
        "the per-subject ciphertext is 0 recoverable"
    );

    let blob_dek = myelin_storage::DekId::new(tenant.clone(), KeyClass::Blob);
    assert!(
        !kms.backup_snapshot().iter().any(|(d, _)| *d == blob_dek),
        "the git crypto-shred reach destroyed the per-tenant blob DEK (0 recoverable in backup)"
    );
}

#[test]
fn cdc_git_reach_post_condition_is_verified_not_assumed() {
    let tenant = TenantId("acme".into());
    let subject = SubjectId::new("u-author");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&tenant, &region(), KeyClass::Blob)
        .expect("blob dek");
    let git_reach = GitCryptoShredReach::new(&kms, region());

    assert!(git_reach.shred_blob_tier(&subject, &tenant).is_ok());

    let receipt = git_reach.shred_git_structures(&tenant);
    assert_eq!(receipt.residual, GitResidual::PseudonymousByDefault);
    assert!(receipt.is_green());
}

#[test]
fn cdc_subject_with_no_git_content_skips_the_reach_no_op() {
    let tenant = TenantId("acme".into());
    let subject = SubjectId::new("u-chat-only");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    let cryptor = ColumnCryptor::new(&kms, region());
    cryptor
        .encrypt(
            &tenant,
            Some(&subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"chat body",
        )
        .expect("seal");
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
        .erase(&subject, &tenant, &holders, 1)
        .expect("erase without git reach");
    assert!(
        receipt.dek_destroyed_now,
        "the per-subject free-text DEK was still destroyed"
    );
    assert!(receipt.is_green());
}
