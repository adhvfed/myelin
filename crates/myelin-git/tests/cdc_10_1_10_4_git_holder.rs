//! # CDC 10.1 / 10.4 — the git side of `PersonalDataHolder{locate, export, rectify, restrict,
//! erase}` + the DSR fan-out (GIT-P29 → P-290, M3-G7; GIT-D2 complete)
//!
//! **Contract:** index rows **10.1** (`PersonalDataHolder` — the five DSR operations) + **10.4**
//! (the DSR state machine / fan-out). The SIGNATURE was frozen at P-GA-01 (`myelin-gdpr`); THIS file
//! ships the **git side** of 10.1/10.4 — the H1 holder ([`GitPersonalDataHolder`]) IMPLEMENTING the
//! five-operation contract over git + its hosting metadata, with the REAL §6.1 erasure fan-out
//! (GIT-D2 complete: every holder hit, residual == the ONE posture, backups shredded). It is the
//! provider+consumer CDC pair the contract-coverage scanner reads for the git holder seam.
//!
//! - **PROVIDER** = the git H1 holder ([`GitPersonalDataHolder`]) implementing the five-operation
//!   10.1 contract. `locate`/`export`/`rectify`/`restrict` return content-addressed receipts; the
//!   contract-shaped `erase(EraseScope)` REFUSES loud (the documented EI-01 §1 deviation — the frozen
//!   signature carries no cross-holder seam bundle, so the honest body refuses rather than claim an
//!   un-wired erase succeeded), pointing the caller at the real fan-out
//!   [`GitPersonalDataHolder::erase_fanout`] which IS GIT-D2-green.
//! - **CONSUMER** = a minimal DSR-orchestrator stand-in that holds the git holder behind
//!   `dyn PersonalDataHolder`, fans `locate` out via the contract, and drives the real §6.1 fan-out
//!   (the shape the real GDPR orchestrator P-GA-06/P-GA-11 takes when it fans a DSR out to H1).
//!
//! The dated green artifact: the consumer fans `locate(subject)` out to H1 (a content-addressed
//! receipt over its git surface); the real `erase_fanout` reaches EVERY git holder (pseudonym map +
//! per-subject DEK bodies + git structures + search + refs + bus + cache/CDN + ledger), with 0
//! recoverable PII in any backup and the residual == the ONE platform posture (10.9 / X-7). If 10.1's
//! body shape drifts, this stops compiling/passing — that is the contract.

use myelin_git::code_tools::{CacheInvalidator, CacheNamespace};
use myelin_git::core::{GitCoreError, RepoLoc};
use myelin_git::holder::{GitHolder, GitPersonalDataHolder, GitResidualPosture};
use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, SubjectRef, TenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::encryption::SubjectId;
use myelin_storage::erase::{
    BusErase, EraseError, EraseHolders, EpochMillis, ErasureLedgerSink, PseudonymShred,
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

// ── the cross-holder seams the DSR orchestrator wires (the real subsystem holders, stubbed here) ──

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
    kms.ensure_kek(&KekId::new(t.clone(), r.clone()));
    kms.ensure_dek(&t, &r, KeyClass::Subject("p-opaque-ada".into())).expect("subject dek");
    kms.ensure_dek(&t, &r, KeyClass::Blob).expect("blob dek");
    kms
}

/// **The CONSUMER side (10.1): a DSR-orchestrator shape that fans out to H1 via the contract.** It
/// holds the holder behind `dyn PersonalDataHolder` + calls the contract — it never reaches into a
/// store. This is the shape the real GDPR orchestrator (P-GA-06/P-GA-11) takes when it fans a DSR out
/// to the git H1 holder.
struct DsrOrchestrator<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}
impl<'a> DsrOrchestrator<'a> {
    fn fan_out_locate(&self, subject: &SubjectRef, tenant: TenantId) -> Vec<LocateReport> {
        self.holders
            .iter()
            .map(|h| h.locate(subject, tenant.clone()).expect("H1 locate via the contract"))
            .collect()
    }
}

/// **provider + consumer wired together (the 10.1 git CDC pair).** The orchestrator (consumer) fans
/// `locate` out to the git H1 holder (provider); it returns a content-addressed receipt over its git
/// surface — the frozen 10.1 contract is honoured. This is the dated green artifact for the git side
/// of 10.1.
#[test]
fn dsr_orchestrator_fans_locate_out_to_the_git_h1_holder_via_the_contract() {
    let eng = engine();
    let holder = GitPersonalDataHolder::new(&eng, region(), Inv { seen: RefCell::new(vec![]) });
    let consumer = DsrOrchestrator { holders: vec![&holder] };

    let reports = consumer.fan_out_locate(&subject(), tenant());
    assert_eq!(reports.len(), 1, "the git H1 holder responded to locate via the contract");
    let r = &reports[0];
    assert_eq!(r.receipt.operation, "locate");
    assert!(r.receipt.content_hash.starts_with("blake3:"), "content-addressed receipt");
    assert!(r.receipt.key_epoch_destroyed.is_none(), "locate shreds no key");
}

/// **The contract-shaped `erase(EraseScope)` REFUSES loud (the documented EI-01 §1 deviation), never
/// a false 'erased'.** The frozen 10.1 signature carries no cross-holder seam bundle, so the honest
/// body refuses + points the caller at the real fan-out — never claims an un-wired erase succeeded.
#[test]
fn the_contract_erase_refuses_loud_and_points_at_the_real_fan_out() {
    let eng = engine();
    let holder = GitPersonalDataHolder::new(&eng, region(), Inv { seen: RefCell::new(vec![]) });
    let err = holder
        .erase(EraseScope::Subject { subject: subject(), tenant: tenant() })
        .expect_err("the contract-shaped erase refuses without wired seams");
    assert!(err.0.contains("wired cross-holder seams"), "loud refusal names the requirement");
    assert!(err.0.contains("erase_fanout"), "points the caller at the real §6.1 fan-out");
}

/// **The REAL §6.1 DSR fan-out reaches EVERY git holder (GIT-D2 complete) via the wired seams.** This
/// is the shape the GDPR orchestrator drives: every git holder hit, 0 recoverable PII in any backup,
/// every cache/CDN namespace invalidated, residual == the ONE platform posture (10.9 / X-7). If the
/// holder set or the §6.1 ordering drifts, this stops passing — that is the 10.4 contract.
#[test]
fn the_real_dsr_fan_out_reaches_every_git_holder_git_d2_complete() {
    let eng = engine();
    let git_reach = GitCryptoShredReach::new(&eng, region());
    let (pseudonym, search, refs, bus) =
        (OkSeam::default(), OkSeam::default(), OkSeam::default(), OkSeam::default());
    let ledger = Ledger::default();
    let holder = GitPersonalDataHolder::new(&eng, region(), Inv { seen: RefCell::new(vec![]) });
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

    // GIT-D2: every holder hit, residual == the posture, backups shredded.
    assert!(receipt.is_green(), "GIT-D2: erase reaches every holder + backups shredded");
    assert!(receipt.missed_holders().is_empty(), "0 holders missed (a missed holder is a breach)");
    assert_eq!(receipt.holders_hit.len(), GitHolder::ALL.len(), "all 8 git holders hit");
    assert_eq!(receipt.recoverable_in_backup, 0, "0 recoverable PII in any backup");
    assert_eq!(receipt.cache_namespaces_invalidated.len(), CacheNamespace::ALL.len());
    assert_eq!(receipt.residual, GitResidualPosture::OnePlatformPosture);
    // The §6.1 cross-holder seams actually ran (the fan-out is real).
    assert!(pseudonym.did_run() && search.did_run() && refs.did_run() && bus.did_run());
    assert!(ledger.is_erased(&subject_id(), &tenant()), "the erasure ledger recorded the subject");
    // The audit receipt is content-addressed (the hash-link; the Merkle seal is P-GA-20).
    assert_eq!(receipt.audit_receipt.operation, "erase");
    assert!(receipt.audit_receipt.content_hash.starts_with("blake3:"));
}

/// **`export`/`rectify`/`restrict` over the git surface return content-addressed receipts (the frozen
/// 10.1 shape).** A real, callable holder — never a `todo!()`/panic.
#[test]
fn git_holder_export_rectify_restrict_return_content_addressed_receipts() {
    let eng = engine();
    let holder = GitPersonalDataHolder::new(&eng, region(), Inv { seen: RefCell::new(vec![]) });

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
