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

fn seed_subject(eng: &KmsEngine, subject: &str) {
    use myelin_issues::{encrypt_free_text, IssueFreeText};
    let _ = encrypt_free_text(
        eng,
        &region(),
        &tenant(),
        &SubjectId::new(subject),
        IssueFreeText::Title,
        b"fix the login bug for Ada Lovelace, customer ada@example.com",
    )
    .expect("seal the free-text under the subject DEK");
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

#[test]
fn iss_d11_erase_reaches_every_holder_with_post_restore_re_erasure() {
    let subject = "8a2f@acme.noreply";
    let eng = KmsEngine::new();
    seed_subject(&eng, subject);
    let id = DrillId::new();
    let restriction = RestrictionFlag::new();
    let fanout = IssueEraseFanout::new(&eng, region(), restriction.clone(), &id);
    let ledger = IssueErasureLedger::new(tenant(), region());

    assert!(
        eng.resolve_dek(&ft_ref(subject), &region()).is_ok(),
        "the subject's free-text is recoverable before the erase"
    );

    let outcome = fanout
        .erase(subject, &tenant(), &ledger, "2026-06-23T00:00:00Z")
        .expect("the Issues erase reaches every holder");

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

    assert!(
        eng.resolve_dek(&ft_ref(subject), &region()).is_err(),
        "the per-subject DEK is crypto-shredded - 0 recoverable free-text PII"
    );
    assert_eq!(
        id.shreds.load(Ordering::SeqCst),
        1,
        "the pseudonym map was shredded (4.8)"
    );
    assert!(
        restriction.is_restricted(subject),
        "the erased subject is restricted (OLAP honours it)"
    );
    assert_eq!(
        outcome.tombstones_emitted, 3,
        "OLAP + Search + Refs tombstoned"
    );
    assert_eq!(
        ERASED_TOMBSTONE_TOKENS.len(),
        2,
        "issue.issue.erased + issue.comment.erased"
    );

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
    assert!(restriction.is_restricted(subject));

    println!(
        "ISS-D11 GREEN: erase reached every holder + 0 resurrected post-restore (dated 2026-06-23)"
    );
}
