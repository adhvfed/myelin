use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use myelin_git::lifecycle::BranchProtectionRuleset;
use myelin_git::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy, UnmetReason};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::BTreeMap;

const HEAD: &str = "deadbeefcafe";

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
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args,
        },
        started_at: Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
        cost_settled: true,
    }
}

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

#[test]
fn merge_gate_blocks_until_the_required_set_is_complete() {
    let head = GitOid(HEAD.into());

    let ruleset = protected_main();
    assert!(
        ruleset.matches("refs/heads/main"),
        "the ruleset protects main"
    );
    let policy = MergeGatePolicy::from_required_contexts(&ruleset.required_contexts).unwrap();
    assert_eq!(policy.required.len(), 2);

    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));

    match evaluate_merge_gate(&policy, &proj, &head, &[]) {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet.len(), 1, "exactly the one missing context is unmet");
            assert_eq!(unmet[0].context, CheckContext::ci("test"));
            assert_eq!(unmet[0].reason, UnmetReason::Missing);
        }
        MergeGateOutcome::Admitted => panic!("the gate must BLOCK with a missing required context"),
    }

    proj.apply(&fact("test", 1, CheckState::Success, TrustTier::Trusted));

    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[]),
        MergeGateOutcome::Admitted,
        "a complete green required set admits the merge"
    );
}

#[test]
fn fork_self_green_is_neutral_until_a_maintainer_endorses() {
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact(
        "build",
        1,
        CheckState::Success,
        TrustTier::UntrustedFork,
    ));

    match evaluate_merge_gate(&policy, &proj, &head, &[]) {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet[0].reason, UnmetReason::UntrustedForkNeutral);
        }
        MergeGateOutcome::Admitted => panic!("a fork must NOT self-green its required gate"),
    }

    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[CheckContext::ci("build")]),
        MergeGateOutcome::Admitted,
        "a maintainer endorsement admits the fork success"
    );
}

#[test]
fn rerun_trusted_supersedes_fork_and_admits() {
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact(
        "build",
        1,
        CheckState::Success,
        TrustTier::UntrustedFork,
    ));
    proj.apply(&fact("build", 2, CheckState::Success, TrustTier::Trusted));

    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[]),
        MergeGateOutcome::Admitted,
        "a re-run trusted greens the gate with no explicit endorsement"
    );
}

#[test]
fn a_superseding_failure_re_blocks_a_previously_green_gate() {
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    let mut proj = CheckStatusProjection::new();
    proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[]),
        MergeGateOutcome::Admitted
    );
    proj.apply(&fact("build", 2, CheckState::Failure, TrustTier::Trusted));
    match evaluate_merge_gate(&policy, &proj, &head, &[]) {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(
                unmet[0].reason,
                UnmetReason::NotGreen {
                    state: CheckState::Failure
                }
            );
        }
        MergeGateOutcome::Admitted => panic!("a superseding failure must re-block the gate"),
    }
}
