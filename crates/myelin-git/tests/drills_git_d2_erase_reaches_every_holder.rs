use myelin_gdpr::TenantId;
use myelin_git::code_tools::{
    CacheInvalidator, CacheNamespace, HistoryRewritePlan, HistoryRewriteTool, RewriteRateLimiter,
};
use myelin_git::core::{GitCoreError, RepoLoc, WireExecutor, WireInvocation, WireOutput};
use myelin_git::holder::{GitHolder, GitPersonalDataHolder, GitResidualPosture};
use myelin_storage::encryption::SubjectId;
use myelin_storage::erase::{
    BusErase, EpochMillis, EraseError, EraseHolders, ErasureLedgerSink, PseudonymShred,
    RefsTombstone, SearchPurge,
};
use myelin_storage::git_shred::GitCryptoShredReach;
use myelin_storage::kms::{DekId, KekId, KeyClass, KmsEngine, PiiKeyRef};
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
const SUBJECT: &str = "p-opaque-ada";

#[derive(Default)]
struct Seam {
    ran: Mutex<bool>,
}
impl Seam {
    fn did_run(&self) -> bool {
        *self.ran.lock().unwrap()
    }
    fn ok(&self) -> Result<(), EraseError> {
        *self.ran.lock().unwrap() = true;
        Ok(())
    }
}
impl PseudonymShred for Seam {
    fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.ok()
    }
}
impl SearchPurge for Seam {
    fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.ok()
    }
}
impl RefsTombstone for Seam {
    fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.ok()
    }
}
impl BusErase for Seam {
    fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
        self.ok()
    }
}

#[derive(Default)]
struct Ledger {
    erased: Mutex<HashSet<String>>,
}
impl ErasureLedgerSink for Ledger {
    fn record_erasure(&self, s: &SubjectId, _t: &TenantId, _at: EpochMillis) {
        self.erased.lock().unwrap().insert(s.0.clone());
    }
    fn is_erased(&self, s: &SubjectId, _t: &TenantId) -> bool {
        self.erased.lock().unwrap().contains(&s.0)
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

struct OkWire;
impl WireExecutor for OkWire {
    fn run(&self, _inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
        Ok(WireOutput {
            stdout: vec![],
            status: 0,
        })
    }
}

#[test]
fn git_d2_complete_erase_reaches_every_holder_residual_is_the_posture_backups_shredded() {
    let (t, r) = (tenant(), region());
    let eng = KmsEngine::new();
    eng.ensure_kek(&KekId::new(t.clone(), r.clone()))
        .expect("seed the in-memory KEK");
    eng.ensure_dek(&t, &r, KeyClass::Subject(SUBJECT.into()))
        .expect("subject dek");
    eng.ensure_dek(&t, &r, KeyClass::Blob).expect("blob dek");

    let subject_dek_ref = PiiKeyRef::new(t.clone(), 0, KeyClass::Subject(SUBJECT.into()));
    let body = b"PR comment by the subject: this is my review feedback on the change";
    let dek = eng
        .resolve_dek(&subject_dek_ref, &r)
        .expect("subject dek resolves before erase");
    let (nonce, ct) = dek.seal(body);
    assert_eq!(
        dek.open(&nonce, &ct).unwrap(),
        body,
        "the authored body decrypts BEFORE erase"
    );

    let blob_ref = PiiKeyRef::new(t.clone(), 0, KeyClass::Blob);
    let lfs = b"\x89PNG... an LFS-stored asset uploaded by the subject";
    let blob_dek = eng
        .resolve_dek(&blob_ref, &r)
        .expect("blob dek resolves before erase");
    let (lfs_nonce, lfs_ct) = blob_dek.seal(lfs);
    assert_eq!(
        blob_dek.open(&lfs_nonce, &lfs_ct).unwrap(),
        lfs,
        "the LFS blob decrypts BEFORE erase"
    );

    let git_reach = GitCryptoShredReach::new(&eng, r.clone());
    let (pseudonym, search, refs, bus) = (
        Seam::default(),
        Seam::default(),
        Seam::default(),
        Seam::default(),
    );
    let ledger = Ledger::default();
    let holder = GitPersonalDataHolder::new(
        &eng,
        r.clone(),
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
        .erase_fanout(&SubjectId::new(SUBJECT), &t, &bundle, 1_000)
        .expect("the git DSR erase is GIT-D2-green");

    assert!(receipt.is_green(), "GIT-D2 GREEN");
    assert!(receipt.missed_holders().is_empty(), "0 holders missed");
    assert_eq!(
        receipt.holders_hit.len(),
        GitHolder::ALL.len(),
        "all 8 git holders hit"
    );
    for h in GitHolder::ALL {
        assert!(
            receipt.holders_hit.contains(&h),
            "holder `{}` was hit",
            h.label()
        );
    }

    assert_eq!(
        receipt.recoverable_in_backup, 0,
        "0 recoverable PII in any backup"
    );
    assert!(
        eng.resolve_dek(&subject_dek_ref, &r).is_err(),
        "the authored body is unrecoverable LIVE after erase (the per-subject DEK is destroyed)"
    );
    let subj_dek = DekId::new(t.clone(), KeyClass::Subject(SUBJECT.into()));
    assert!(
        !eng.backup_snapshot().unwrap().iter().any(|(d, _)| *d == subj_dek),
        "the per-subject DEK is ABSENT from the backup snapshot (crypto-shred reaches backups, §7.5)"
    );

    assert!(
        eng.resolve_dek(&blob_ref, &r).is_err(),
        "the LFS blob + reflog/bitmap/pack-backup are unrecoverable LIVE (the blob DEK is destroyed)"
    );
    let blob_dek_id = DekId::new(t.clone(), KeyClass::Blob);
    assert!(
        !eng.backup_snapshot().unwrap().iter().any(|(d, _)| *d == blob_dek_id),
        "the per-tenant blob DEK is ABSENT from the backup snapshot (reflog/bitmap/pack/LFS shredded)"
    );

    assert_eq!(receipt.residual, GitResidualPosture::OnePlatformPosture);

    assert!(
        ledger.is_erased(&SubjectId::new(SUBJECT), &t),
        "the erasure ledger recorded the subject"
    );
    assert!(pseudonym.did_run() && search.did_run() && refs.did_run() && bus.did_run());

    assert_eq!(receipt.audit_receipt.operation, "erase");
    assert!(receipt.audit_receipt.content_hash.starts_with("blake3:"));
    assert!(
        receipt.audit_receipt.key_epoch_destroyed.is_some(),
        "names the destroyed key epoch"
    );
}

#[test]
fn git_d2_history_rewrite_expunges_a_residual_body_with_the_invalidation_fan_out() {
    let eng = KmsEngine::new();
    let holder = GitPersonalDataHolder::new(
        &eng,
        region(),
        Inv {
            seen: RefCell::new(vec![]),
        },
    );
    let tool = HistoryRewriteTool::new(OkWire, holder.invalidator());
    let mut limiter = RewriteRateLimiter::new(5);
    let plan = HistoryRewritePlan {
        tenant: tenant(),
        repo: RepoLoc::new("acme", "fr-par", "team/app"),
        target_refs: vec!["refs/heads/main".into()],
        reason_code: "dsr-body".into(),
    };
    let rewrite = holder
        .expunge_body(&tool, &plan, &mut limiter, 2_000)
        .expect("the residual body-expunge is green");
    assert!(
        rewrite.is_complete(),
        "the invalidation fan-out reached EVERY trust-scoped namespace"
    );
    assert_eq!(
        rewrite.namespaces_invalidated.len(),
        CacheNamespace::ALL.len()
    );
    assert_eq!(rewrite.receipt.operation, "git.history_rewrite");
    assert!(
        rewrite.receipt.content_hash.starts_with("blake3:"),
        "the audited receipt is content-addressed"
    );
}
