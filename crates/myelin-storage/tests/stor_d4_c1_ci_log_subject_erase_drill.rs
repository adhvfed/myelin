use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    BusErase, CiLogFrame, CiLogTier, ColumnCryptor, CryptoShredErase, DekId, EpochMillis,
    EraseError, EraseHolders, ErasureLedgerSink, KekId, KeyClass, KmsEngine, PseudonymShred,
    RefsTombstone, SearchPurge, SegmentKeying, SubjectId,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
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
fn stor_d4_c1_erasing_a_subject_crypto_shreds_their_ci_log_zero_recoverable_in_backups() {
    let erase_me = SubjectId::new("u-erase");
    let keep = SubjectId::new("u-keep");

    let kms = Arc::new(KmsEngine::new());
    kms.ensure_kek(&KekId::new(tenant(), region()))
        .expect("seed the in-memory KEK");

    let tier = CiLogTier::with_tenant_dek("run-42", tenant(), region(), kms.clone());

    tier.seal_ci_batch_for_subject(
        &erase_me,
        &[(
            1,
            CiLogFrame::new(
                "run-42",
                1,
                b"erase-me@corp.test FAILED at line 7\n".to_vec(),
            ),
        )],
    )
    .expect("seal erase_me's isolable CI log step");
    tier.seal_ci_batch_for_subject(
        &keep,
        &[(
            2,
            CiLogFrame::new("run-42", 2, b"keep@corp.test ran clean\n".to_vec()),
        )],
    )
    .expect("seal keep's isolable CI log step");
    tier.seal_ci_batch(&[(
        3,
        CiLogFrame::new("run-42", 3, b"interleaved many-author log\n".to_vec()),
    )])
    .expect("seal the non-isolable step (tenant fallback)");

    assert_eq!(
        tier.step_keying("run-42", 1).unwrap(),
        vec![SegmentKeying::Subject(erase_me.clone())]
    );
    assert_eq!(
        tier.step_keying("run-42", 2).unwrap(),
        vec![SegmentKeying::Subject(keep.clone())]
    );
    assert_eq!(
        tier.step_keying("run-42", 3).unwrap(),
        vec![SegmentKeying::Tenant]
    );

    let cryptor = ColumnCryptor::new(&kms, region());
    let col = cryptor
        .encrypt(
            &tenant(),
            Some(&erase_me),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"erase-me free-text bio",
        )
        .expect("seal erase_me's column");

    assert!(
        tier.resolve_step("run-42", 1).is_ok(),
        "erase_me CI log resolves before erase"
    );
    assert!(
        cryptor.decrypt(&col).is_ok(),
        "erase_me column decrypts before erase"
    );
    let dek_of = |s: &SubjectId| DekId::new(tenant(), KeyClass::Subject(s.0.clone()));
    assert!(
        kms.backup_snapshot()
            .unwrap()
            .iter()
            .any(|(k, _)| *k == dek_of(&erase_me)),
        "erase_me DEK is in the backup before erase"
    );

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
        .erase(&erase_me, &tenant(), &holders, 1_718_000_000_000)
        .expect("erase completes all six steps");

    let live = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tier.resolve_step("run-42", 1)
    }));
    assert!(
        live.is_err(),
        "STOR-D4+C1 RED: erase_me's CI log step is STILL recoverable LIVE after erase"
    );
    assert!(
        cryptor.decrypt(&col).is_err(),
        "STOR-D4+C1 RED: erase_me's column still recoverable (the per-subject DEK reaches both)"
    );

    let recoverable_in_backup = kms
        .backup_snapshot()
        .unwrap()
        .iter()
        .filter(|(k, _)| *k == dek_of(&erase_me))
        .count();
    assert_eq!(
        recoverable_in_backup, 0,
        "STOR-D4+C1 RED: a backup still carries erase_me's per-subject DEK (a restore could \
         resurrect their CI log) - threshold 0, NOT weakened"
    );
    assert_eq!(receipt.recoverable_in_backup, 0);
    assert!(
        receipt.is_green(),
        "STOR-D4+C1 green: 0 recoverable PII in backup"
    );

    assert_eq!(
        tier.resolve_step("run-42", 2).unwrap(),
        b"keep@corp.test ran clean\n",
        "keep's isolable CI log survives erase_me's erasure (per-subject isolation)"
    );
    assert_eq!(
        tier.resolve_step("run-42", 3).unwrap(),
        b"interleaved many-author log\n",
        "the per-tenant-fallback CI log survives (the documented 10.9 residual, not erased by an \
         individual's Art. 17)"
    );
    assert!(
        kms.backup_snapshot()
            .unwrap()
            .iter()
            .any(|(k, _)| *k == dek_of(&keep)),
        "keep's DEK is still in the backup (isolation held)"
    );

    println!(
        "STOR-D4+C1 GREEN [2026-06-22] per-subject CI-log DEK crypto-shred erase(subject={}, \
         tenant={}): erase_me CI log step recoverable_live=0 (loud refusal), \
         recoverable_in_backup={} (threshold 0); keep's isolable CI log + the per-tenant-fallback \
         CI log survived (isolation held); crypto_shred_lag_ms={}, dek_destroyed_now={}.",
        receipt.subject,
        receipt.tenant.as_str(),
        receipt.recoverable_in_backup,
        receipt.crypto_shred_lag_ms,
        receipt.dek_destroyed_now,
    );
}
