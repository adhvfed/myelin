use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, SubjectRef, TenantId};
use myelin_git::code_tools::{CacheInvalidator, CacheNamespace};
use myelin_git::core::{GitCoreError, RepoLoc};
use myelin_git::holder::{GitHolder, GitPersonalDataHolder, GitResidualPosture};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::SubjectId;
use myelin_storage::erase::{
    BusErase, EpochMillis, EraseError, EraseHolders, ErasureLedgerSink, PseudonymShred,
    RefsTombstone, SearchPurge,
};
use myelin_storage::git_shred::GitCryptoShredReach;
use myelin_storage::kms::{KekId, KeyClass, KmsEngine};
use myelin_tenancy::Region;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Mutex;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}
fn region() -> Region {
    Region("fr-par".into())
}
fn subject() -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId("p-opaque-ada".into()),
        PrincipalKind::Human,
        tenant(),
    ))
}
fn subject_id() -> SubjectId {
    SubjectId::new("p-opaque-ada")
}

#[derive(Default)]
struct OkSeam {
    ran: Mutex<bool>,
}
impl OkSeam {
    fn did_run(&self) -> bool {
        *self.ran.lock().unwrap()
    }
    fn mark(&self) -> Result<(), EraseError> {
        *self.ran.lock().unwrap() = true;
        Ok(())
    }
}
impl PseudonymShred for OkSeam {
    fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.mark()
    }
}
impl SearchPurge for OkSeam {
    fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.mark()
    }
}
impl RefsTombstone for OkSeam {
    fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.mark()
    }
}
impl BusErase for OkSeam {
    fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.mark()
    }
}

#[derive(Default)]
struct Ledger {
    erased: Mutex<HashSet<String>>,
}
impl ErasureLedgerSink for Ledger {
    fn record_erasure(&self, subject: &SubjectId, _t: &TenantId, _at: EpochMillis) {
        self.erased.lock().unwrap().insert(subject.0.clone());
    }
    fn is_erased(&self, subject: &SubjectId, _t: &TenantId) -> bool {
        self.erased.lock().unwrap().contains(&subject.0)
    }
}

struct Inv {
    seen: RefCell<Vec<CacheNamespace>>,
}
impl CacheInvalidator for Inv {
    fn invalidate(
        &self,
        _t: &TenantId,
        _r: &RepoLoc,
        ns: CacheNamespace,
    ) -> Result<usize, GitCoreError> {
        self.seen.borrow_mut().push(ns);
        Ok(1)
    }
}

fn engine() -> KmsEngine {
    let kms = KmsEngine::new();
    let (t, r) = (tenant(), region());
    kms.ensure_kek(&KekId::new(t.clone(), r.clone()))
        .expect("seed the in-memory KEK");
    kms.ensure_dek(&t, &r, KeyClass::Subject("p-opaque-ada".into()))
        .expect("subject dek");
    kms.ensure_dek(&t, &r, KeyClass::Blob).expect("blob dek");
    kms
}

struct DsrOrchestrator<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}
impl<'a> DsrOrchestrator<'a> {
    fn fan_out_locate(&self, subject: &SubjectRef, tenant: TenantId) -> Vec<LocateReport> {
        self.holders
            .iter()
            .map(|h| {
                h.locate(subject, tenant.clone())
                    .expect("H1 locate via the contract")
            })
            .collect()
    }
}

#[test]
fn dsr_orchestrator_fans_locate_out_to_the_git_h1_holder_via_the_contract() {
    let eng = engine();
    let holder = GitPersonalDataHolder::new(
        &eng,
        region(),
        Inv {
            seen: RefCell::new(vec![]),
        },
    );
    let consumer = DsrOrchestrator {
        holders: vec![&holder],
    };

    let reports = consumer.fan_out_locate(&subject(), tenant());
    assert_eq!(
        reports.len(),
        1,
        "the git H1 holder responded to locate via the contract"
    );
    let r = &reports[0];
    assert_eq!(r.receipt.operation, "locate");
    assert!(
        r.receipt.content_hash.starts_with("blake3:"),
        "content-addressed receipt"
    );
    assert!(
        r.receipt.key_epoch_destroyed.is_none(),
        "locate shreds no key"
    );
}

#[test]
fn the_contract_erase_refuses_loud_and_points_at_the_real_fan_out() {
    let eng = engine();
    let holder = GitPersonalDataHolder::new(
        &eng,
        region(),
        Inv {
            seen: RefCell::new(vec![]),
        },
    );
    let err = holder
        .erase(EraseScope::Subject {
            subject: subject(),
            tenant: tenant(),
        })
        .expect_err("the contract-shaped erase refuses without wired seams");
    assert!(
        err.0.contains("wired cross-holder seams"),
        "loud refusal names the requirement"
    );
    assert!(
        err.0.contains("erase_fanout"),
        "points the caller at the real §6.1 fan-out"
    );
}

#[test]
fn the_real_dsr_fan_out_reaches_every_git_holder_git_d2_complete() {
    let eng = engine();
    let git_reach = GitCryptoShredReach::new(&eng, region());
    let (pseudonym, search, refs, bus) = (
        OkSeam::default(),
        OkSeam::default(),
        OkSeam::default(),
        OkSeam::default(),
    );
    let ledger = Ledger::default();
    let holder = GitPersonalDataHolder::new(
        &eng,
        region(),
        Inv {
            seen: RefCell::new(vec![]),
        },
    );
    let bundle = EraseHolders {
        pseudonym: &pseudonym,
        search: &search,
        refs: &refs,
        bus: &bus,
        ledger: &ledger,
        git_reach: Some(&git_reach),
    };

    let receipt = holder
        .erase_fanout(&subject_id(), &tenant(), &bundle, 1_000)
        .expect("the real git DSR fan-out is GIT-D2-green");

    assert!(
        receipt.is_green(),
        "GIT-D2: erase reaches every holder + backups shredded"
    );
    assert!(
        receipt.missed_holders().is_empty(),
        "0 holders missed (a missed holder is a breach)"
    );
    assert_eq!(
        receipt.holders_hit.len(),
        GitHolder::ALL.len(),
        "all 8 git holders hit"
    );
    assert_eq!(
        receipt.recoverable_in_backup, 0,
        "0 recoverable PII in any backup"
    );
    assert_eq!(
        receipt.cache_namespaces_invalidated.len(),
        CacheNamespace::ALL.len()
    );
    assert_eq!(receipt.residual, GitResidualPosture::OnePlatformPosture);
    assert!(pseudonym.did_run() && search.did_run() && refs.did_run() && bus.did_run());
    assert!(
        ledger.is_erased(&subject_id(), &tenant()),
        "the erasure ledger recorded the subject"
    );
    assert_eq!(receipt.audit_receipt.operation, "erase");
    assert!(receipt.audit_receipt.content_hash.starts_with("blake3:"));
}

#[test]
fn git_holder_export_rectify_restrict_return_content_addressed_receipts() {
    let eng = engine();
    let holder = GitPersonalDataHolder::new(
        &eng,
        region(),
        Inv {
            seen: RefCell::new(vec![]),
        },
    );

    let exp = holder.export(&subject(), tenant()).expect("export");
    assert_eq!(exp.receipt.operation, "export");
    assert!(exp.receipt.content_hash.starts_with("blake3:"));

    let rec = holder
        .rectify(&subject(), myelin_gdpr::Patch("title: redacted".into()))
        .expect("rectify");
    assert_eq!(rec.receipt.operation, "rectify");

    let on = holder.restrict(&subject(), true).expect("restrict on");
    assert_eq!(on.receipt.operation, "restrict");
}
