use crate::check_status::{
    is_acceptable_satisfaction, CheckContext, CheckKey, CheckStatusProjection, CheckStatusRow,
    GitOid, TrustTier,
};
use crate::live_check::{is_allow, GitCheckGate};
use myelin_identity::{IdentityService, Principal, Zookie};
use myelin_storage::{Cache, CacheError};
use myelin_substrate::Clock;
use myelin_tenancy::{ArtifactRef, TenantId};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndorsementNeed {
    NeedsEndorsement,
    NotApplicable,
}

pub fn endorsement_need(
    projection: &CheckStatusProjection,
    head_oid: &GitOid,
    context: &CheckContext,
) -> EndorsementNeed {
    let key = CheckKey {
        commit_oid: head_oid.clone(),
        context: context.clone(),
    };
    match projection.current(&key) {
        Some(row) => endorsement_need_for_row(row),
        None => EndorsementNeed::NotApplicable,
    }
}

pub fn endorsement_need_for_row(row: &CheckStatusRow) -> EndorsementNeed {
    if row.state.is_success() && row.trust_tier == TrustTier::UntrustedFork {
        EndorsementNeed::NeedsEndorsement
    } else {
        EndorsementNeed::NotApplicable
    }
}

pub struct Endorser<'a> {
    pub subject: &'a Principal,
    pub repo: &'a ArtifactRef,
    pub zookie: Zookie,
    pub subject_revoked: bool,
}

pub struct EndorsementResolver<'g, I: IdentityService, C: Clock> {
    gate: &'g GitCheckGate<I, C>,
}

impl<'g, I: IdentityService, C: Clock> EndorsementResolver<'g, I, C> {
    pub fn new(gate: &'g GitCheckGate<I, C>) -> EndorsementResolver<'g, I, C> {
        EndorsementResolver { gate }
    }

    pub fn resolve_endorsed(
        &self,
        required: &[CheckContext],
        projection: &CheckStatusProjection,
        head_oid: &GitOid,
        endorser: &Endorser<'_>,
    ) -> Vec<CheckContext> {
        let mut endorsed = Vec::new();
        for ctx in required {
            if endorsement_need(projection, head_oid, ctx) == EndorsementNeed::NeedsEndorsement {
                let decision = self.gate.fork_endorsement_check(
                    endorser.subject,
                    endorser.repo,
                    endorser.zookie.clone(),
                    endorser.subject_revoked,
                );
                if is_allow(&decision) {
                    endorsed.push(ctx.clone());
                }
            }
        }
        endorsed
    }

    pub fn context_satisfied(
        &self,
        projection: &CheckStatusProjection,
        head_oid: &GitOid,
        context: &CheckContext,
        endorser: &Endorser<'_>,
    ) -> bool {
        let key = CheckKey {
            commit_oid: head_oid.clone(),
            context: context.clone(),
        };
        let Some(row) = projection.current(&key) else {
            return false;
        };
        let endorsed = match endorsement_need_for_row(row) {
            EndorsementNeed::NeedsEndorsement => {
                let d = self.gate.fork_endorsement_check(
                    endorser.subject,
                    endorser.repo,
                    endorser.zookie.clone(),
                    endorser.subject_revoked,
                );
                is_allow(&d)
            }
            EndorsementNeed::NotApplicable => false,
        };
        is_acceptable_satisfaction(row, endorsed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustScope {
    Trusted,
    Fork {
        pr_id: String,
    },
}

impl TrustScope {
    pub fn for_run(trust_tier: TrustTier, pr_id: &str) -> TrustScope {
        match trust_tier {
            TrustTier::Trusted => TrustScope::Trusted,
            TrustTier::UntrustedFork => TrustScope::Fork {
                pr_id: pr_id.to_string(),
            },
        }
    }

    pub fn key_prefix(&self) -> String {
        match self {
            TrustScope::Trusted => "trusted".to_string(),
            TrustScope::Fork { pr_id } => format!("fork:{pr_id}"),
        }
    }

    pub fn is_trusted(&self) -> bool {
        matches!(self, TrustScope::Trusted)
    }
}

pub struct ScopedCache<'c, K: Cache> {
    inner: &'c K,
    scope: TrustScope,
}

impl<'c, K: Cache> ScopedCache<'c, K> {
    pub fn new(inner: &'c K, scope: TrustScope) -> ScopedCache<'c, K> {
        ScopedCache { inner, scope }
    }

    pub fn scope(&self) -> &TrustScope {
        &self.scope
    }

    fn scoped_key(&self, key: &str) -> String {
        format!("{}:{}", self.scope.key_prefix(), key)
    }

    pub fn get(&self, tenant: &TenantId, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        self.inner.get(tenant, &self.scoped_key(key))
    }

    pub fn set(
        &self,
        tenant: &TenantId,
        key: &str,
        value: &[u8],
        ttl: Duration,
    ) -> Result<(), CacheError> {
        self.inner.set(tenant, &self.scoped_key(key), value, ttl)
    }

    pub fn delete(&self, tenant: &TenantId, key: &str) -> Result<(), CacheError> {
        self.inner.delete(tenant, &self.scoped_key(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_status::{CheckState, CheckStatus, HumanisedRef, Timestamp};
    use crate::live_check::GitCheckGate;
    use myelin_identity::{
        AuthzError, CaveatContext, Consistency, Credential, Decision, EffectivePolicy,
        IdentityService, ListObjectsResult, ObjectId, ObjectType, Permission, Precondition,
        Principal, PrincipalId, PrincipalKind, Result as IdResult, RewriteTrace, SubjectTree,
        TupleDelta, Zookie,
    };
    use myelin_storage::InMemoryCache;
    use myelin_substrate::{FailStaticThreshold, SystemClock};
    use myelin_tenancy::{Region, TenantId};
    use std::collections::{BTreeMap, HashMap};

    fn fact(
        commit: &str,
        ctx: CheckContext,
        attempt: u32,
        state: CheckState,
        trust: TrustTier,
    ) -> CheckStatus {
        CheckStatus {
            tenant: TenantId("acme".into()),
            repo: ArtifactRef("myelin://acme/git/repo/core".into()),
            commit_oid: GitOid(commit.into()),
            context: ctx,
            state,
            required: true,
            run: ArtifactRef("myelin://acme/ci/run/1".into()),
            run_attempt: attempt,
            trust_tier: trust,
            details_ref: ArtifactRef("myelin://acme/ci/run/1#step-3".into()),
            summary: HumanisedRef {
                template_key: "ci.check.updated".into(),
                args: BTreeMap::new(),
            },
            started_at: Timestamp("2026-06-22T00:00:00Z".into()),
            completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
            cost_settled: true,
        }
    }

    fn principal(id: &str) -> Principal {
        let mut p = Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        p.region = Region("fr-par".into());
        p
    }

    fn maintainer() -> Principal {
        principal("maintainer-1")
    }

    const REPO: &str = "myelin://acme/git/repo/core";

    struct StubId {
        endorsers: HashMap<String, Decision>,
    }
    impl StubId {
        fn new() -> Self {
            Self {
                endorsers: HashMap::new(),
            }
        }
        fn allowing_endorser(mut self, principal_id: &str, repo: &str) -> Self {
            self.endorsers.insert(
                format!("approve_untrusted_ci@{principal_id}@{repo}"),
                Decision::Allow,
            );
            self
        }
    }
    impl IdentityService for StubId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            _cav: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            Ok(self
                .endorsers
                .get(&format!(
                    "{}@{}@{}",
                    permission.0, s.principal_id.0, object.0
                ))
                .copied()
                .unwrap_or(Decision::Deny))
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
            Ok(Zookie("zk-1".into()))
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

    fn threshold() -> FailStaticThreshold {
        FailStaticThreshold {
            status: "OPEN - LEGAL".into(),
            owner: "DPO / Legal".into(),
            static_max_secs: None,
            static_max_default_secs: 300,
            agent_token_ttl_secs: 60,
            constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
        }
    }

    fn gate(id: StubId) -> GitCheckGate<StubId, SystemClock> {
        GitCheckGate::try_new(id, 300, &threshold()).expect("gate constructs")
    }

    fn zk() -> Zookie {
        Zookie("zk-merge".into())
    }

    fn repo_ref() -> ArtifactRef {
        ArtifactRef(REPO.into())
    }

    fn endorser<'a>(subject: &'a Principal, repo: &'a ArtifactRef, revoked: bool) -> Endorser<'a> {
        Endorser {
            subject,
            repo,
            zookie: zk(),
            subject_revoked: revoked,
        }
    }

    #[test]
    fn endorsement_need_only_flags_un_endorsed_fork_success() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        proj.apply(&fact(
            "h1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));
        assert_eq!(
            endorsement_need(&proj, &head, &build),
            EndorsementNeed::NeedsEndorsement
        );
        let test = CheckContext::ci("test");
        proj.apply(&fact(
            "h1",
            test.clone(),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        assert_eq!(
            endorsement_need(&proj, &head, &test),
            EndorsementNeed::NotApplicable
        );
        let lint = CheckContext::ci("lint");
        proj.apply(&fact(
            "h1",
            lint.clone(),
            1,
            CheckState::Failure,
            TrustTier::UntrustedFork,
        ));
        assert_eq!(
            endorsement_need(&proj, &head, &lint),
            EndorsementNeed::NotApplicable
        );
        assert_eq!(
            endorsement_need(&proj, &head, &CheckContext::ci("absent")),
            EndorsementNeed::NotApplicable
        );
    }

    #[test]
    fn maintainer_endorsement_resolves_the_fork_context() {
        let id = StubId::new().allowing_endorser("maintainer-1", REPO);
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        proj.apply(&fact(
            "h1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let m = maintainer();
        let repo = repo_ref();
        let endorsed = resolver.resolve_endorsed(
            std::slice::from_ref(&build),
            &proj,
            &head,
            &endorser(&m, &repo, false),
        );
        assert_eq!(
            endorsed,
            vec![build],
            "the maintainer endorsement resolves the fork context"
        );
    }

    #[test]
    fn a_non_maintainer_cannot_self_endorse_the_fork_gate() {
        let id = StubId::new();
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        proj.apply(&fact(
            "h1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let fork_author = principal("fork-author");
        let repo = repo_ref();
        let endorsed = resolver.resolve_endorsed(
            std::slice::from_ref(&build),
            &proj,
            &head,
            &endorser(&fork_author, &repo, false),
        );
        assert!(
            endorsed.is_empty(),
            "a non-maintainer cannot self-endorse the fork gate"
        );
    }

    #[test]
    fn resolver_skips_the_check_for_non_fork_contexts() {
        let id = StubId::new().allowing_endorser("maintainer-1", REPO);
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        let test = CheckContext::ci("test");
        proj.apply(&fact(
            "h1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        let m = maintainer();
        let repo = repo_ref();
        let endorsed =
            resolver.resolve_endorsed(&[build, test], &proj, &head, &endorser(&m, &repo, false));
        assert!(
            endorsed.is_empty(),
            "no fork contexts → no endorsements minted"
        );
    }

    #[test]
    fn revoked_maintainer_cannot_endorse() {
        let id = StubId::new().allowing_endorser("maintainer-1", REPO);
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        proj.apply(&fact(
            "h1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let m = maintainer();
        let repo = repo_ref();
        let endorsed = resolver.resolve_endorsed(
            std::slice::from_ref(&build),
            &proj,
            &head,
            &endorser(&m, &repo, true),
        );
        assert!(endorsed.is_empty(), "a revoked maintainer cannot endorse");
    }

    #[test]
    fn context_satisfied_composes_need_check_and_acceptable() {
        let id = StubId::new().allowing_endorser("maintainer-1", REPO);
        let g = gate(id);
        let resolver = EndorsementResolver::new(&g);

        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        let build = CheckContext::ci("build");
        proj.apply(&fact(
            "h1",
            build.clone(),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));
        let m = maintainer();
        let repo = repo_ref();
        assert!(resolver.context_satisfied(&proj, &head, &build, &endorser(&m, &repo, false)));
        assert!(!resolver.context_satisfied(
            &proj,
            &head,
            &CheckContext::ci("absent"),
            &endorser(&m, &repo, false)
        ));
    }

    #[test]
    fn trust_scope_is_derived_from_the_run_never_caller_chosen() {
        assert_eq!(
            TrustScope::for_run(TrustTier::Trusted, "42"),
            TrustScope::Trusted
        );
        let fork = TrustScope::for_run(TrustTier::UntrustedFork, "42");
        assert_eq!(fork, TrustScope::Fork { pr_id: "42".into() });
        assert!(!fork.is_trusted(), "a fork run is NEVER the trusted scope");
        assert!(
            TrustScope::Trusted.is_trusted(),
            "the trusted scope is trusted"
        );
        assert!(TrustScope::for_run(TrustTier::Trusted, "ignored").is_trusted());
        assert_eq!(fork.key_prefix(), "fork:42");
        assert_eq!(TrustScope::Trusted.key_prefix(), "trusted");
    }

    #[test]
    fn a_fork_write_cannot_reach_the_trusted_scope() {
        let cache = InMemoryCache::new();
        let tenant = TenantId("acme".into());
        let ttl = Duration::from_secs(60);

        let fork = ScopedCache::new(&cache, TrustScope::for_run(TrustTier::UntrustedFork, "42"));
        fork.set(&tenant, "dep-graph", b"poison", ttl).unwrap();

        let trusted = ScopedCache::new(&cache, TrustScope::Trusted);
        assert_eq!(
            trusted.get(&tenant, "dep-graph").unwrap(),
            None,
            "a trusted read of a fork-written key is a MISS (0 fork writes in the trusted scope)"
        );

        assert_eq!(
            fork.get(&tenant, "dep-graph").unwrap(),
            Some(b"poison".to_vec())
        );
    }

    #[test]
    fn two_forks_are_isolated_from_each_other() {
        let cache = InMemoryCache::new();
        let tenant = TenantId("acme".into());
        let ttl = Duration::from_secs(60);

        let f42 = ScopedCache::new(&cache, TrustScope::for_run(TrustTier::UntrustedFork, "42"));
        let f99 = ScopedCache::new(&cache, TrustScope::for_run(TrustTier::UntrustedFork, "99"));
        f42.set(&tenant, "k", b"v42", ttl).unwrap();
        assert_eq!(
            f99.get(&tenant, "k").unwrap(),
            None,
            "PR 99 cannot read PR 42's fork scope"
        );
        assert_eq!(f42.get(&tenant, "k").unwrap(), Some(b"v42".to_vec()));
    }

    #[test]
    fn a_trusted_write_is_visible_to_a_later_trusted_run() {
        let cache = InMemoryCache::new();
        let tenant = TenantId("acme".into());
        let ttl = Duration::from_secs(60);

        let t1 = ScopedCache::new(&cache, TrustScope::Trusted);
        t1.set(&tenant, "k", b"shared", ttl).unwrap();
        let t2 = ScopedCache::new(&cache, TrustScope::Trusted);
        assert_eq!(t2.get(&tenant, "k").unwrap(), Some(b"shared".to_vec()));
    }

    #[test]
    fn scoped_delete_invalidates_the_scoped_key() {
        let cache = InMemoryCache::new();
        let tenant = TenantId("acme".into());
        let ttl = Duration::from_secs(60);

        let fork = ScopedCache::new(&cache, TrustScope::for_run(TrustTier::UntrustedFork, "42"));
        fork.set(&tenant, "k", b"v", ttl).unwrap();
        assert_eq!(fork.get(&tenant, "k").unwrap(), Some(b"v".to_vec()));
        fork.delete(&tenant, "k").unwrap();
        assert_eq!(
            fork.get(&tenant, "k").unwrap(),
            None,
            "delete dropped the scoped key"
        );
    }
}
