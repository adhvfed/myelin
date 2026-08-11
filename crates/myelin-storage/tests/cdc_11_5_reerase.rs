use myelin_gdpr::ErasureMethod;
use myelin_storage::{
    restore_to_offset, BlobPresence, BusErase, DekId, EpochMillis, ErasureLedgerSink, KeyClass,
    PseudonymShred, RefsTombstone, SearchPurge, SourceLog,
};
use myelin_storage::{
    ColumnCryptor, ContinuousArchiver, EraseError, EraseHolders, ErasureRecord,
    InMemoryPostPitLedger, KekId, KmsEngine, PostRestoreErasureLedger, ReErasePass, ReEraseReport,
    SubjectId, WalSegment,
};
use myelin_tenancy::{Region, TenantId};

use std::cell::RefCell;
use std::collections::BTreeSet;

fn region() -> Region {
    Region("eu-west".into())
}
fn tenant(s: &str) -> TenantId {
    TenantId(s.into())
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

#[derive(Default)]
struct Seams {
    erased_ledger: RefCell<BTreeSet<String>>,
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
        self.erased_ledger.borrow_mut().insert(subject.0.clone());
    }
    fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
        self.erased_ledger.borrow().contains(&subject.0)
    }
}

struct RestoreDriver;

impl RestoreDriver {
    fn restore_then_reerase(
        &self,
        kms: &KmsEngine,
        archiver: &ContinuousArchiver,
        target: u64,
        ledger: &dyn PostRestoreErasureLedger,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<ReEraseReport, EraseError> {
        let report = restore_to_offset(
            archiver,
            target,
            &[],
            &BlobPresence::new(),
            &SourceLog::new(),
            kms,
        )
        .expect("the restore lands");
        ReErasePass::new(kms, region()).run(&report, ledger, holders, now)
    }
}

#[test]
fn restore_driver_re_erases_a_post_pit_subject_to_zero_resurrected() {
    let t = tenant("acme");
    let subject = SubjectId::new("u-erased-after-backup");

    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
    ColumnCryptor::new(&kms, region())
        .encrypt(
            &t,
            Some(&subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"resurrected bio",
        )
        .unwrap();
    let subject_dek = DekId::new(t.clone(), KeyClass::Subject(subject.0.clone()));
    assert!(
        kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the restore resurrected the subject's DEK"
    );

    let mut ledger = InMemoryPostPitLedger::new();
    ledger.record(ErasureRecord::new(subject.clone(), t.clone(), 140));

    let seams = Seams::default();
    let holders = EraseHolders {
        pseudonym: &seams,
        search: &seams,
        refs: &seams,
        bus: &seams,
        ledger: &seams,
        git_reach: None,
    };

    let arch = reachable_archiver(300);
    let report = RestoreDriver
        .restore_then_reerase(&kms, &arch, 100, &ledger, &holders, 1_000)
        .expect("the mandatory re-erasure pass succeeds");

    assert!(report.is_green(), "0 resurrected → ready");
    assert_eq!(report.resurrected_count, 0);
    assert!(
        report.re_erased_subject(&subject, &t),
        "the post-PIT subject was re-erased"
    );
    assert!(
        !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the resurrected DEK was re-destroyed by the mandatory pass"
    );
    assert!(seams.is_erased(&subject, &t));
}

#[test]
fn the_gate_run_with_reerase_greens_a_re_erased_restore() {
    use myelin_storage::{ErasureLedger, GateInputs, RestoreVerifyGate};

    let t = tenant("acme");
    let subject = SubjectId::new("u-post-pit");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");
    ColumnCryptor::new(&kms, region())
        .encrypt(
            &t,
            Some(&subject),
            &ErasureMethod::CryptoShred("subject_dek".into()),
            b"bio",
        )
        .unwrap();

    let mut post_pit = InMemoryPostPitLedger::new();
    post_pit.record(ErasureRecord::new(subject.clone(), t.clone(), 140));

    let seams = Seams::default();
    let holders = EraseHolders {
        pseudonym: &seams,
        search: &seams,
        refs: &seams,
        bus: &seams,
        ledger: &seams,
        git_reach: None,
    };
    let before_backup = ErasureLedger::new();
    let arch = reachable_archiver(300);
    let inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &[],
        objects: &[],
        source: &SourceLog::new(),
        kms: &kms,
        erasure_ledger: &before_backup,
    };

    let verdict =
        RestoreVerifyGate::new().run_with_reerase(&inputs, &post_pit, &holders, region(), 1_000);
    assert!(
        verdict.is_green(),
        "the gate with re-erasure greens (0 resurrected), got {:?}",
        verdict.failure()
    );
    assert_eq!(
        verdict.green_artifact().unwrap().resurrected_subjects,
        0,
        "the gate artifact records 0 resurrected after the re-erasure pass"
    );
}

#[test]
fn a_window_erasure_with_no_post_pit_coverage_is_refused_not_trusted_green() {
    use myelin_storage::{ErasureLedger, GateInputs, RestoreVerifyGate};

    let t = tenant("acme");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()))
        .expect("seed the in-memory KEK");

    let ledger = ErasureLedger::new();
    ledger.record_erased_at(t.clone(), 140);
    let post_pit = InMemoryPostPitLedger::new();

    let seams = Seams::default();
    let holders = EraseHolders {
        pseudonym: &seams,
        search: &seams,
        refs: &seams,
        bus: &seams,
        ledger: &seams,
        git_reach: None,
    };
    let arch = reachable_archiver(300);
    let inputs = GateInputs {
        archiver: &arch,
        target: 100,
        rows: &[],
        objects: &[],
        source: &SourceLog::new(),
        kms: &kms,
        erasure_ledger: &ledger,
    };

    let verdict =
        RestoreVerifyGate::new().run_with_reerase(&inputs, &post_pit, &holders, region(), 1_000);
    assert!(
        !verdict.is_green(),
        "a window erasure with no post-PIT coverage must be REFUSED (structural cross-ledger \
         assert), never a trusted green"
    );
}
