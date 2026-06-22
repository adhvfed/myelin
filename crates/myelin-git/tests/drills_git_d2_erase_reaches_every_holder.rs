//! # GIT-D2 (COMPLETE) — erase-reaches-every-holder + history-rewrite erasure semantics
//! (GIT-P29 → P-290, M3-G7; drill catalogue row GIT-D2, SCHED)
//!
//! **The drill (catalogue row GIT-D2):** *"Erase a subject who authored commits/PRs/comments + LFS →
//! every holder hit; residual == the ONE platform-posture residual (10.9); crypto-shred reaches
//! backups."* **Green artifact:** the DSR receipt set + the erasure-ledger entry (0 holders missed; 0
//! recoverable PII beyond the named residual; crypto-shred reaches backups).
//!
//! GIT-D2 ships in two halves across the ledger: the **GIT-1 half** (pseudonymous-by-default — the
//! immutable commit bytes carry only the opaque pseudonym, so after the pseudonym-map shred 0 real
//! identity is recoverable) landed at GIT-P12 (`drills_git_d2_pseudonymous_residual.rs` +
//! `drills_git_d2_receive_pack_pseudonymity.rs`); the **completion half** (the full DSR fan-out hits
//! EVERY git holder + the history-rewrite erasure semantics) lands HERE (GIT-P29). This file is the
//! dated green artifact for the completion half.
//!
//! This drill stands up a subject who AUTHORED a PR-comment body (sealed under the per-subject DEK)
//! and whose reflog/bitmap/pack-backup ride the per-tenant blob DEK + uploaded an LFS blob (the blob
//! DEK), runs the full §6.1 erasure fan-out through the git H1 holder, and asserts: (1) EVERY git
//! holder is hit; (2) the per-subject DEK body is unrecoverable LIVE and ABSENT from the backup
//! snapshot (crypto-shred reaches backups); (3) the per-tenant blob DEK (reflog/bitmap/pack-backup +
//! LFS) is absent from the backup; (4) the residual is EXACTLY the ONE platform posture (10.9 / X-7),
//! nothing more; (5) the erasure ledger recorded the subject; (6) the history-rewrite erasure path
//! (10.6) expunges a residual body WITH the fork/mirror/clone-cache invalidation fan-out.

use myelin_git::code_tools::{
    CacheInvalidator, CacheNamespace, HistoryRewritePlan, HistoryRewriteTool, RewriteRateLimiter,
};
use myelin_git::core::{GitCoreError, RepoLoc, WireExecutor, WireInvocation, WireOutput};
use myelin_git::holder::{GitHolder, GitPersonalDataHolder, GitResidualPosture};
use myelin_gdpr::TenantId;
use myelin_storage::encryption::SubjectId;
use myelin_storage::erase::{
    BusErase, EraseError, EraseHolders, EpochMillis, ErasureLedgerSink, PseudonymShred,
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
const SUBJECT: &str = "p-opaque-ada"; // the opaque, pseudonymous principal id (never real identity).

// ── the cross-holder seams the DSR orchestrator wires (the real subsystem holders, stubbed here) ──

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
    fn invalidate(&self, _t: &TenantId, _r: &RepoLoc, ns: CacheNamespace) -> Result<usize, GitCoreError> {
        self.seen.borrow_mut().push(ns);
        Ok(1)
    }
}

struct OkWire;
impl WireExecutor for OkWire {
    fn run(&self, _inv: &WireInvocation) -> Result<WireOutput, GitCoreError> {
        Ok(WireOutput { stdout: vec![], status: 0 })
    }
}

/// **GIT-D2 (completion): erase a subject who authored a PR-comment body + LFS blob + has
/// reflog/bitmap/pack-backups → every holder hit, residual == the ONE posture, crypto-shred reaches
/// backups.** The dated green artifact (DSR receipt set + ledger entry).
#[test]
fn git_d2_complete_erase_reaches_every_holder_residual_is_the_posture_backups_shredded() {
    // ── Stand up the subject's at-rest content. ──
    let (t, r) = (tenant(), region());
    let eng = KmsEngine::new();
    eng.ensure_kek(&KekId::new(t.clone(), r.clone()));
    // The per-SUBJECT DEK: the PR-comment BODY + the PR TITLE seal under it (11.4).
    eng.ensure_dek(&t, &r, KeyClass::Subject(SUBJECT.into())).expect("subject dek");
    // The per-tenant BLOB DEK: the reflog/bitmap/pack-backup + the uploaded LFS blob seal under it.
    eng.ensure_dek(&t, &r, KeyClass::Blob).expect("blob dek");

    // Seal the subject's authored PR-comment body under the per-subject DEK (the at-rest ciphertext).
    let subject_dek_ref = PiiKeyRef::new(t.clone(), 0, KeyClass::Subject(SUBJECT.into()));
    let body = b"PR comment by the subject: this is my review feedback on the change";
    let dek = eng.resolve_dek(&subject_dek_ref, &r).expect("subject dek resolves before erase");
    let (nonce, ct) = dek.seal(body);
    assert_eq!(dek.open(&nonce, &ct).unwrap(), body, "the authored body decrypts BEFORE erase");

    // Seal an uploaded LFS blob under the per-tenant blob DEK (an LFS blob the subject uploaded).
    let blob_ref = PiiKeyRef::new(t.clone(), 0, KeyClass::Blob);
    let lfs = b"\x89PNG... an LFS-stored asset uploaded by the subject";
    let blob_dek = eng.resolve_dek(&blob_ref, &r).expect("blob dek resolves before erase");
    let (lfs_nonce, lfs_ct) = blob_dek.seal(lfs);
    assert_eq!(blob_dek.open(&lfs_nonce, &lfs_ct).unwrap(), lfs, "the LFS blob decrypts BEFORE erase");

    // ── Wire the §6.1 cross-holder seams + git's structure reach + the cache fan-out. ──
    let git_reach = GitCryptoShredReach::new(&eng, r.clone());
    let (pseudonym, search, refs, bus) =
        (Seam::default(), Seam::default(), Seam::default(), Seam::default());
    let ledger = Ledger::default();
    let holder = GitPersonalDataHolder::new(&eng, r.clone(), Inv { seen: RefCell::new(vec![]) });
    let bundle = EraseHolders {
        pseudonym: &pseudonym,
        search: &search,
        refs: &refs,
        bus: &bus,
        ledger: &ledger,
        git_reach: Some(&git_reach),
    };

    // ── ERASE: run the full §6.1 DSR fan-out. ──
    let receipt = holder
        .erase_fanout(&SubjectId::new(SUBJECT), &t, &bundle, 1_000)
        .expect("the git DSR erase is GIT-D2-green");

    // (1) EVERY git holder hit — 0 missed (a missed holder is a breach).
    assert!(receipt.is_green(), "GIT-D2 GREEN");
    assert!(receipt.missed_holders().is_empty(), "0 holders missed");
    assert_eq!(receipt.holders_hit.len(), GitHolder::ALL.len(), "all 8 git holders hit");
    for h in GitHolder::ALL {
        assert!(receipt.holders_hit.contains(&h), "holder `{}` was hit", h.label());
    }

    // (2) crypto-shred reaches BACKUPS: the per-subject DEK body is unrecoverable live + absent backup.
    assert_eq!(receipt.recoverable_in_backup, 0, "0 recoverable PII in any backup");
    assert!(
        eng.resolve_dek(&subject_dek_ref, &r).is_err(),
        "the authored body is unrecoverable LIVE after erase (the per-subject DEK is destroyed)"
    );
    let subj_dek = DekId::new(t.clone(), KeyClass::Subject(SUBJECT.into()));
    assert!(
        !eng.backup_snapshot().iter().any(|(d, _)| *d == subj_dek),
        "the per-subject DEK is ABSENT from the backup snapshot (crypto-shred reaches backups, §7.5)"
    );

    // (3) the per-tenant blob DEK (reflog/bitmap/pack-backup + LFS) is gone live + absent from backup.
    assert!(
        eng.resolve_dek(&blob_ref, &r).is_err(),
        "the LFS blob + reflog/bitmap/pack-backup are unrecoverable LIVE (the blob DEK is destroyed)"
    );
    let blob_dek_id = DekId::new(t.clone(), KeyClass::Blob);
    assert!(
        !eng.backup_snapshot().iter().any(|(d, _)| *d == blob_dek_id),
        "the per-tenant blob DEK is ABSENT from the backup snapshot (reflog/bitmap/pack/LFS shredded)"
    );

    // (4) the residual is EXACTLY the ONE platform posture (10.9 / X-7), nothing more.
    assert_eq!(receipt.residual, GitResidualPosture::OnePlatformPosture);
    assert!(GitResidualPosture::RESIDUAL_POSTURE_REF.contains("10.9"));

    // (5) the erasure-ledger entry exists (the green artifact's second half).
    assert!(ledger.is_erased(&SubjectId::new(SUBJECT), &t), "the erasure ledger recorded the subject");
    // The §6.1 cross-holder seams actually ran (the fan-out is real).
    assert!(pseudonym.did_run() && search.did_run() && refs.did_run() && bus.did_run());

    // The DSR receipt set: the content-addressed audit receipt (the ledger hash-link).
    assert_eq!(receipt.audit_receipt.operation, "erase");
    assert!(receipt.audit_receipt.content_hash.starts_with("blake3:"));
    assert!(receipt.audit_receipt.key_epoch_destroyed.is_some(), "names the destroyed key epoch");
}

/// **GIT-D2 history-rewrite erasure semantics (10.6): the X-7 body-expunge path with the
/// fork/mirror/clone-cache invalidation fan-out.** For the rare residual case a body must be EXPUNGED
/// from the immutable bytes, the holder routes through the GIT-P27 audited tool — sandboxed +
/// rate-limited + the full invalidation fan-out (so no fork/mirror/CDN resurrects the expunged bytes).
#[test]
fn git_d2_history_rewrite_expunges_a_residual_body_with_the_invalidation_fan_out() {
    let eng = KmsEngine::new();
    let holder = GitPersonalDataHolder::new(&eng, region(), Inv { seen: RefCell::new(vec![]) });
    let tool = HistoryRewriteTool::new(OkWire, holder.invalidator());
    let mut limiter = RewriteRateLimiter::new(5);
    let plan = HistoryRewritePlan {
        tenant: tenant(),
        repo: RepoLoc::new("acme", "fr-par", "team/app"),
        target_refs: vec!["refs/heads/main".into()],
        reason_code: "dsr-body".into(), // the X-7 residual body-expunge reason.
    };
    let rewrite = holder
        .expunge_body(&tool, &plan, &mut limiter, 2_000)
        .expect("the residual body-expunge is green");
    assert!(rewrite.is_complete(), "the invalidation fan-out reached EVERY trust-scoped namespace");
    assert_eq!(rewrite.namespaces_invalidated.len(), CacheNamespace::ALL.len());
    assert_eq!(rewrite.receipt.operation, "git.history_rewrite");
    assert!(rewrite.receipt.content_hash.starts_with("blake3:"), "the audited receipt is content-addressed");
}
