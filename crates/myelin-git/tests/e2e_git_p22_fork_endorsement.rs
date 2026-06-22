//! # GIT-P22 / P-284 — the fork / trust-tier endorsement gate: the chained e2e (M3-G4)
//!
//! **The poisoned-pipeline defence (GIT-D10 parts (b) + (c)).** This is the end-to-end flow EI-01 §4
//! requires — NOT a unit: it drives the LIVE [`EndorsementResolver`] (the GIT-P22 deliverable that runs
//! the maintainer's `check(subject, approve_untrusted_ci, repo)` through the LIVE
//! [`myelin_git::live_check::GitCheckGate`]) feeding the GIT-P21 merge gate, proving:
//!
//! - **(b) fork self-green → NEUTRAL FOR GATING** — a fork PR's `untrusted_fork` CI success is recorded
//!   but the merge gate BLOCKS it (0 forks green their own required gate). The fork AUTHOR cannot
//!   self-endorse (the resolver runs the check with the author as subject → Deny → 0 endorsements).
//! - **(c) maintainer endorses → the gate FLIPS GREEN** — a maintainer who holds `approve_untrusted_ci`
//!   endorses; the resolver produces the endorsed context; the merge gate admits.
//! - the **`fork:<pr_id>` cache confinement** (11.2 C4): a fork run's cache write cannot reach the
//!   trusted cache scope (0 fork writes in the trusted scope — the poisoned-cache half).
//!
//! **Contracts:** index rows 5.9 (fork-endorsement — neutral-until-endorsed), 11.2 C4 (the
//! `fork:<pr_id>` trust-scoped cache), 4.9 (the `approve_untrusted_ci` relation). Owning architecture:
//! `git-hosting/architecture/02-internals-and-algorithms.md` §6.3. **Reconciliation:** X-1 + §8.
//!
//! The fork facts here are the SYNTHETIC `ci.check.updated` producer's (CI's real producer is EB-27/M4
//! — the seam goes end-to-end at the M4 co-gate GIT-D10 / CI-D8).

use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use myelin_git::fork_gate::{Endorser, EndorsementResolver, ScopedCache, TrustScope};
use myelin_git::live_check::GitCheckGate;
use myelin_git::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy, UnmetReason};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, EffectivePolicy, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, RewriteTrace, Result as IdResult, SubjectTree, TupleDelta, Zookie,
};
use myelin_storage::InMemoryCache;
use myelin_substrate::{FailStaticThreshold, SystemClock};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

const HEAD: &str = "deadbeefcafe";
const REPO: &str = "myelin://acme/git/repo/core";
const PR_ID: &str = "1421";

fn fact(context: &str, attempt: u32, state: CheckState, trust: TrustTier) -> CheckStatus {
    CheckStatus {
        tenant: TenantId("acme".into()),
        repo: ArtifactRef(REPO.into()),
        commit_oid: GitOid(HEAD.into()),
        context: CheckContext::ci(context),
        state,
        required: true,
        run: ArtifactRef(format!("myelin://acme/ci/run/{attempt}")),
        run_attempt: attempt,
        trust_tier: trust,
        details_ref: ArtifactRef(format!("myelin://acme/ci/run/{attempt}#step-2")),
        summary: HumanisedRef { template_key: "ci.check.updated".into(), args: BTreeMap::new() },
        started_at: Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
        cost_settled: true,
    }
}

fn principal(id: &str) -> Principal {
    let mut p =
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId("acme".into()));
    p.region = Region("fr-par".into());
    p
}

// An IdentityService stub: `approve_untrusted_ci@<subject>@repo` is Allow IFF the subject holds the
// endorsement relation (a maintainer). The fork author does not → Deny (cannot self-endorse).
struct StubId {
    endorsers: HashMap<String, Decision>,
}
impl StubId {
    fn new() -> Self {
        Self { endorsers: HashMap::new() }
    }
    fn allowing_endorser(mut self, principal_id: &str) -> Self {
        self.endorsers
            .insert(format!("approve_untrusted_ci@{principal_id}@{REPO}"), Decision::Allow);
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
            .get(&format!("{}@{}@{}", permission.0, s.principal_id.0, object.0))
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
        status: "OPEN — LEGAL".into(),
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

/// **THE CHAINED E2E — fork self-green NEUTRAL → maintainer endorses → gate FLIPS GREEN (GIT-D10 b+c).**
#[test]
fn fork_self_green_is_neutral_until_a_live_maintainer_endorsement() {
    let head = GitOid(HEAD.into());
    let repo = ArtifactRef(REPO.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
    let build = CheckContext::ci("build");

    // 1. The fork's CI self-greens `ci/build` — but the run is untrusted_fork (it ran fork code).
    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::UntrustedFork));

    // The maintainer holds approve_untrusted_ci@repo; the fork author does NOT.
    let g = gate(StubId::new().allowing_endorser("maintainer-1"));
    let resolver = EndorsementResolver::new(&g);

    // 2. (GIT-D10 b) — the FORK AUTHOR tries to merge: the resolver runs the endorsement check with the
    //    author as subject → Deny → 0 endorsements → the merge gate BLOCKS (0 forks green their gate).
    let author = principal("fork-author");
    let endorsed_by_author = resolver.resolve_endorsed(
        &policy.required,
        &proj,
        &head,
        &Endorser { subject: &author, repo: &repo, zookie: Zookie("zk".into()), subject_revoked: false },
    );
    assert!(endorsed_by_author.is_empty(), "a fork author cannot endorse their own run");
    match evaluate_merge_gate(&policy, &proj, &head, &endorsed_by_author) {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet[0].reason, UnmetReason::UntrustedForkNeutral, "fork-neutral");
        }
        MergeGateOutcome::Admitted => panic!("(b) a fork must NOT self-green its required gate"),
    }

    // 3. (GIT-D10 c) — a MAINTAINER endorses: the resolver runs the check with the maintainer as
    //    subject → Allow → produces the endorsed context → the merge gate FLIPS GREEN.
    let maintainer = principal("maintainer-1");
    let endorsed_by_maintainer = resolver.resolve_endorsed(
        &policy.required,
        &proj,
        &head,
        &Endorser { subject: &maintainer, repo: &repo, zookie: Zookie("zk".into()), subject_revoked: false },
    );
    assert_eq!(endorsed_by_maintainer, vec![build], "the maintainer endorsement resolves the context");
    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &endorsed_by_maintainer),
        MergeGateOutcome::Admitted,
        "(c) a maintainer endorsement flips the fork gate green"
    );
}

/// **THE CHAINED E2E — the re-run-trusted escape hatch (the other Δ3 path).** A maintainer re-runs the
/// context trusted (a higher-attempt trusted fact supersedes the fork fact); the gate greens with NO
/// explicit endorsement — the resolver finds the current row is already trusted (need = NotApplicable).
#[test]
fn rerun_trusted_supersedes_and_greens_with_no_endorsement() {
    let head = GitOid(HEAD.into());
    let repo = ArtifactRef(REPO.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::UntrustedFork));
    // The maintainer re-runs the context trusted (attempt 2) — supersedes the fork fact in place.
    proj.apply(&fact("build", 2, CheckState::Success, TrustTier::Trusted));

    // Nobody endorses (the StubId grants no endorsement relation) — yet the gate greens.
    let g = gate(StubId::new());
    let resolver = EndorsementResolver::new(&g);
    let anyone = principal("anyone");
    let endorsed = resolver.resolve_endorsed(
        &policy.required,
        &proj,
        &head,
        &Endorser { subject: &anyone, repo: &repo, zookie: Zookie("zk".into()), subject_revoked: false },
    );
    assert!(endorsed.is_empty(), "a trusted current row needs no endorsement");
    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &endorsed),
        MergeGateOutcome::Admitted,
        "a re-run trusted greens the gate with no explicit endorsement"
    );
}

/// **THE CACHE-CONFINEMENT HALF (11.2 C4) — 0 fork writes in the trusted scope.** A fork run's cache
/// write (a poisoned dependency graph) cannot be read by a later TRUSTED run of the same logical key.
#[test]
fn fork_cache_write_cannot_poison_the_trusted_scope() {
    let cache = InMemoryCache::new();
    let tenant = TenantId("acme".into());
    let ttl = Duration::from_secs(60);

    // The fork run derives its scope from its CI-stamped trust tier — fork:<pr_id>, NEVER trusted.
    let fork_scope = TrustScope::for_run(TrustTier::UntrustedFork, PR_ID);
    assert!(!fork_scope.is_trusted(), "a fork run is structurally never the trusted scope");
    let fork = ScopedCache::new(&cache, fork_scope);
    fork.set(&tenant, "dep-graph", b"attacker-controlled", ttl).unwrap();

    // A later TRUSTED run reads the same logical key — it MUST see a clean miss (0 fork writes reach
    // the trusted scope; the fork could not poison it).
    let trusted = ScopedCache::new(&cache, TrustScope::Trusted);
    assert_eq!(
        trusted.get(&tenant, "dep-graph").unwrap(),
        None,
        "0 fork writes in the trusted scope (the poisoned-cache defence holds)"
    );
}
