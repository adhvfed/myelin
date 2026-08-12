use myelin_gdpr::{
    EraseScope, LocateReport, PersonalDataHolder, PortableBundle, Receipt, SubjectRef, TenantId,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_issues::{
    HolderTarget, IssueEraseFanout, IssueErasureLedger, IssueHolder, RestrictionFlag,
};
use myelin_storage::encryption::SubjectId;
use myelin_storage::kms::{KeyClass, KmsEngine, PiiKeyRef};
use myelin_tenancy::{Region, TenantId as TenancyTenantId};

type IdResult<T> = myelin_identity::Result<T>;

fn tenant() -> TenantId {
    TenancyTenantId("acme".into())
}
fn region() -> Region {
    Region::new("fr-par")
}
fn subject_ref(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

struct CdcId;
impl IdentityService for CdcId {
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Ok(())
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenancyTenantId) -> IdResult<String> {
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

#[test]
fn provider_issue_holder_responds_to_the_frozen_holder_surface() {
    let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(IssueHolder::new())];
    let subj = subject_ref("8a2f@acme.noreply");
    for h in &holders {
        let loc: LocateReport = h.locate(&subj, tenant()).expect("locate responds");
        assert_eq!(loc.receipt.operation, "locate");
        let exp: PortableBundle = h.export(&subj, tenant()).expect("export responds");
        assert_eq!(exp.receipt.operation, "export");
        let er = h
            .erase(EraseScope::Subject {
                subject: subj.clone(),
                tenant: tenant(),
            })
            .expect("erase responds with the typed aggregate receipt");
        assert_eq!(er.receipt.operation, "erase");
        assert!(er.receipt.content_hash.starts_with("blake3:"));
    }
}

#[test]
fn consumer_dsr_drives_the_full_issues_erase_fanout() {
    let subject = "8a2f@acme.noreply";
    let eng = KmsEngine::new();
    use myelin_issues::{encrypt_free_text, IssueFreeText};
    let _ = encrypt_free_text(
        &eng,
        &region(),
        &tenant(),
        &SubjectId::new(subject),
        IssueFreeText::Title,
        b"private title with PII",
    )
    .expect("seal");
    eng.ensure_dek(
        &tenant(),
        &region(),
        KeyClass::Subject(format!("{subject}/blob")),
    )
    .expect("blob dek");

    let id = CdcId;
    let restriction = RestrictionFlag::new();
    let fanout = IssueEraseFanout::new(&eng, region(), restriction.clone(), &id);
    let ledger = IssueErasureLedger::new(tenant(), region());

    let outcome = fanout
        .erase(subject, &tenant(), &ledger, "2026-06-23T00:00:00Z")
        .expect("the DSR orchestrator drives the fan-out");

    assert!(outcome.reached_every_holder());
    assert_eq!(outcome.per_holder.len(), HolderTarget::ALL.len());

    let ft = PiiKeyRef::new(tenant(), 0, KeyClass::Subject(subject.to_string()));
    assert!(
        eng.resolve_dek(&ft, &region()).is_err(),
        "free-text DEK crypto-shredded"
    );

    eng.ensure_dek(&tenant(), &region(), KeyClass::Subject(subject.to_string()))
        .expect("restore resurrects");
    let reerase = fanout
        .re_erase_after_restore(&ledger, "2026-06-23T01:00:00Z")
        .unwrap();
    assert_eq!(reerase.resurrected, 0, "0 resurrected post-restore (GD-14)");
    assert!(reerase.is_green());
}

#[test]
fn the_frozen_10_1_shapes_are_consumed_verbatim() {
    let _scope = EraseScope::Subject {
        subject: subject_ref("8a2f@acme.noreply"),
        tenant: tenant(),
    };
    let _tenant_scope = EraseScope::Tenant(tenant());
    let r = Receipt::content_addressed(
        "erase",
        "free-text-dek",
        "8a2f@acme.noreply",
        "acme",
        "ok",
        Some(0),
        0,
    );
    assert!(r.content_hash.starts_with("blake3:"));
    assert_eq!(r.key_epoch_destroyed, Some(0));
}
