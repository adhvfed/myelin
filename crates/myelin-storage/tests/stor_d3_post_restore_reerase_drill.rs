use std::cell::RefCell;
use std::collections::BTreeSet;

use myelin_gdpr::ErasureMethod;
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_storage::{
    restore_to_offset, BlobPresence, BusErase, ColumnCryptor, ContinuousArchiver, DekId,
    EpochMillis, EraseError, EraseHolders, ErasureLedgerSink, ErasureRecord, InMemoryPostPitLedger,
    KekId, KeyClass, KmsEngine, PseudonymShred, ReErasePass, RefsTombstone, SearchPurge, SourceLog,
    SubjectId, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("eu-west".into())
}
fn tenant(s: &str) -> TenantId {
    TenantId(s.into())
}

#[derive(Default)]
struct Seams {
    erased: RefCell<BTreeSet<String>>,
}
impl PseudonymShred for Seams {
    fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl SearchPurge for Seams {
    fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl RefsTombstone for Seams {
    fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl BusErase for Seams {
    fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        Ok(())
    }
}
impl ErasureLedgerSink for Seams {
    fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
        self.erased.borrow_mut().insert(subject.0.clone());
    }
    fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
        self.erased.borrow().contains(&subject.0)
    }
}

fn reachable_archiver(tail: u64) -> ContinuousArchiver {
    let mut arch = ContinuousArchiver::new();
    arch.archive_segment(WalSegment {
        end_offset: 0,
        committed_at: 0,
    })
    .unwrap();
    arch.take_base_backup(1);
    arch.archive_segment(WalSegment {
        end_offset: tail,
        committed_at: 10,
    })
    .unwrap();
    arch
}

fn restored_copy_with_resurrected_subject(t: &TenantId, subject: &SubjectId) -> KmsEngine {
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
    ColumnCryptor::new(&kms, region())
        .encrypt(
            t,
            Some(subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"data to forget",
        )
        .expect("seal a per-subject column (the DEK is live in the restored copy)");
    kms
}

#[test]
fn stor_d3_post_restore_reerase_zero_resurrected() {
    let t = tenant("acme");
    let subject = SubjectId::new("u-forget");

    let kms = restored_copy_with_resurrected_subject(&t, &subject);
    let subject_dek = DekId::new(t.clone(), KeyClass::Subject(subject.0.clone()));
    assert!(
        kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "precondition: the restore of the older backup RESURRECTED the subject's DEK"
    );

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
    ledger.record(ErasureRecord::new(subject.clone(), t.clone(), 200));

    let seams = Seams::default();
    let holders = EraseHolders {
        pseudonym: &seams,
        search: &seams,
        refs: &seams,
        bus: &seams,
        ledger: &seams,
        git_reach: None,
    };
    let pass = ReErasePass::new(&kms, region());
    let rep = pass
        .run(&report, &ledger, &holders, 1_000)
        .expect("re-erasure pass runs");

    let mut signals = SignalSource::new();
    signals.set_scalar(
        SignalName::RestoreCrossSeamMismatch,
        rep.resurrected_count as i64,
    );
    signals
        .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
        .expect_green();

    assert_eq!(
        rep.resurrected_count, 0,
        "STOR-D3: 0 resurrected subjects (§7.5)"
    );
    assert!(rep.is_green());
    assert!(
        rep.re_erased_subject(&subject, &t),
        "the subject was re-erased (the receipt)"
    );
    assert!(
        rep.re_erased[0].was_resurrected_before_reapply,
        "the subject WAS resurrected by the restore and re-killed by the pass"
    );
    assert!(
        !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the resurrected DEK is re-destroyed - still erased after restore"
    );

    println!(
        "[P-100 DRILL GREEN 2026-06-20] STOR-D3 (post-restore re-erasure): erased a subject at \
         offset 200, restored the OLDER backup at PIT T=100 (which RESURRECTED the per-subject DEK), \
         ran the mandatory §7.5 re-erasure pass -> resurrected_count=0 (re-erasure receipt: subject \
         re-erased, DEK re-destroyed, `*.erased` re-emitted). Idempotent (a 2nd pass is a no-op \
         success). RTO/cell-kill leg -> stor_d2_cell_kill_rto_drill; cell-scale re-confirm -> \
         P-ST-30 (M5); §7.6 backup-window residual number -> [OPEN -> LEGAL]."
    );
}

#[test]
fn stor_d3_without_reerase_the_restore_resurrects() {
    let t = tenant("acme");
    let subject = SubjectId::new("u-forget");
    let kms = restored_copy_with_resurrected_subject(&t, &subject);
    let subject_dek = DekId::new(t.clone(), KeyClass::Subject(subject.0.clone()));

    let resurrected = kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek);
    assert!(
        resurrected,
        "WITHOUT the §7.5 re-erasure pass, the restore RESURRECTS the post-T-erased subject's DEK"
    );

    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::RestoreCrossSeamMismatch, 1);
    let verdict = signals.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "a resurrected subject (no re-erasure) MUST read RED on the STOR-D3 assertion"
    );
}
