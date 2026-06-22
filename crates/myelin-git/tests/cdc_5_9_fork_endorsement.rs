//! # The CDC pair for contract 5.9 — the **fork-endorsement** half (GIT-P22 / P-284)
//!
//! **Contract:** `contract-index.md` row 5.9 (the Git↔CI `CheckStatus` seam — *an `untrusted_fork`
//! success is **neutral for gating** until endorsed via `check(subject, approve_untrusted_ci, repo)` or
//! re-run trusted*) + row 4.9 (the `approve_untrusted_ci` relation). Owning architecture:
//! `git-hosting/architecture/02-internals-and-algorithms.md` §6.3 (the fork / trust-tier gate).
//! **Reconciliation:** X-1 (the seam) + §8 (the trust-scoped cache).
//!
//! ## The seam this pair pins (CI STAMPS the trust tier; Git OWNS the endorsement decision)
//! `cdc_5_9_merge_gate_required_set.rs` (GIT-P21) pinned the required-set-POLICY half. THIS pair pins
//! the **fork-endorsement half**:
//!
//! - **PRODUCER (CI):** stamps `trust_tier` on the fact from the run's provenance + the
//!   `read & !is_untrusted_fork` ABAC edge. A fork PR run is `untrusted_fork`. CI does NOT decide
//!   gating — it reports the tier.
//! - **CONSUMER + GATE (Git):** reads `trust_tier` OFF the fact (never recomputes it), treats an
//!   un-endorsed `untrusted_fork` success as NEUTRAL, and flips it green ONLY via a maintainer
//!   `check(subject, approve_untrusted_ci, repo)` Allow — an ordinary RELATION check, never bespoke
//!   trust logic.
//!
//! The load-bearing CDC assertion: **Git reads the tier off the fact (it does not recompute trust); the
//! endorsement is a plain `approve_untrusted_ci` check whose subject is a MAINTAINER (the fork author
//! cannot self-endorse).**

use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use myelin_git::fork_gate::{endorsement_need, EndorsementNeed, EndorsementResolver, Endorser};
use myelin_git::live_check::GitCheckGate;
use myelin_git::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, EffectivePolicy, IdentityService,
    ListObjectsResult, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta, Zookie,
};
use myelin_substrate::{FailStaticThreshold, SystemClock};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::collections::{BTreeMap, HashMap};

const HEAD: &str = "c0ffee";
const REPO: &str = "myelin://acme/git/repo/core";

/// **PRODUCER side of 5.9** — CI stamps `trust_tier` on the fact. A fork run is `untrusted_fork`.
fn producer_fact(context: &str, state: CheckState, trust: TrustTier) -> CheckStatus {
    CheckStatus {
        tenant: TenantId("acme".into()),
        repo: ArtifactRef(REPO.into()),
        commit_oid: GitOid(HEAD.into()),
        context: CheckContext::ci(context),
        state,
        required: true,
        run: ArtifactRef("myelin://acme/ci/run/7".into()),
        run_attempt: 1,
        trust_tier: trust, // CI's STAMP — Git reads it off, never recomputes.
        details_ref: ArtifactRef("myelin://acme/ci/run/7#step-1".into()),
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

struct StubId {
    endorsers: HashMap<String, Decision>,
}
impl StubId {
    fn with_endorser(principal_id: &str) -> Self {
        let mut endorsers = HashMap::new();
        endorsers.insert(
            format!("approve_untrusted_ci@{principal_id}@{REPO}"),
            Decision::Allow,
        );
        Self { endorsers }
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
        // The endorsement is a PLAIN relation check — Git asks Id "does this subject hold
        // approve_untrusted_ci on the repo", never CI "is this run trusted" (no recompute).
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

/// **CDC: CI stamps `untrusted_fork`; Git treats it as NEUTRAL until a maintainer endorses.** The
/// producer-side stamp (a fork run) decodes to exactly Git's consumer view; Git's gate then reads the
/// tier off the fact and blocks the un-endorsed fork success.
#[test]
fn ci_stamps_fork_tier_git_treats_it_neutral_until_endorsed() {
    let head = GitOid(HEAD.into());
    let repo = ArtifactRef(REPO.into());
    let build = CheckContext::ci("build");
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    // PRODUCER: CI emits a fork success carried OPAQUE; the consumer decodes exactly Git's view.
    let fork_fact = producer_fact("build", CheckState::Success, TrustTier::UntrustedFork);
    let opaque: serde_json::Value = serde_json::to_value(&fork_fact).unwrap();
    let decoded: CheckStatus = serde_json::from_value(opaque).unwrap();
    assert_eq!(
        decoded.trust_tier,
        TrustTier::UntrustedFork,
        "CI's tier stamp survives the seam"
    );

    let mut proj = CheckStatusProjection::new();
    proj.apply(&decoded);

    // Git reads the tier off the fact — the context NEEDS an endorsement decision (un-endorsed fork).
    assert_eq!(
        endorsement_need(&proj, &head, &build),
        EndorsementNeed::NeedsEndorsement,
        "Git reads the tier off the fact (an un-endorsed fork success needs endorsement)"
    );

    // CONSUMER + GATE: with no endorsement, the gate blocks (neutral-for-gating).
    assert!(matches!(
        evaluate_merge_gate(&policy, &proj, &head, &[]),
        MergeGateOutcome::Blocked { .. }
    ));

    // The endorsement is a PLAIN approve_untrusted_ci check whose subject is a MAINTAINER.
    let g = gate(StubId::with_endorser("maintainer-1"));
    let resolver = EndorsementResolver::new(&g);
    let m = principal("maintainer-1");
    let endorsed = resolver.resolve_endorsed(
        &policy.required,
        &proj,
        &head,
        &Endorser {
            subject: &m,
            repo: &repo,
            zookie: Zookie("zk".into()),
            subject_revoked: false,
        },
    );
    assert_eq!(
        endorsed,
        vec![build],
        "a maintainer's plain approve_untrusted_ci check endorses"
    );
    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &endorsed),
        MergeGateOutcome::Admitted,
        "the endorsement flips the fork gate green"
    );
}

/// **CDC: the fork author cannot self-endorse (0 forks green their own gate).** The endorsement check's
/// subject is the actor; a non-maintainer (the fork author) is Denied → 0 endorsements → the gate keeps
/// blocking. This is the poisoned-pipeline non-negotiability (EI-01 §2).
#[test]
fn the_fork_author_cannot_self_endorse() {
    let head = GitOid(HEAD.into());
    let repo = ArtifactRef(REPO.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    let mut proj = CheckStatusProjection::new();
    proj.apply(&producer_fact(
        "build",
        CheckState::Success,
        TrustTier::UntrustedFork,
    ));

    // Only `maintainer-1` holds the relation; the fork author does not.
    let g = gate(StubId::with_endorser("maintainer-1"));
    let resolver = EndorsementResolver::new(&g);
    let author = principal("fork-author");
    let endorsed = resolver.resolve_endorsed(
        &policy.required,
        &proj,
        &head,
        &Endorser {
            subject: &author,
            repo: &repo,
            zookie: Zookie("zk".into()),
            subject_revoked: false,
        },
    );
    assert!(
        endorsed.is_empty(),
        "the fork author cannot self-endorse (0 forks green their gate)"
    );
    assert!(matches!(
        evaluate_merge_gate(&policy, &proj, &head, &endorsed),
        MergeGateOutcome::Blocked { .. }
    ));
}
