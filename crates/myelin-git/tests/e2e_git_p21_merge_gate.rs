//! # GIT-P21 / P-282 — the merge gate + the required-set policy: the chained e2e (M3-G4)
//!
//! **Contract:** `contract-index.md` row 5.9 (the required-set policy — `ruleset.required_contexts`;
//! CI reports facts, Git decides which contexts gate this `base_ref`). Owning architecture:
//! `git-hosting/architecture/02-internals-and-algorithms.md` §6.2 (the merge gate — the "what is
//! allowed to land" decision) + §6.3 (the fork / trust-tier gate). **Reconciliation:** X-1 (the merge
//! gate is the consumer of Git's OWN `check_status` projection — Git never synchronously calls CI).
//!
//! ## The chained scenario (EI-01 §4 — a real end-to-end flow, not a unit)
//! 1. configure a `base_ref`'s branch-protection ruleset's `required_contexts` (`ci/build`, `ci/test`);
//! 2. project SOME of them green (the synthetic `ci.check.updated` consumer apply) — `ci/build` green,
//!    `ci/test` still MISSING;
//! 3. assert the merge gate BLOCKS (the missing required context fails the gate — 0 under-gated);
//! 4. complete the set (project `ci/test` green too);
//! 5. assert the gate is now ADMITTED.
//!
//! Plus the fork branch (Δ3, the poisoned-pipeline defence): a fork's `untrusted_fork` success is
//! NEUTRAL for gating (blocked) until a maintainer endorses — proving a fork cannot self-green its own
//! required gate. The LIVE endorsement RESOLUTION (`approve_untrusted_ci`) is GIT-P22; here the
//! endorsement is the explicit gate input, and the neutral-until-endorsed rule already holds.
//!
//! The projection here is the in-memory [`CheckStatusProjection`] (the SEMANTICS the live store
//! implements byte-for-byte); the LIVE-STACK proof of the same gate over Postgres is the
//! `integration_git_p21_merge_gate` integration test (run with --features integration).

use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use myelin_git::lifecycle::BranchProtectionRuleset;
use myelin_git::merge_gate::{
    evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy, UnmetReason,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::BTreeMap;

const HEAD: &str = "deadbeefcafe";

/// A synthetic `ci.check.updated` fact (CI's real producer is EB-27/M4) for the PR head commit.
fn fact(context: &str, attempt: u32, state: CheckState, trust: TrustTier) -> CheckStatus {
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    CheckStatus {
        tenant: TenantId("acme".into()),
        repo: ArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid(HEAD.into()),
        context: CheckContext::ci(context),
        state,
        required: true,
        run: ArtifactRef(format!("myelin://acme/ci/run/{attempt}")),
        run_attempt: attempt,
        trust_tier: trust,
        details_ref: ArtifactRef(format!("myelin://acme/ci/run/{attempt}#step-2")),
        summary: HumanisedRef { template_key: "ci.check.updated".into(), args },
        started_at: Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
        cost_settled: true,
    }
}

/// A protected `base_ref` ruleset requiring `ci/build` + `ci/test` (the per-base_ref policy — Git
/// decides which contexts gate).
fn protected_main() -> BranchProtectionRuleset {
    BranchProtectionRuleset {
        ref_pattern: "refs/heads/main".into(),
        required_contexts: vec!["ci/build".into(), "ci/test".into()],
        required_approvals: 0,
        require_codeowner_review: false,
        require_conversation_resolution: false,
        allow_force_push: false,
    }
}

/// **THE CHAINED E2E — configure → partial-green → BLOCK → complete → ADMIT (0 under-gated merges).**
#[test]
fn merge_gate_blocks_until_the_required_set_is_complete() {
    let head = GitOid(HEAD.into());

    // 1. configure the base_ref's required-set policy from the protected-ref ruleset (Git decides which
    //    contexts gate). The ruleset NAMES `ci/build` + `ci/test`.
    let ruleset = protected_main();
    assert!(ruleset.matches("refs/heads/main"), "the ruleset protects main");
    let policy = MergeGatePolicy::from_required_contexts(&ruleset.required_contexts).unwrap();
    assert_eq!(policy.required.len(), 2);

    // 2. project SOME green — `ci/build` succeeds (trusted); `ci/test` is still MISSING (CI hasn't
    //    reported it for the head yet).
    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));

    // 3. the gate BLOCKS — the missing required `ci/test` fails the gate (0 under-gated merges: a
    //    merge is NOT admitted with a missing required context).
    match evaluate_merge_gate(&policy, &proj, &head, &[]) {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet.len(), 1, "exactly the one missing context is unmet");
            assert_eq!(unmet[0].context, CheckContext::ci("test"));
            assert_eq!(unmet[0].reason, UnmetReason::Missing);
        }
        MergeGateOutcome::Admitted => panic!("the gate must BLOCK with a missing required context"),
    }

    // 4. complete the set — CI reports `ci/test` green (trusted).
    proj.apply(&fact("test", 1, CheckState::Success, TrustTier::Trusted));

    // 5. the gate is now ADMITTED — every required context is green-and-current with an acceptable
    //    trust posture.
    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[]),
        MergeGateOutcome::Admitted,
        "a complete green required set admits the merge"
    );
}

/// **THE CHAINED E2E — the fork branch (Δ3): fork self-green is NEUTRAL until endorsed.** A fork PR's
/// `untrusted_fork` success cannot self-satisfy the required gate (the poisoned-pipeline defence); a
/// maintainer endorsement flips it green. (The LIVE `approve_untrusted_ci` resolution is GIT-P22.)
#[test]
fn fork_self_green_is_neutral_until_a_maintainer_endorses() {
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    // The fork's CI self-greens `ci/build` — but the run is untrusted_fork (it executed fork code).
    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::UntrustedFork));

    // The gate BLOCKS — a fork success is NEUTRAL for gating (0 forks green their own required gate).
    match evaluate_merge_gate(&policy, &proj, &head, &[]) {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet[0].reason, UnmetReason::UntrustedForkNeutral);
        }
        MergeGateOutcome::Admitted => panic!("a fork must NOT self-green its required gate"),
    }

    // A maintainer ENDORSES the context (the GIT-P22 approve_untrusted_ci input) → the gate flips green.
    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[CheckContext::ci("build")]),
        MergeGateOutcome::Admitted,
        "a maintainer endorsement admits the fork success"
    );
}

/// **A re-run under trust_tier = trusted flips the gate via supersession** — the other Δ3 escape hatch
/// ("approve and run"): a higher-attempt trusted fact supersedes the fork fact, so the gate greens with
/// no explicit endorsement (the merge gate reads the current row, which is now trusted).
#[test]
fn rerun_trusted_supersedes_fork_and_admits() {
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::UntrustedFork));
    // The maintainer re-runs the context trusted (attempt 2) — supersedes the fork fact in place.
    proj.apply(&fact("build", 2, CheckState::Success, TrustTier::Trusted));

    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[]),
        MergeGateOutcome::Admitted,
        "a re-run trusted greens the gate with no explicit endorsement"
    );
}

/// **A stale required context BLOCKS** — a required context whose CURRENT row is a re-run FAILURE (the
/// supersession brought a newer failing attempt) blocks the gate even though an earlier attempt was
/// green. The gate reads the CURRENT row only.
#[test]
fn a_superseding_failure_re_blocks_a_previously_green_gate() {
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    let mut proj = CheckStatusProjection::new();
    // attempt 1 green...
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
    assert_eq!(evaluate_merge_gate(&policy, &proj, &head, &[]), MergeGateOutcome::Admitted);
    // ...then a re-run (attempt 2) FAILS — supersedes → the current row is now a failure → re-blocked.
    proj.apply(&fact("build", 2, CheckState::Failure, TrustTier::Trusted));
    match evaluate_merge_gate(&policy, &proj, &head, &[]) {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet[0].reason, UnmetReason::NotGreen { state: CheckState::Failure });
        }
        MergeGateOutcome::Admitted => panic!("a superseding failure must re-block the gate"),
    }
}
