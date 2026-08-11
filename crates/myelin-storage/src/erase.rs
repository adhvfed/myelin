use std::fmt;

use myelin_tenancy::{Region, TenantId};

use crate::encryption::SubjectId;
use crate::kms::{DekId, KeyClass, KmsEngine};

pub trait PseudonymShred {
    fn shred_pseudonym(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

pub trait SearchPurge {
    fn purge_and_reindex(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

pub trait RefsTombstone {
    fn tombstone(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

pub trait BusErase {
    fn erase_inline_pii(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

pub trait BlobShredReach {
    fn shred_blob_tier(&self, subject: &SubjectId, tenant: &TenantId) -> Result<(), EraseError>;
}

pub trait ErasureLedgerSink {
    fn record_erasure(&self, subject: &SubjectId, tenant: &TenantId, at: EpochMillis);
    fn is_erased(&self, subject: &SubjectId, tenant: &TenantId) -> bool;
}

pub type EpochMillis = u64;

pub struct EraseHolders<'a> {
    pub pseudonym: &'a dyn PseudonymShred,
    pub search: &'a dyn SearchPurge,
    pub refs: &'a dyn RefsTombstone,
    pub bus: &'a dyn BusErase,
    pub ledger: &'a dyn ErasureLedgerSink,
    pub git_reach: Option<&'a dyn BlobShredReach>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EraseError {
    PseudonymShred(String),
    SearchPurge(String),
    RefsTombstone(String),
    BusErase(String),
    BlobShredReach(String),
}

impl fmt::Display for EraseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EraseError::PseudonymShred(m) => write!(
                f,
                "erase step 1 (pseudonym-map shred / Id.erase) failed: {m} - erase ABORTED as \
                 INCOMPLETE, NEVER recorded as erased (a partial erase is a loud retry, not 'assume erased')"
            ),
            EraseError::SearchPurge(m) => write!(
                f,
                "erase step 3 (Search purge+reindex) failed: {m} - erase ABORTED as INCOMPLETE \
                 (a stale plaintext-derived index entry could remain)"
            ),
            EraseError::RefsTombstone(m) => write!(
                f,
                "erase step 4 (Refs tombstone) failed: {m} - erase ABORTED as INCOMPLETE (an \
                 unfurl could still leak the subject)"
            ),
            EraseError::BusErase(m) => write!(
                f,
                "erase step 5 (Bus erase) failed: {m} - erase ABORTED as INCOMPLETE (an inline-PII \
                 event key could still be live)"
            ),
            EraseError::BlobShredReach(m) => write!(
                f,
                "erase step 2 (git crypto-shred reach, P-ST-24) failed: {m} - erase ABORTED as \
                 INCOMPLETE (a reflog/bitmap/pack-tier-backup git structure could still be \
                 recoverable from a backup)"
            ),
        }
    }
}

impl std::error::Error for EraseError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErasureReceipt {
    pub subject: String,
    pub tenant: TenantId,
    pub dek_destroyed_now: bool,
    pub recoverable_in_backup: usize,
    pub crypto_shred_lag_ms: EpochMillis,
    pub re_run: bool,
    pub completed_at: EpochMillis,
}

impl ErasureReceipt {
    pub fn is_green(&self) -> bool {
        self.recoverable_in_backup == 0
    }
}

pub struct CryptoShredErase<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> CryptoShredErase<'a> {
    pub fn new(engine: &'a KmsEngine, region: Region) -> CryptoShredErase<'a> {
        CryptoShredErase { engine, region }
    }

    pub fn erase(
        &self,
        subject: &SubjectId,
        tenant: &TenantId,
        holders: &EraseHolders<'_>,
        now: EpochMillis,
    ) -> Result<ErasureReceipt, EraseError> {
        let started = now;
        let re_run = holders.ledger.is_erased(subject, tenant);

        holders.pseudonym.shred_pseudonym(subject, tenant)?;

        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        let dek_destroyed_now = self.engine.destroy_dek(&subject_dek);

        if let Some(git_reach) = holders.git_reach {
            git_reach.shred_blob_tier(subject, tenant)?;
        }

        holders.search.purge_and_reindex(subject, tenant)?;

        holders.refs.tombstone(subject, tenant)?;

        holders.bus.erase_inline_pii(subject, tenant)?;

        let recoverable_in_backup = self
            .engine
            .backup_snapshot()
            .iter()
            .filter(|(d, _)| *d == subject_dek)
            .count();

        let completed_at = now;
        let crypto_shred_lag_ms = completed_at.saturating_sub(started);

        holders.ledger.record_erasure(subject, tenant, completed_at);

        Ok(ErasureReceipt {
            subject: subject.0.clone(),
            tenant: tenant.clone(),
            dek_destroyed_now,
            recoverable_in_backup,
            crypto_shred_lag_ms,
            re_run,
            completed_at,
        })
    }

    pub fn region(&self) -> &Region {
        &self.region
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::ColumnCryptor;
    use crate::kms::KekId;
    use myelin_gdpr::ErasureMethod;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r() -> Region {
        Region("eu-west".to_string())
    }

    #[derive(Default)]
    struct CallLog(RefCell<Vec<&'static str>>);
    impl CallLog {
        fn push(&self, step: &'static str) {
            self.0.borrow_mut().push(step);
        }
        fn steps(&self) -> Vec<&'static str> {
            self.0.borrow().clone()
        }
    }

    struct RecPseudonym<'a> {
        log: &'a CallLog,
        fail: bool,
    }
    impl PseudonymShred for RecPseudonym<'_> {
        fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            if self.fail {
                return Err(EraseError::PseudonymShred("id down".into()));
            }
            self.log.push("1:pseudonym");
            Ok(())
        }
    }

    struct RecSearch<'a> {
        log: &'a CallLog,
        fail: bool,
    }
    impl SearchPurge for RecSearch<'_> {
        fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            if self.fail {
                return Err(EraseError::SearchPurge("search down".into()));
            }
            self.log.push("3:search");
            Ok(())
        }
    }

    struct RecRefs<'a> {
        log: &'a CallLog,
    }
    impl RefsTombstone for RecRefs<'_> {
        fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.log.push("4:refs");
            Ok(())
        }
    }

    struct RecBus<'a> {
        log: &'a CallLog,
    }
    impl BusErase for RecBus<'_> {
        fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
            self.log.push("5:bus");
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecLedger {
        log: RefCell<Vec<&'static str>>,
        erased: RefCell<BTreeSet<String>>,
    }
    impl ErasureLedgerSink for RecLedger {
        fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
            self.log.borrow_mut().push("6:ledger");
            self.erased.borrow_mut().insert(subject.0.clone());
        }
        fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
            self.erased.borrow().contains(&subject.0)
        }
    }

    fn engine_with_subject_column(
        tenant: &TenantId,
        subject: &SubjectId,
        plaintext: &[u8],
    ) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()))
            .expect("seed the in-memory KEK");
        let cryptor = ColumnCryptor::new(&kms, r());
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

    #[test]
    fn erase_runs_the_six_steps_in_order() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-erase");
        let kms = engine_with_subject_column(&tenant, &subject, b"alice bio");
        let eraser = CryptoShredErase::new(&kms, r());

        let log = CallLog::default();
        let ledger = RecLedger::default();
        let ps = RecPseudonym {
            log: &log,
            fail: false,
        };
        let se = RecSearch {
            log: &log,
            fail: false,
        };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps,
            search: &se,
            refs: &rf,
            bus: &bu,
            ledger: &ledger,
            git_reach: None,
        };
        let receipt = eraser
            .erase(&subject, &tenant, &holders, 1_000)
            .expect("erase succeeds");

        assert_eq!(
            log.steps(),
            vec!["1:pseudonym", "3:search", "4:refs", "5:bus"]
        );
        assert_eq!(ledger.log.borrow().as_slice(), ["6:ledger"]);
        assert!(
            ledger.is_erased(&subject, &tenant),
            "the ledger records the subject as erased"
        );

        assert!(
            receipt.dek_destroyed_now,
            "the per-subject DEK was destroyed"
        );
        assert_eq!(receipt.recoverable_in_backup, 0);
        assert!(
            receipt.is_green(),
            "STOR-D4 green: 0 recoverable PII in backup"
        );
        assert!(!receipt.re_run, "first erase is not a re-run");
        assert_eq!(receipt.completed_at, 1_000);
    }

    #[test]
    fn step2_crypto_shred_renders_the_subject_column_unrecoverable_live_and_in_backup() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-1");
        let kms = engine_with_subject_column(&tenant, &subject, b"to be forgotten");
        let cryptor = ColumnCryptor::new(&kms, r());

        let col = cryptor
            .encrypt(
                &tenant,
                Some(&subject),
                &ErasureMethod::CryptoShred("subject_dek".into()),
                b"to be forgotten",
            )
            .unwrap();
        assert!(cryptor.decrypt(&col).is_ok(), "decrypts before the erase");

        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the subject DEK is in the backup before erase"
        );

        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();
        let ps = RecPseudonym {
            log: &log,
            fail: false,
        };
        let se = RecSearch {
            log: &log,
            fail: false,
        };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps,
            search: &se,
            refs: &rf,
            bus: &bu,
            ledger: &ledger,
            git_reach: None,
        };
        eraser.erase(&subject, &tenant, &holders, 5).unwrap();

        assert!(
            cryptor.decrypt(&col).is_err(),
            "column unrecoverable live after crypto-shred"
        );
        assert!(
            !kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "the subject DEK is absent from the backup after erase (0 recoverable, §7.5)"
        );
    }

    #[test]
    fn re_erasing_an_already_erased_subject_is_a_noop_success() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-twice");
        let kms = engine_with_subject_column(&tenant, &subject, b"bio");
        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();
        let ps = RecPseudonym {
            log: &log,
            fail: false,
        };
        let se = RecSearch {
            log: &log,
            fail: false,
        };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps,
            search: &se,
            refs: &rf,
            bus: &bu,
            ledger: &ledger,
            git_reach: None,
        };

        let r1 = eraser
            .erase(&subject, &tenant, &holders, 1)
            .expect("first erase");
        assert!(r1.dek_destroyed_now, "first erase destroys the DEK");
        assert!(!r1.re_run);

        let r2 = eraser
            .erase(&subject, &tenant, &holders, 2)
            .expect("re-erase is a no-op SUCCESS, never an error");
        assert!(
            !r2.dek_destroyed_now,
            "the DEK was already destroyed (idempotent re-run)"
        );
        assert!(r2.re_run, "the second erase is flagged as a re-run");
        assert_eq!(r2.recoverable_in_backup, 0, "still 0 recoverable in backup");
        assert!(r2.is_green());
    }

    #[test]
    fn step1_failure_aborts_loudly_and_never_records_the_erasure() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-fail1");
        let kms = engine_with_subject_column(&tenant, &subject, b"bio");
        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();

        let ps = RecPseudonym {
            log: &log,
            fail: true,
        };
        let se = RecSearch {
            log: &log,
            fail: false,
        };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps,
            search: &se,
            refs: &rf,
            bus: &bu,
            ledger: &ledger,
            git_reach: None,
        };
        let err = eraser
            .erase(&subject, &tenant, &holders, 1)
            .expect_err("a step-1 failure is a loud error");
        assert!(matches!(err, EraseError::PseudonymShred(_)));
        assert!(
            !ledger.is_erased(&subject, &tenant),
            "an incomplete erase is NOT recorded"
        );
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == subject_dek),
            "step 2 never ran - the DEK is intact"
        );
    }

    #[test]
    fn step3_search_failure_aborts_after_the_shred_but_does_not_record() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-fail3");
        let kms = engine_with_subject_column(&tenant, &subject, b"bio");
        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();

        let ps = RecPseudonym {
            log: &log,
            fail: false,
        };
        let se = RecSearch {
            log: &log,
            fail: true,
        };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps,
            search: &se,
            refs: &rf,
            bus: &bu,
            ledger: &ledger,
            git_reach: None,
        };
        let err = eraser
            .erase(&subject, &tenant, &holders, 1)
            .expect_err("a step-3 failure is a loud error");
        assert!(matches!(err, EraseError::SearchPurge(_)));
        assert!(
            !ledger.is_erased(&subject, &tenant),
            "not recorded until every step succeeds"
        );
        assert_eq!(log.steps(), vec!["1:pseudonym"]);
    }

    #[test]
    fn receipt_carries_the_crypto_shred_lag_and_green_reading() {
        let tenant = t("acme");
        let subject = SubjectId::new("u-lag");
        let kms = engine_with_subject_column(&tenant, &subject, b"bio");
        let eraser = CryptoShredErase::new(&kms, r());
        let log = CallLog::default();
        let ledger = RecLedger::default();

        let ps = RecPseudonym {
            log: &log,
            fail: false,
        };
        let se = RecSearch {
            log: &log,
            fail: false,
        };
        let rf = RecRefs { log: &log };
        let bu = RecBus { log: &log };
        let holders = EraseHolders {
            pseudonym: &ps,
            search: &se,
            refs: &rf,
            bus: &bu,
            ledger: &ledger,
            git_reach: None,
        };
        let receipt = eraser.erase(&subject, &tenant, &holders, 140).unwrap();
        assert_eq!(receipt.subject, "u-lag");
        assert_eq!(receipt.tenant, tenant);
        assert_eq!(receipt.completed_at, 140);
        assert_eq!(receipt.crypto_shred_lag_ms, 0);
        assert!(receipt.is_green());
    }

    #[test]
    fn erase_error_display_names_the_loud_incomplete_failure() {
        let e = EraseError::PseudonymShred("x".into());
        assert!(e.to_string().contains("step 1") && e.to_string().contains("INCOMPLETE"));
        let e = EraseError::SearchPurge("x".into());
        assert!(e.to_string().contains("step 3") && e.to_string().contains("INCOMPLETE"));
        let e = EraseError::RefsTombstone("x".into());
        assert!(e.to_string().contains("step 4"));
        let e = EraseError::BusErase("x".into());
        assert!(e.to_string().contains("step 5"));
    }

    #[test]
    fn receipt_is_green_only_when_zero_recoverable() {
        let red = ErasureReceipt {
            subject: "u".into(),
            tenant: t("acme"),
            dek_destroyed_now: true,
            recoverable_in_backup: 1,
            crypto_shred_lag_ms: 0,
            re_run: false,
            completed_at: 0,
        };
        assert!(!red.is_green(), "non-zero recoverable is RED");
        let green = ErasureReceipt {
            recoverable_in_backup: 0,
            ..red
        };
        assert!(green.is_green(), "0 recoverable is GREEN");
    }

    #[test]
    fn region_accessor_returns_the_kek_region() {
        let kms = KmsEngine::new();
        let eraser = CryptoShredErase::new(&kms, r());
        assert_eq!(eraser.region(), &r());
    }
}
