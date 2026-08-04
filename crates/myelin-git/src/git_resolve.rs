use myelin_identity::{Consistency, IdentityService, Principal, Zookie};
use myelin_notif::humanise::{
    RefProjection, RefResolution, RefResolvePort, Tombstone, TombstoneReason,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::project::{Projected, Projector, TombstoneReason as GitTombstoneReason};

pub const GIT_REVIEW_REQUESTS_FILTER_REASON: myelin_notif::Reason =
    myelin_notif::Reason::ReviewRequested;

pub const GIT_SUBJECT_PREFIX: &str = "git/";

pub fn git_review_requests_filter() -> (myelin_notif::Reason, &'static str) {
    (GIT_REVIEW_REQUESTS_FILTER_REASON, GIT_SUBJECT_PREFIX)
}

pub fn matches_review_requests_view(reason: myelin_notif::Reason, subject: &ArtifactRef) -> bool {
    reason == GIT_REVIEW_REQUESTS_FILTER_REASON && is_git_subject(subject)
}

fn is_git_subject(r: &ArtifactRef) -> bool {
    r.0.strip_prefix("myelin://")
        .and_then(|rest| rest.split('/').nth(1))
        .map(|subsystem| subsystem == "git")
        .unwrap_or(false)
}

pub struct GitRefResolver<I: IdentityService> {
    projector: Projector<I>,
}

impl<I: IdentityService> GitRefResolver<I> {
    pub fn new(projector: Projector<I>) -> GitRefResolver<I> {
        GitRefResolver { projector }
    }

    pub fn projector_mut(&mut self) -> &mut Projector<I> {
        &mut self.projector
    }

    fn to_resolution(reference: &ArtifactRef, projected: Projected) -> RefResolution {
        match projected {
            Projected::Visible(p) => RefResolution::Projection(RefProjection {
                ref_: reference.clone(),
                title: p.title,
                icon: p.icon,
            }),
            Projected::Tombstoned(t) => RefResolution::Tombstone(Tombstone {
                root: reference.clone(),
                reason: map_tombstone_reason(t.reason),
            }),
        }
    }
}

fn map_tombstone_reason(reason: GitTombstoneReason) -> TombstoneReason {
    match reason {
        GitTombstoneReason::Unauthorized => TombstoneReason::Denied,
        GitTombstoneReason::Restricted => TombstoneReason::Denied,
        GitTombstoneReason::Erased => TombstoneReason::Erased,
        GitTombstoneReason::ContentGone => TombstoneReason::SubGone,
    }
}

impl<I: IdentityService + Send + Sync> RefResolvePort for GitRefResolver<I> {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> RefResolution {
        let zookie: Zookie = at.at_least.clone();
        match self.projector.project(ref_, viewer, zookie) {
            Ok(projected) => Self::to_resolution(ref_, projected),
            Err(_) => RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::RootGone,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::check_status::GateOutcome;
    use crate::lifecycle::PullRequest;
    use crate::project::{git_pr_ref, ArtifactStore};
    use myelin_identity::{
        AuthzError, CaveatContext, ConsistencyMode, Credential, Decision, ListObjectsResult,
        ObjectId, ObjectType, Permission, PrincipalId, PrincipalKind, Result as IdResult,
        RewriteTrace, SubjectTree, TupleDelta,
    };
    use myelin_notif::humanise::{humanise, Channel, TemplateStore, DEFAULT_LOCALE};
    use myelin_notif::reason_template_key;
    use myelin_notif::Reason;
    use std::collections::HashSet;

    struct StubId {
        allow: HashSet<String>,
    }
    impl StubId {
        fn new() -> Self {
            Self {
                allow: HashSet::new(),
            }
        }
        fn allow_view(mut self, object: &ArtifactRef) -> Self {
            self.allow.insert(format!("view@{}", object.0));
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
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            _caveat: Option<&CaveatContext>,
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

    const SECRET_TITLE: &str = "rotate the PROJECT-NIGHTFALL signing key before the acquisition";

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
    fn secret_pr() -> PullRequest {
        let mut pr = PullRequest::open(
            9,
            "refs/heads/main",
            "refs/heads/feature",
            "psn:alice",
            false,
        );
        pr.body = Body::new(SECRET_TITLE, vec![]);
        pr
    }

    fn private_pr_resolver(grant_view: bool) -> (GitRefResolver<StubId>, ArtifactRef) {
        let pr_ref = git_pr_ref("acme", "repo7", 9);
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, secret_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        let id = if grant_view {
            StubId::new().allow_view(&pr_ref)
        } else {
            StubId::new()
        };
        (GitRefResolver::new(Projector::new(id, store)), pr_ref)
    }

    #[test]
    fn denied_viewer_resolves_to_a_tombstone_no_title() {
        let (resolver, pr_ref) = private_pr_resolver(false);
        let r = resolver.resolve_display(
            &acme(),
            &region(),
            &pr_ref,
            &viewer("ex-contractor"),
            &strong("zk-1"),
        );
        match r {
            RefResolution::Tombstone(t) => {
                assert_eq!(
                    t.root, pr_ref,
                    "the opaque root crosses (for `a restricted pr`)"
                );
                assert_eq!(t.reason, TombstoneReason::Denied);
            }
            RefResolution::Projection(_) => {
                panic!("a denied viewer must NOT get a projection (leak!)")
            }
        }
    }

    #[test]
    fn permitted_viewer_resolves_to_a_projection_with_the_title() {
        let (resolver, pr_ref) = private_pr_resolver(true);
        let r = resolver.resolve_display(
            &acme(),
            &region(),
            &pr_ref,
            &viewer("maintainer"),
            &strong("zk-1"),
        );
        match r {
            RefResolution::Projection(p) => {
                assert_eq!(p.ref_, pr_ref);
                assert_eq!(
                    p.title, SECRET_TITLE,
                    "the permitted viewer sees the PR title"
                );
                assert_eq!(p.icon, "pr");
            }
            RefResolution::Tombstone(_) => panic!("the permitted viewer must see the projection"),
        }
    }

    #[test]
    fn notif_d4_zero_title_leak_through_humanise() {
        let (resolver, pr_ref) = private_pr_resolver(false);
        let templates = TemplateStore::with_platform_defaults();
        let reasons = [Reason::ReviewRequested, Reason::Mentioned, Reason::Watched];
        let mut renders = 0u64;
        let mut leaks = 0u64;
        for &reason in &reasons {
            let key = reason_template_key(reason);
            for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
                let h = humanise(
                    &resolver,
                    &acme(),
                    &region(),
                    &templates,
                    key,
                    std::slice::from_ref(&pr_ref),
                    &viewer("ex-contractor"),
                    DEFAULT_LOCALE,
                    &strong("zk-1"),
                    channel,
                );
                renders += 1;
                if h.text.contains(SECRET_TITLE) || h.text.to_lowercase().contains("nightfall") {
                    leaks += 1;
                }
                assert!(
                    h.text.contains("a restricted pr"),
                    "the denied render is a tombstone"
                );
                assert!(
                    h.links.is_empty(),
                    "a denied unfurl yields no click-route link"
                );
            }
        }
        assert_eq!(
            leaks, 0,
            "NOTIF-D4-class: 0 title leak over {renders} denied renders (threshold 0)"
        );
    }

    #[test]
    fn tombstone_reason_mapping_is_total_and_pii_free() {
        assert_eq!(
            map_tombstone_reason(GitTombstoneReason::Unauthorized),
            TombstoneReason::Denied
        );
        assert_eq!(
            map_tombstone_reason(GitTombstoneReason::Restricted),
            TombstoneReason::Denied
        );
        assert_eq!(
            map_tombstone_reason(GitTombstoneReason::Erased),
            TombstoneReason::Erased
        );
        assert_eq!(
            map_tombstone_reason(GitTombstoneReason::ContentGone),
            TombstoneReason::SubGone
        );
    }

    #[test]
    fn an_unprojectable_ref_degrades_to_a_non_leaking_tombstone() {
        let (resolver, _) = private_pr_resolver(false);
        let not_git = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let r = resolver.resolve_display(
            &acme(),
            &region(),
            &not_git,
            &viewer("alice"),
            &strong("zk-1"),
        );
        match r {
            RefResolution::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::RootGone),
            RefResolution::Projection(_) => panic!("a non-git ref must not project a title"),
        }
    }

    #[test]
    fn review_requests_is_a_filter_over_the_one_inbox() {
        let (reason, prefix) = git_review_requests_filter();
        assert_eq!(reason, Reason::ReviewRequested);
        assert_eq!(prefix, "git/");

        let git_pr = git_pr_ref("acme", "repo7", 9);
        assert!(matches_review_requests_view(
            Reason::ReviewRequested,
            &git_pr
        ));
        assert!(!matches_review_requests_view(Reason::Mentioned, &git_pr));
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        assert!(!matches_review_requests_view(
            Reason::ReviewRequested,
            &issue
        ));
    }
}
