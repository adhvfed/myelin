use std::collections::BTreeMap;

use myelin_tenancy::TenantId;

use crate::backup::{EpochSecs, WalOffset};
use crate::encryption::SubjectId;
use crate::erase::{CryptoShredErase, EpochMillis, EraseError, EraseHolders, ErasureReceipt};
use crate::kms::{DekId, KeyClass, KmsEngine};
use crate::restore::RestoreReport;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureRecord {
    pub subject: SubjectId,
    pub tenant: TenantId,
    pub completed_at_offset: WalOffset,
}

impl ErasureRecord {
    pub fn new(
        subject: SubjectId,
        tenant: TenantId,
        completed_at_offset: WalOffset,
    ) -> ErasureRecord {
        ErasureRecord {
            subject,
            tenant,
            completed_at_offset,
        }
    }
}

pub trait PostRestoreErasureLedger {
    fn erasures_completed_after(&self, pit: WalOffset) -> Vec<ErasureRecord>;
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, Default)]
pub struct InMemoryPostPitLedger {
    records: Vec<ErasureRecord>,
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryPostPitLedger {
    pub fn new() -> InMemoryPostPitLedger {
        InMemoryPostPitLedger::default()
    }

    pub fn record(&mut self, record: ErasureRecord) -> &mut Self {
        self.records.push(record);
        self
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl PostRestoreErasureLedger for InMemoryPostPitLedger {
    fn erasures_completed_after(&self, pit: WalOffset) -> Vec<ErasureRecord> {
        self.records
            .iter()
            .filter(|r| r.completed_at_offset > pit)
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReErasedSubject {
    pub subject: SubjectId,
    pub tenant: TenantId,
    pub was_resurrected_before_reapply: bool,
    pub receipt: ErasureReceipt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReEraseReport {
    pub restored_to_offset: WalOffset,
    pub re_erased: Vec<ReErasedSubject>,
    pub resurrected_count: u64,
}

impl ReEraseReport {
    pub fn is_green(&self) -> bool {
        self.resurrected_count == 0
    }

    pub fn re_erased_count(&self) -> usize {
        self.re_erased.len()
    }

    pub fn re_erased_subject(&self, subject: &SubjectId, tenant: &TenantId) -> bool {
        self.re_erased
            .iter()
            .any(|s| &s.subject == subject && &s.tenant == tenant)
    }
}

pub struct ReErasePass<'a> {
    eraser: CryptoShredErase<'a>,
    engine: &'a KmsEngine,
}

impl<'a> ReErasePass<'a> {
    pub fn new(engine: &'a KmsEngine, region: myelin_tenancy::Region) -> ReErasePass<'a> {
        ReErasePass {
            eraser: CryptoShredErase::new(engine, region),
            engine,
        }
    }

    pub fn run(
        &self,
        report: &RestoreReport,
        ledger: &dyn PostRestoreErasureLedger,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<ReEraseReport, EraseError> {
        let pit = report.restored_to_offset;
        let post_pit = ledger.erasures_completed_after(pit);

        let mut re_erased = Vec::with_capacity(post_pit.len());
        for record in &post_pit {
            let subject_dek = DekId::new(
                record.tenant.clone(),
                KeyClass::Subject(record.subject.0.clone()),
            );
            let was_resurrected = self.dek_present(&subject_dek)?;

            let receipt = self
                .eraser
                .erase(&record.subject, &record.tenant, holders, now)?;

            re_erased.push(ReErasedSubject {
                subject: record.subject.clone(),
                tenant: record.tenant.clone(),
                was_resurrected_before_reapply: was_resurrected,
                receipt,
            });
        }

        let mut resurrected_count = 0;
        for record in &post_pit {
            let dek = DekId::new(
                record.tenant.clone(),
                KeyClass::Subject(record.subject.0.clone()),
            );
            if self.dek_present(&dek)? {
                resurrected_count += 1;
            }
        }

        Ok(ReEraseReport {
            restored_to_offset: pit,
            re_erased,
            resurrected_count,
        })
    }

    fn dek_present(&self, dek: &DekId) -> Result<bool, EraseError> {
        Ok(self
            .engine
            .backup_snapshot()
            .map_err(EraseError::Kms)?
            .iter()
            .any(|(d, _)| d == dek))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtoGrain {
    Tenant,
    Cell,
}

impl RtoGrain {
    pub fn label(self) -> &'static str {
        match self {
            RtoGrain::Tenant => "tenant",
            RtoGrain::Cell => "cell",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellKillRestore {
    pub grain: RtoGrain,
    pub began_at: EpochSecs,
    pub ready_at: EpochSecs,
}

impl CellKillRestore {
    pub fn new(grain: RtoGrain, began_at: EpochSecs, ready_at: EpochSecs) -> CellKillRestore {
        CellKillRestore {
            grain,
            began_at,
            ready_at: ready_at.max(began_at),
        }
    }

    pub fn rto_secs(&self) -> EpochSecs {
        self.ready_at.saturating_sub(self.began_at)
    }

    pub fn within_bound(&self, bound_secs: EpochSecs) -> bool {
        self.rto_secs() <= bound_secs
    }
}

#[derive(Clone, Debug, Default)]
pub struct CellKillRtoReport {
    rto_secs: BTreeMap<&'static str, EpochSecs>,
}

impl CellKillRtoReport {
    pub fn new() -> CellKillRtoReport {
        CellKillRtoReport::default()
    }

    pub fn record(&mut self, recovery: &CellKillRestore) -> &mut Self {
        self.rto_secs
            .insert(recovery.grain.label(), recovery.rto_secs());
        self
    }

    pub fn rto_for(&self, grain: RtoGrain) -> Option<EpochSecs> {
        self.rto_secs.get(grain.label()).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{ContinuousArchiver, WalSegment};
    use crate::encryption::ColumnCryptor;
    use crate::erase::{BusErase, ErasureLedgerSink, PseudonymShred, RefsTombstone, SearchPurge};
    use crate::kms::KekId;
    use crate::restore::{restore_to_offset, BlobPresence, SourceLog};
    use myelin_gdpr::ErasureMethod;
    use myelin_tenancy::Region;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    fn t(s: &str) -> TenantId {
        TenantId(s.into())
    }
    fn r() -> Region {
        Region("eu-west".into())
    }

    #[derive(Default)]
    struct RecSeams {
        calls: RefCell<Vec<String>>,
        erased: RefCell<BTreeSet<String>>,
    }
    impl PseudonymShred for RecSeams {
        fn shred_pseudonym(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.calls.borrow_mut().push(format!("pseudonym:{}", s.0));
            Ok(())
        }
    }
    impl SearchPurge for RecSeams {
        fn purge_and_reindex(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.calls.borrow_mut().push(format!("search:{}", s.0));
            Ok(())
        }
    }
    impl RefsTombstone for RecSeams {
        fn tombstone(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.calls.borrow_mut().push(format!("refs:{}", s.0));
            Ok(())
        }
    }
    impl BusErase for RecSeams {
        fn erase_inline_pii(&self, s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.calls.borrow_mut().push(format!("erased:{}", s.0));
            Ok(())
        }
    }
    impl ErasureLedgerSink for RecSeams {
        fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.erased.borrow_mut().insert(subject.0.clone());
        }
        fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
            self.erased.borrow().contains(&subject.0)
        }
    }

    fn holders(seams: &RecSeams) -> EraseHolders<'_> {
        EraseHolders {
            pseudonym: seams,
            search: seams,
            refs: seams,
            bus: seams,
            ledger: seams,
            git_reach: None,
        }
    }

    fn reachable_archiver(tail: WalOffset) -> ContinuousArchiver {
        let mut a = ContinuousArchiver::new();
        a.archive_segment(WalSegment {
            end_offset: 0,
            committed_at: 0,
        })
        .unwrap();
        a.take_base_backup(1);
        a.archive_segment(WalSegment {
            end_offset: tail,
            committed_at: 10,
        })
        .unwrap();
        a
    }

    fn engine_with_subject(tenant: &TenantId, subject: &SubjectId) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()))
            .expect("seed the in-memory KEK");
        let cryptor = ColumnCryptor::new(&kms, r());
        cryptor
            .encrypt(
                tenant,
                Some(subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                b"to be re-erased",
            )
            .expect("seal a per-subject column");
        kms
    }

    #[test]
    fn ledger_selects_only_post_pit_erasures() {
        let mut ledger = InMemoryPostPitLedger::new();
        ledger
            .record(ErasureRecord::new(SubjectId::new("pre"), t("acme"), 50))
            .record(ErasureRecord::new(SubjectId::new("at"), t("acme"), 100))
            .record(ErasureRecord::new(SubjectId::new("post"), t("acme"), 140));

        let after = ledger.erasures_completed_after(100);
        let ids: Vec<&str> = after.iter().map(|r| r.subject.0.as_str()).collect();
        assert_eq!(ids, vec!["post"], "only the post-PIT erasure is selected");
    }

    #[test]
    fn reerase_re_kills_a_resurrected_post_pit_subject() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-erased-after-backup");
        let kms = engine_with_subject(&tenant, &subject);
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        assert!(
            kms.backup_snapshot()
                .unwrap()
                .iter()
                .any(|(d, _)| *d == subject_dek),
            "the restored copy RESURRECTED the subject's DEK (it was live at the backup PIT)"
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
        ledger.record(ErasureRecord::new(subject.clone(), tenant.clone(), 140));

        let seams = RecSeams::default();
        let pass = ReErasePass::new(&kms, r());
        let rep = pass
            .run(&report, &ledger, &holders(&seams), 1_000)
            .expect("the re-erasure pass succeeds");

        assert_eq!(rep.re_erased_count(), 1);
        assert!(rep.re_erased_subject(&subject, &tenant));
        assert!(
            rep.re_erased[0].was_resurrected_before_reapply,
            "the subject WAS resurrected by the restore (its DEK was live at T)"
        );
        assert_eq!(rep.resurrected_count, 0, "0 resurrected subjects (§7.5)");
        assert!(rep.is_green());
        assert!(
            !kms.backup_snapshot()
                .unwrap()
                .iter()
                .any(|(d, _)| *d == subject_dek),
            "the resurrected DEK is re-destroyed by the pass"
        );
        let calls = seams.calls.borrow();
        assert!(calls.contains(&"search:u-erased-after-backup".to_string()));
        assert!(calls.contains(&"refs:u-erased-after-backup".to_string()));
        assert!(
            calls.contains(&"erased:u-erased-after-backup".to_string()),
            "`*.erased` re-emitted"
        );
    }

    #[test]
    fn reerase_pass_is_idempotent() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-twice");
        let kms = engine_with_subject(&tenant, &subject);
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
        ledger.record(ErasureRecord::new(subject.clone(), tenant.clone(), 140));
        let seams = RecSeams::default();
        let pass = ReErasePass::new(&kms, r());

        let first = pass.run(&report, &ledger, &holders(&seams), 1).unwrap();
        assert!(first.is_green());
        assert!(
            first.re_erased[0].was_resurrected_before_reapply,
            "first pass re-killed it"
        );

        let second = pass.run(&report, &ledger, &holders(&seams), 2).unwrap();
        assert_eq!(
            second.resurrected_count, 0,
            "idempotent: still 0 resurrected"
        );
        assert!(
            !second.re_erased[0].was_resurrected_before_reapply,
            "the second pass found the key already gone (no resurrection to re-kill)"
        );
        assert!(second.is_green());
    }

    #[test]
    fn ledger_and_report_accessors_are_exact() {
        let mut ledger = InMemoryPostPitLedger::new();
        assert!(ledger.is_empty(), "a fresh ledger is empty");
        assert_eq!(ledger.len(), 0);
        ledger.record(ErasureRecord::new(SubjectId::new("a"), t("acme"), 10));
        ledger.record(ErasureRecord::new(SubjectId::new("b"), t("acme"), 20));
        assert!(!ledger.is_empty(), "a populated ledger is not empty");
        assert_eq!(ledger.len(), 2, "len counts every recorded erasure");

        let report = ReEraseReport {
            restored_to_offset: 100,
            re_erased: vec![ReErasedSubject {
                subject: SubjectId::new("present"),
                tenant: t("acme"),
                was_resurrected_before_reapply: true,
                receipt: crate::erase::ErasureReceipt {
                    subject: "present".into(),
                    tenant: t("acme"),
                    dek_destroyed_now: true,
                    recoverable_in_backup: 0,
                    crypto_shred_lag_ms: 0,
                    re_run: false,
                    completed_at: 0,
                },
            }],
            resurrected_count: 0,
        };
        assert!(report.re_erased_subject(&SubjectId::new("present"), &t("acme")));
        assert!(
            !report.re_erased_subject(&SubjectId::new("absent"), &t("acme")),
            "a non-re-erased subject is not reported (kills the `-> true` mutant)"
        );
        assert!(
            !report.re_erased_subject(&SubjectId::new("present"), &t("other-tenant")),
            "the tenant must ALSO match (kills the `&& -> ||` mutant)"
        );
    }

    #[test]
    fn a_pre_pit_erasure_is_not_re_applied() {
        let kms = KmsEngine::new();
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
        ledger.record(ErasureRecord::new(SubjectId::new("pre"), t("acme"), 60));
        let seams = RecSeams::default();
        let pass = ReErasePass::new(&kms, r());
        let rep = pass.run(&report, &ledger, &holders(&seams), 1).unwrap();
        assert_eq!(
            rep.re_erased_count(),
            0,
            "a pre-PIT erasure is not re-applied"
        );
        assert!(rep.is_green());
        assert!(seams.calls.borrow().is_empty(), "no re-erasure ran");
    }

    #[test]
    fn report_is_green_only_when_zero_resurrected() {
        let red = ReEraseReport {
            restored_to_offset: 100,
            re_erased: vec![],
            resurrected_count: 1,
        };
        assert!(!red.is_green(), "a resurrected subject is RED");
        let green = ReEraseReport {
            resurrected_count: 0,
            ..red
        };
        assert!(green.is_green(), "0 resurrected is GREEN");
    }

    #[test]
    fn cell_kill_rto_is_measured_per_grain() {
        let tenant_recovery = CellKillRestore::new(RtoGrain::Tenant, 0, 2_400);
        assert_eq!(tenant_recovery.rto_secs(), 2_400);
        assert!(
            tenant_recovery.within_bound(3_600),
            "40 min ≤ 1 h tenant bound"
        );
        assert!(
            !tenant_recovery.within_bound(1_800),
            "40 min exceeds a 30-min bound (the gate bites)"
        );

        let cell_recovery = CellKillRestore::new(RtoGrain::Cell, 0, 10_800);
        assert_eq!(cell_recovery.rto_secs(), 10_800);
        assert!(cell_recovery.within_bound(14_400), "3 h ≤ 4 h cell bound");

        let mut report = CellKillRtoReport::new();
        report.record(&tenant_recovery).record(&cell_recovery);
        assert_eq!(report.rto_for(RtoGrain::Tenant), Some(2_400));
        assert_eq!(report.rto_for(RtoGrain::Cell), Some(10_800));
    }

    #[test]
    fn rto_clock_is_monotone() {
        let recovery = CellKillRestore::new(RtoGrain::Tenant, 100, 50);
        assert_eq!(
            recovery.rto_secs(),
            0,
            "ready clamped to began → RTO 0, never underflow"
        );
    }

    #[test]
    fn rto_grain_labels_are_stable() {
        assert_eq!(RtoGrain::Tenant.label(), "tenant");
        assert_eq!(RtoGrain::Cell.label(), "cell");
    }
}
