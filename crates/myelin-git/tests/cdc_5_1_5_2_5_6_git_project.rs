use myelin_git::body::Body;
use myelin_git::check_status::GateOutcome;
use myelin_git::lifecycle::PullRequest;
use myelin_git::project::{
    git_commit_ref, git_pr_ref, ArtifactStore, CommitMeta, Projected, Projector,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, DataRole, Decision, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind,
    PrincipalStatus, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashSet;

fn provider_canonical_keys() -> Vec<(ArtifactRef, &'static str)> {
    vec![
        (
            git_pr_ref("acme", "payments", 1421).unwrap(),
            "myelin://acme/git/pr/payments:1421",
        ),
        (
            git_commit_ref("acme", "payments", "blake3:deadbeefcafe").unwrap(),
            "myelin://acme/git/commit/payments:blake3:deadbeefcafe",
        ),
    ]
}

#[test]
fn provider_mints_canonical_keys_that_round_trip_and_have_no_stored_display_key() {
    for (key, expect) in provider_canonical_keys() {
        assert_eq!(myelin_refs::format(&key), expect);
        assert_eq!(myelin_refs::parse(expect).unwrap(), key);
        if let Some(disp) = myelin_git::project::display_key(&key) {
            assert!(
                myelin_refs::parse(&disp).is_err(),
                "the display key `{disp}` must NOT re-parse to a scope (0 stored display keys)"
            );
        }
    }
}

struct StubId {
    allow: HashSet<String>,
}
impl StubId {
    fn allowing(objects: &[&ArtifactRef]) -> Self {
        Self {
            allow: objects.iter().map(|o| format!("view@{}", o.0)).collect(),
        }
    }
    fn denying_all() -> Self {
        Self {
            allow: HashSet::new(),
        }
    }
}
impl IdentityService for StubId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        let key = format!("{}@{}", permission.0, object.0);
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

fn seeded_projector(authorized: bool) -> (Projector<StubId>, ArtifactRef, ArtifactRef) {
    let pr_ref = git_pr_ref("acme", "payments", 1421).unwrap();
    let commit_ref = git_commit_ref("acme", "payments", "blake3:deadbeefcafe").unwrap();
    let mut store = ArtifactStore::new();
    let mut pr = PullRequest::open(
        1421,
        "refs/heads/main",
        "refs/heads/feature",
        "psn:alice",
        false,
    );
    pr.body = Body::new("Harden the retry path", vec![]);
    store.put_pr(&pr_ref, pr, GateOutcome::AllRequiredGreen, 1, 1);
    store.put_commit(
        &commit_ref,
        CommitMeta {
            subject: "Fix the leak".into(),
            verified: true,
        },
    );
    let id = if authorized {
        StubId::allowing(&[&pr_ref, &commit_ref])
    } else {
        StubId::denying_all()
    };
    (Projector::new(id, store), pr_ref, commit_ref)
}

#[test]
fn provider_project_is_permission_first_deny_yields_a_tombstone_with_no_title() {
    let (projector, pr_ref, _commit) = seeded_projector(false);
    let got = projector
        .project(&pr_ref, &viewer("mallory"), Zookie("z".into()))
        .unwrap();
    assert!(got.is_tombstone());
    assert_eq!(
        got.title(),
        None,
        "the provider's deny path never reads the title (0 leak)"
    );
}

fn consumer_render(projector: &Projector<StubId>, r: &ArtifactRef, v: &Principal) -> String {
    match projector.project(r, v, Zookie("z".into())) {
        Ok(Projected::Visible(p)) => format!("{}|{}|{}", p.icon, p.state, p.title),
        Ok(Projected::Tombstoned(t)) => t.display_text().to_string(),
        Err(e) => format!("ERR:{e}"),
    }
}

#[test]
fn consumer_reads_the_projection_for_an_authorized_viewer() {
    let (projector, pr_ref, commit_ref) = seeded_projector(true);
    assert_eq!(
        consumer_render(&projector, &pr_ref, &viewer("alice")),
        "pr|open|Harden the retry path"
    );
    assert_eq!(
        consumer_render(&projector, &commit_ref, &viewer("alice")),
        "commit|verified|deadbee Fix the leak"
    );
}

#[test]
fn consumer_gets_a_content_free_tombstone_for_an_unauthorized_viewer() {
    let (projector, pr_ref, _commit) = seeded_projector(false);
    let rendered = consumer_render(&projector, &pr_ref, &viewer("mallory"));
    assert_eq!(
        rendered, "(not available)",
        "0 leak - the consumer never sees the title"
    );
    assert!(
        !rendered.contains("Harden"),
        "the title must NOT appear anywhere in a consumer's tombstone render"
    );
}
