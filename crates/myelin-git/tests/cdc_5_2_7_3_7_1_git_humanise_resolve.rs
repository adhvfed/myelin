use myelin_git::body::Body;
use myelin_git::check_status::GateOutcome;
use myelin_git::git_resolve::{
    git_review_requests_filter, matches_review_requests_view, GitRefResolver,
};
use myelin_git::lifecycle::PullRequest;
use myelin_git::project::{git_pr_ref, ArtifactStore, Projector};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, ConsistencyMode, Credential, Decision, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Principal, PrincipalId, PrincipalKind,
    Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_notif::humanise::{
    humanise, Channel, RefResolution, RefResolvePort, TemplateStore, DEFAULT_LOCALE,
};
use myelin_notif::Reason;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashSet;

const SECRET_TITLE: &str = "merge: the unannounced acquisition term sheet";

fn acme() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::new(
        acme(),
        region(),
        PrincipalId(id.into()),
        PrincipalKind::Human,
        myelin_identity::DataRole::Controller,
        myelin_identity::PrincipalStatus::Active,
    )
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

struct StubId {
    allow: HashSet<String>,
}
impl StubId {
    fn new() -> Self {
        Self {
            allow: HashSet::new(),
        }
    }
    fn allow_view(mut self, o: &ArtifactRef) -> Self {
        self.allow.insert(format!("view@{}", o.0));
        self
    }
}
impl IdentityService for StubId {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn check(
        &self,
        _s: &Principal,
        p: &Permission,
        o: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(if self.allow.contains(&format!("{}@{}", p.0, o.0)) {
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
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("n/a"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
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

fn resolver(granted: bool) -> (GitRefResolver<StubId>, ArtifactRef) {
    let pr_ref = git_pr_ref("acme", "repo7", 9).unwrap();
    let mut store = ArtifactStore::new();
    let mut pr = PullRequest::open(
        9,
        "refs/heads/main",
        "refs/heads/feature",
        "psn:alice",
        false,
    );
    pr.body = Body::new(SECRET_TITLE, vec![]);
    store.put_pr(&pr_ref, pr, GateOutcome::AllRequiredGreen, 0, 0);
    let id = if granted {
        StubId::new().allow_view(&pr_ref)
    } else {
        StubId::new()
    };
    (GitRefResolver::new(Projector::new(id, store)), pr_ref)
}

#[test]
fn provider_git_resolver_returns_projection_or_tombstone() {
    let (denied_resolver, pr_ref) = resolver(false);
    match denied_resolver.resolve_display(
        &acme(),
        &region(),
        &pr_ref,
        &viewer("nobody"),
        &strong("zk-1"),
    ) {
        RefResolution::Tombstone(t) => assert_eq!(t.root, pr_ref),
        RefResolution::Projection(_) => panic!("a denied viewer must get a tombstone (5.2)"),
    }

    let (ok_resolver, pr_ref) = resolver(true);
    match ok_resolver.resolve_display(
        &acme(),
        &region(),
        &pr_ref,
        &viewer("maintainer"),
        &strong("zk-1"),
    ) {
        RefResolution::Projection(p) => assert_eq!(p.title, SECRET_TITLE),
        RefResolution::Tombstone(_) => panic!("a permitted viewer must get a projection (5.2)"),
    }
}

#[test]
fn consumer_humanise_renders_denied_git_subject_as_tombstone() {
    let (resolver, pr_ref) = resolver(false);
    let h = humanise(
        &resolver,
        &acme(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        "review_requested",
        std::slice::from_ref(&pr_ref),
        &viewer("nobody"),
        DEFAULT_LOCALE,
        &strong("zk-1"),
        Channel::Cli,
    );
    assert!(
        !h.text.contains(SECRET_TITLE),
        "0 title leak (7.3 over Git's resolver)"
    );
    assert!(
        h.text.contains("a restricted pr"),
        "the denied slot binds the PII-free tombstone"
    );
    assert!(h.links.is_empty(), "no click-route for a denied subject");
}

#[test]
fn consumer_humanise_renders_permitted_git_subject_with_title() {
    let (resolver, pr_ref) = resolver(true);
    let h = humanise(
        &resolver,
        &acme(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        "review_requested",
        std::slice::from_ref(&pr_ref),
        &viewer("maintainer"),
        DEFAULT_LOCALE,
        &strong("zk-1"),
        Channel::Cli,
    );
    assert!(
        h.text.contains(SECRET_TITLE),
        "the permitted viewer sees the title (7.3)"
    );
    assert_eq!(
        h.links,
        vec![pr_ref.0],
        "the allowed branch yields the click-route (7.3)"
    );
}

#[test]
fn review_requests_is_a_filter_over_the_one_inbox() {
    let (reason, prefix) = git_review_requests_filter();
    assert_eq!(reason, Reason::ReviewRequested);
    assert_eq!(prefix, "git/");

    let git_pr = git_pr_ref("acme", "repo7", 9).unwrap();
    assert!(
        matches_review_requests_view(Reason::ReviewRequested, &git_pr),
        "a git review-request matches"
    );
    assert!(
        !matches_review_requests_view(Reason::Mentioned, &git_pr),
        "a mention is not a review-request"
    );
    let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
    assert!(
        !matches_review_requests_view(Reason::ReviewRequested, &issue),
        "a non-git review-request is filtered out (git scoped view)"
    );
}
