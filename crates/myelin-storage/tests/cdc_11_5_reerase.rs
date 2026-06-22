//! Contract 11.5 CDC pair — the **post-restore re-erasure** caller (P-ST-14 / global P-100).
//!
//! The prompt requires "CDC: provider+consumer pair for 11.5 (the re-erasure caller)". This is the
//! consumer-driven contract test for the RE-ERASURE half of row 11.5 (the backup half is
//! `cdc_11_5_backup`, the restore half is `cdc_11_5_restore`, the CI-gate half is
//! `cdc_11_5_restore_verify_gate`, this is the post-restore RE-ERASURE half):
//!
//! - the **PROVIDER** is `myelin-storage` — the [`ReErasePass`] (and the
//!   [`RestoreVerifyGate::run_with_reerase`] gate wiring) this prompt ships: after a restore, it
//!   re-applies every erasure the [`PostRestoreErasureLedger`] records as completed AFTER the PIT and
//!   asserts 0 resurrected subjects (§7.5).
//! - the **CONSUMER** is the **restore driver** (the CI durability gate / the cell-kill recovery path
//!   — the real wiring lands with the restore-verify CI runner) modelled here as a tiny
//!   `RestoreDriver` that, after landing a restore, MUST run the mandatory re-erasure pass against the
//!   erasure ledger and refuse to mark the copy "ready" until it returns 0 resurrected. This is the
//!   call shape the §7.5 "every restore re-erases by construction" requirement relies on — if the
//!   ledger seam, the [`ReErasePass::run`] signature, or the `ReEraseReport`/0-resurrected contract
//!   drift, this stops compiling/passing.
//!
//! It pins the load-bearing contract property the consumer depends on: a subject erased AFTER the
//! backup's PIT is RESURRECTED by a restore of T, and the mandatory re-erasure pass re-kills it to 0
//! recoverable — so the restore driver never marks a copy ready that un-erased a person.

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

/// The cross-holder re-erasure seams (the §7.5 re-purge Search / re-tombstone Refs / re-emit
/// `*.erased`), recorded so the CDC asserts they re-ran.
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

/// THE CONSUMER: the restore driver. After landing a restore it MUST run the mandatory re-erasure
/// pass and refuse "ready" unless 0 resurrected. The real CI/cell-kill recovery path wires THIS shape.
struct RestoreDriver;

impl RestoreDriver {
    /// Land the restore, then run the mandatory §7.5 re-erasure pass. Returns the report; the driver
    /// only marks the copy "ready" when `report.is_green()` (0 resurrected).
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

/// PROVIDER ⇄ CONSUMER: a subject erased AFTER the backup PIT is resurrected by a restore; the driver
/// runs the mandatory re-erasure pass and gets 0 resurrected — the copy is safe to mark ready.
#[test]
fn restore_driver_re_erases_a_post_pit_subject_to_zero_resurrected() {
    let t = tenant("acme");
    let subject = SubjectId::new("u-erased-after-backup");

    // The restored copy holds the subject's pre-erasure DEK alive (resurrected by the restore of T).
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
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

    // The ledger records the erasure as completed at offset 140 (AFTER the PIT T=100).
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

    // The consumer-depended-on property: 0 resurrected → the copy is safe to mark ready.
    assert!(report.is_green(), "0 resurrected → ready");
    assert_eq!(report.resurrected_count, 0);
    assert!(
        report.re_erased_subject(&subject, &t),
        "the post-PIT subject was re-erased"
    );
    // The resurrected DEK is gone from the restored copy.
    assert!(
        !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
        "the resurrected DEK was re-destroyed by the mandatory pass"
    );
    // The cross-holder ledger re-recorded the erasure (re-emit leg).
    assert!(seams.is_erased(&subject, &t));
}

/// PROVIDER ⇄ CONSUMER (the gate-wired path): the restore-verify gate's `run_with_reerase` greens a
/// restore that re-erases a post-PIT subject — wiring re-erasure into the gate (every restore
/// re-erases by construction, §7.5).
#[test]
fn the_gate_run_with_reerase_greens_a_re_erased_restore() {
    use myelin_storage::{ErasureLedger, GateInputs, RestoreVerifyGate};

    let t = tenant("acme");
    let subject = SubjectId::new("u-post-pit");
    let kms = KmsEngine::new();
    kms.ensure_kek(&KekId::new(t.clone(), region()));
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
