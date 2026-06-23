//! **ISS-D11 / P-385 (M4-I8, the band exit) — erase-reaches-every-holder + post-restore re-erasure.**
//!
//! The drill scenario for the Issues GDPR band-exit gate (testing-strategy row ISS-D11): erase a data
//! subject → assert the PII is gone from EVERY Issues holder (per-subject DEK, change-log, comments,
//! attachments, OLAP + restriction, Search incl. embeddings, Refs) with a PER-HOLDER receipt → a backup
//! restore resurrects the key → post-restore re-erasure (GD-14) re-destroys it → 0 resurrected. The
//! third-party free-text residual is the documented `[OPEN — LEGAL]` limit (the ONE posture 10.9/X-7, by
//! reference). The per-holder receipts + the 0-resurrected re-erasure ARE the dated green artifact.
//!
//! This drill is DB-free: it runs the REAL per-subject-DEK crypto-shred over the in-memory `KmsEngine`
//! (the SAME engine ISS-P07 seals the free-text under) — the live-Postgres at-rest round-trip rides the
//! ISS-P07 integration drill (`integration_iss_p07_subject_dek.rs`).

use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_issues::{
    HolderTarget, IssueEraseFanout, IssueErasureLedger, RestrictionFlag, ERASED_TOMBSTONE_TOKENS,
};
use myelin_storage::encryption::SubjectId;
use myelin_storage::kms::{KeyClass, KmsEngine, PiiKeyRef};
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicUsize, Ordering};

type IdResult<T> = myelin_identity::Result<T>;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region::new("fr-par")
}

/// The Identity surface whose `erase` shreds the person↔pseudonym map (4.8) — counts the shred so the
/// drill proves the pseudonym-map shred reached. The REAL map is Identity's store (test scaffolding).
struct DrillId {
    shreds: AtomicUsize,
}
impl DrillId {
    fn new() -> Self {
        DrillId {
            shreds: AtomicUsize::new(0),
        }
    }
}
impl IdentityService for DrillId {
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        self.shreds.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &myelin_tenancy::ArtifactRef,
        _a: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _a: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _a: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _a: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &RunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> IdResult<RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

/// Seal a subject's free-text + attachment-blob DEK so the erase has a REAL key to crypto-shred.
fn seed_subject(eng: &KmsEngine, subject: &str) {
    use myelin_issues::{encrypt_free_text, IssueFreeText};
    // the free-text DEK (the SAME class ISS-P07's encrypt_free_text seals under).
    let _ = encrypt_free_text(
        eng,
        &region(),
        &tenant(),
        &SubjectId::new(subject),
        IssueFreeText::Title,
        b"fix the login bug for Ada Lovelace, customer ada@example.com",
    )
    .expect("seal the free-text under the subject DEK");
    // the attachment-blob DEK.
    eng.ensure_dek(
        &tenant(),
        &region(),
        KeyClass::Subject(format!("{subject}/blob")),
    )
    .expect("ensure the attachment-blob DEK");
}

fn ft_ref(subject: &str) -> PiiKeyRef {
    PiiKeyRef::new(tenant(), 0, KeyClass::Subject(subject.to_string()))
}

/// **ISS-D11 — the chained-mutation drill: erase → every-holder receipt → restore → re-erasure.**
#[test]
fn iss_d11_erase_reaches_every_holder_with_post_restore_re_erasure() {
    let subject = "8a2f@acme.noreply";
    let eng = KmsEngine::new();
    seed_subject(&eng, subject);
    let id = DrillId::new();
    let restriction = RestrictionFlag::new();
    let fanout = IssueEraseFanout::new(&eng, region(), restriction.clone(), &id);
    let ledger = IssueErasureLedger::new(tenant(), region());

    // ── pre-erase: the subject's free-text DEK is LIVE (their PII is recoverable) ──
    assert!(
        eng.resolve_dek(&ft_ref(subject), &region()).is_ok(),
        "the subject's free-text is recoverable before the erase"
    );

    // ── (1) ERASE: the fan-out reaches every Issues holder ──
    let outcome = fanout
        .erase(subject, &tenant(), &ledger, "2026-06-23T00:00:00Z")
        .expect("the Issues erase reaches every holder");

    // every holder reached, each with a content-addressed per-holder receipt (the green artifact).
    assert!(
        outcome.reached_every_holder(),
        "PII gone from EVERY Issues holder (per-holder receipts)"
    );
    for target in HolderTarget::ALL {
        let receipt = outcome
            .per_holder
            .iter()
            .find(|r| r.holder == target)
            .unwrap_or_else(|| panic!("holder {} has no erase receipt", target.label()));
        assert!(
            receipt.receipt.content_hash.starts_with("blake3:"),
            "{} receipt is content-addressed",
            target.label()
        );
        println!(
            "ISS-D11 holder receipt: {:<16} {} (work={})",
            target.label(),
            receipt.receipt.content_hash,
            receipt.did_work
        );
    }

    // the headline DEK is DEAD — the free-text/change-log/comments/worklog is unrecoverable.
    assert!(
        eng.resolve_dek(&ft_ref(subject), &region()).is_err(),
        "the per-subject DEK is crypto-shredded — 0 recoverable free-text PII"
    );
    // the pseudonym map was shredded ("Former user 8a2f" without rewriting issues others own).
    assert_eq!(
        id.shreds.load(Ordering::SeqCst),
        1,
        "the pseudonym map was shredded (4.8)"
    );
    // the erased subject is restricted (no analytics/agent-use/notif).
    assert!(
        restriction.is_restricted(subject),
        "the erased subject is restricted (OLAP honours it)"
    );
    // the issue.*.erased tombstones were emitted (Search/Refs/OLAP consume them).
    assert_eq!(
        outcome.tombstones_emitted, 3,
        "OLAP + Search + Refs tombstoned"
    );
    assert_eq!(
        ERASED_TOMBSTONE_TOKENS.len(),
        2,
        "issue.issue.erased + issue.comment.erased"
    );

    // ── (2) RESTORE an OLDER backup: it resurrects the subject's DEKs (the pre-erase state) ──
    eng.ensure_dek(&tenant(), &region(), KeyClass::Subject(subject.to_string()))
        .expect("the restore resurrected the free-text DEK");
    eng.ensure_dek(
        &tenant(),
        &region(),
        KeyClass::Subject(format!("{subject}/blob")),
    )
    .expect("the restore resurrected the blob DEK");
    assert!(
        eng.resolve_dek(&ft_ref(subject), &region()).is_ok(),
        "the restore RESURRECTED the subject's free-text DEK (the GD-14 hazard)"
    );

    // ── (3) POST-RESTORE RE-ERASURE (GD-14): replay the ledger, re-destroy the resurrected DEKs ──
    let reerase = fanout.re_erase_after_restore(&ledger, "2026-06-23T01:00:00Z");
    println!(
        "ISS-D11 re-erasure: subjects={} resurrected_by_restore={} re_emitted_tombstones={} resurrected={}",
        reerase.re_erased_subjects,
        reerase.deks_resurrected_by_restore,
        reerase.tombstones_re_emitted,
        reerase.resurrected
    );
    assert_eq!(reerase.re_erased_subjects, 1);
    assert_eq!(
        reerase.deks_resurrected_by_restore, 2,
        "the restore brought back the free-text + blob DEKs"
    );
    // THE GATE: 0 resurrected PII keys post-restore.
    assert_eq!(
        reerase.resurrected, 0,
        "0 resurrected Issues PII keys post-restore"
    );
    assert!(
        reerase.is_green(),
        "the key stays destroyed across the restore (GD-14)"
    );
    assert!(
        eng.resolve_dek(&ft_ref(subject), &region()).is_err(),
        "the subject's free-text is unrecoverable again after the re-erasure"
    );
    // the subject stays restricted across the restore.
    assert!(restriction.is_restricted(subject));

    println!(
        "ISS-D11 GREEN: erase reached every holder + 0 resurrected post-restore (dated 2026-06-23)"
    );
}
