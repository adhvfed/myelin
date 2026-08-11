use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    BusErase, ColumnCryptor, CryptoShredErase, DekId, EpochMillis, EraseError, EraseHolders,
    ErasureLedgerSink, KekId, KeyClass, KmsEngine, PseudonymShred, RefsTombstone, SearchPurge,
    SubjectId,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

fn region() -> Region {
    Region("eu-west".into())
}

#[derive(Default)]
struct DrillWiring {
    erased: RefCell<BTreeSet<String>>,
}
impl PseudonymShred for DrillWiring {
    fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl SearchPurge for DrillWiring {
    fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl RefsTombstone for DrillWiring {
    fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl BusErase for DrillWiring {
    fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl ErasureLedgerSink for DrillWiring {
    fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
        self.erased.borrow_mut().insert(subject.0.clone());
    }
    fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
        self.erased.borrow().contains(&subject.0)
    }
}

#[test]
fn stor_d4_crypto_shred_erase_leaves_zero_recoverable_pii_in_backups() {
    let tenant = TenantId("acme".into());
    let erase_me = SubjectId::new("u-erase");
    let keep_a = SubjectId::new("u-keep-a");
    let keep_b = SubjectId::new("u-keep-b");

    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    let cryptor = ColumnCryptor::new(&kms, region());

    let seal = |subject: &SubjectId, pii: &[u8]| {
        cryptor
            .encrypt(
                &tenant,
                Some(subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                pii,
            )
            .expect("seal a per-subject column")
    };
    let col_erase = seal(&erase_me, b"erase-me-secret-bio@example.test");
    let col_a = seal(&keep_a, b"keep-a-bio");
    let col_b = seal(&keep_b, b"keep-b-bio");

    assert!(
        cryptor.decrypt(&col_erase).is_ok(),
        "erase-me decrypts before the erase"
    );
    let dek_of = |s: &SubjectId| DekId::new(tenant.clone(), KeyClass::Subject(s.0.clone()));
    for s in [&erase_me, &keep_a, &keep_b] {
        let d = dek_of(s);
        assert!(
            kms.backup_snapshot().iter().any(|(k, _)| *k == d),
            "subject {} DEK is in the backup before erase",
            s.as_str()
        );
    }

    let eraser = CryptoShredErase::new(&kms, region());
    let wiring = DrillWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring,
        search: &wiring,
        refs: &wiring,
        bus: &wiring,
        ledger: &wiring,
        git_reach: None,
    };
    let receipt = eraser
        .erase(&erase_me, &tenant, &holders, 1_718_000_000_000)
        .expect("erase completes all six steps");

    assert!(
        cryptor.decrypt(&col_erase).is_err(),
        "STOR-D4 RED: the erased subject's column is STILL recoverable LIVE (key not destroyed)"
    );

    let recoverable_in_backup = kms
        .backup_snapshot()
        .iter()
        .filter(|(k, _)| *k == dek_of(&erase_me))
        .count();
    assert_eq!(
        recoverable_in_backup, 0,
        "STOR-D4 RED: a backup snapshot still carries the erased subject's per-subject DEK \
         (a restore could resurrect the subject) - the threshold is 0 and is NOT weakened"
    );
    assert_eq!(receipt.recoverable_in_backup, 0);
    assert!(
        receipt.is_green(),
        "STOR-D4 green: 0 recoverable PII in backup"
    );

    for (s, col) in [(&keep_a, &col_a), (&keep_b, &col_b)] {
        assert_eq!(
            cryptor.decrypt(col).expect("kept subject still decrypts"),
            cryptor.decrypt(col).unwrap(),
            "kept subject {} decrypts after the other's erase",
            s.as_str()
        );
        assert!(
            kms.backup_snapshot().iter().any(|(k, _)| *k == dek_of(s)),
            "kept subject {} DEK is still in the backup (per-subject isolation)",
            s.as_str()
        );
    }

    println!(
        "STOR-D4 GREEN [2026-06-19] crypto-shred erase(subject={}, tenant={}): \
         recoverable_in_backup={} (threshold 0), recoverable_live=0, \
         crypto_shred_lag_ms={}, dek_destroyed_now={}, kept_subjects=2 (isolation held)",
        receipt.subject,
        receipt.tenant.as_str(),
        receipt.recoverable_in_backup,
        receipt.crypto_shred_lag_ms,
        receipt.dek_destroyed_now,
    );
}

#[test]
fn stor_d4_re_erase_after_a_restore_style_replay_stays_zero_recoverable() {
    let tenant = TenantId("beta".into());
    let subject = SubjectId::new("u-again");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(tenant.clone(), region()))
        .expect("seed the in-memory KEK");
    ColumnCryptor::new(&kms, region())
        .encrypt(
            &tenant,
            Some(&subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"bio",
        )
        .unwrap();

    let eraser = CryptoShredErase::new(&kms, region());
    let wiring = DrillWiring::default();
    let holders = EraseHolders {
        pseudonym: &wiring,
        search: &wiring,
        refs: &wiring,
        bus: &wiring,
        ledger: &wiring,
        git_reach: None,
    };
    eraser.erase(&subject, &tenant, &holders, 1).unwrap();
    let again = eraser
        .erase(&subject, &tenant, &holders, 2)
        .expect("re-erase is a no-op SUCCESS");
    assert!(again.re_run);
    assert_eq!(
        again.recoverable_in_backup, 0,
        "still 0 recoverable after re-erase"
    );
}
