use myelin_git::body::Body;
use myelin_git::check_status::GateOutcome;
use myelin_git::lifecycle::PullRequest;
use myelin_git::project::{
    display_key, git_pr_ref, ArtifactStore, Projected, Projector, TombstoneReason,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, DataRole, Decision, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind,
    PrincipalStatus, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashSet;

struct StubId {
    allow: HashSet<String>,
}
impl StubId {
    fn granting(subject: &str, objects: &[&ArtifactRef]) -> Self {
        Self {
            allow: objects
                .iter()
                .map(|o| format!("{subject}:view@{}", o.0))
                .collect(),
        }
    }
}
impl IdentityService for StubId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let key = format!("{}:{}@{}", subject.principal_id.0, permission.0, object.0);
        Ok(if self.allow.contains(&key) {
            Decision::Allow
        } else {
            Decision::Deny
        })
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
    fn delegation(
        &self,
        _a: &Principal,
        _t: &Principal,
    ) -> IdResult<myelin_identity::EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn write_tuples(
        &self,
        _d: &[TupleDelta],
        _p: Option<&myelin_identity::Precondition>,
    ) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &myelin_identity::RunId,
        _d: &myelin_identity::DelegationCaveats,
        _t: &myelin_identity::FailStaticBound,
    ) -> IdResult<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn admit_fragment(
        &self,
        _f: &myelin_identity::NamespaceFragment,
    ) -> IdResult<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
}

fn viewer(id: &str) -> Principal {
    Principal::new(
        TenantId("acme".into()),
        Region("fr-par".into()),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

#[test]
fn project_pr_authorized_gets_title_unauthorized_gets_tombstone_zero_leak() {
    let pr_ref = git_pr_ref("acme", "payments", 1421).unwrap();
    assert_eq!(
        myelin_refs::format(&pr_ref),
        "myelin://acme/git/pr/payments:1421",
        "git's stored canonical key is `pr/<repo>:<n>` (REF-3) - never the `#1421` display"
    );
    assert_eq!(display_key(&pr_ref).as_deref(), Some("#1421"));
    assert!(
        myelin_refs::parse("#1421").is_err(),
        "the `#1421` display is render-time only - 0 stored display keys (it never re-parses to a scope)"
    );

    let mut pr = PullRequest::open(
        1421,
        "refs/heads/main",
        "refs/heads/feature",
        "psn:alice",
        false,
    );
    pr.body = Body::new("Harden the payment retry path", vec![]);
    let mut store = ArtifactStore::new();
    store.put_pr(&pr_ref, pr, GateOutcome::AllRequiredGreen, 2, 1);

    let projector = Projector::new(StubId::granting("alice", &[&pr_ref]), store);

    let alice = projector
        .project(&pr_ref, &viewer("alice"), Zookie("z".into()))
        .unwrap();
    assert!(
        alice.is_visible(),
        "an authorized viewer gets a visible projection"
    );
    assert_eq!(
        alice.title(),
        Some("Harden the payment retry path"),
        "the authorized viewer gets the title"
    );

    let mallory = projector
        .project(&pr_ref, &viewer("mallory"), Zookie("z".into()))
        .unwrap();
    assert!(
        mallory.is_tombstone(),
        "an unauthorized viewer gets a tombstone"
    );
    assert_eq!(
        mallory.title(),
        None,
        "0 title leak - the unauthorized viewer NEVER gets the title (feeds GIT-D11 / SRCH-D1/D3)"
    );
    if let Projected::Tombstoned(t) = mallory {
        assert_eq!(t.reason, TombstoneReason::Unauthorized);
        assert_eq!(t.display_text(), "(not available)");
    }
}
