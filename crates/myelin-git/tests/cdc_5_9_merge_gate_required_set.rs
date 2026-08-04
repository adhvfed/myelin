use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use myelin_git::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy, UnmetReason};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::BTreeMap;

const HEAD: &str = "c0ffee";

fn producer_fact(
    context: &str,
    state: CheckState,
    ci_required: bool,
    trust: TrustTier,
) -> CheckStatus {
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    CheckStatus {
        tenant: TenantId("acme".into()),
        repo: ArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid(HEAD.into()),
        context: CheckContext::ci(context),
        state,
        required: ci_required,
        run: ArtifactRef("myelin://acme/ci/run/7".into()),
        run_attempt: 1,
        trust_tier: trust,
        details_ref: ArtifactRef("myelin://acme/ci/run/7#step-1".into()),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args,
        },
        started_at: Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
        cost_settled: true,
    }
}

fn consumer_apply(proj: &mut CheckStatusProjection, fact: &CheckStatus) {
    let opaque: serde_json::Value = serde_json::to_value(fact).unwrap();
    let decoded: CheckStatus = serde_json::from_value(opaque).unwrap();
    assert_eq!(
        &decoded, fact,
        "the opaque Bus payload decodes to exactly Git's CheckStatus"
    );
    proj.apply(&decoded);
}

#[test]
fn cdc_5_9_git_required_set_policy_overrides_the_ci_required_bool() {
    let head = GitOid(HEAD.into());
    let mut proj = CheckStatusProjection::new();

    consumer_apply(
        &mut proj,
        &producer_fact("build", CheckState::Success, true, TrustTier::Trusted),
    );
    consumer_apply(
        &mut proj,
        &producer_fact("lint", CheckState::Failure, false, TrustTier::Trusted),
    );

    let policy = MergeGatePolicy::from_required_contexts(&["ci/lint"]).unwrap();

    match evaluate_merge_gate(&policy, &proj, &head, &[]) {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet[0].context, CheckContext::ci("lint"));
            assert_eq!(
                unmet[0].reason,
                UnmetReason::NotGreen {
                    state: CheckState::Failure
                }
            );
        }
        MergeGateOutcome::Admitted => {
            panic!("Git's policy names lint → the failing lint must block (CI's required bool is advisory)")
        }
    }

    let build_only = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
    assert_eq!(
        evaluate_merge_gate(&build_only, &proj, &head, &[]),
        MergeGateOutcome::Admitted,
        "a context Git's policy does not name does not gate, regardless of CI's required bool"
    );
}

#[test]
fn cdc_5_9_untrusted_fork_success_is_neutral_until_endorsed() {
    let head = GitOid(HEAD.into());
    let mut proj = CheckStatusProjection::new();
    consumer_apply(
        &mut proj,
        &producer_fact("build", CheckState::Success, true, TrustTier::UntrustedFork),
    );

    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    assert!(matches!(
        evaluate_merge_gate(&policy, &proj, &head, &[]),
        MergeGateOutcome::Blocked { .. }
    ));
    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[CheckContext::ci("build")]),
        MergeGateOutcome::Admitted
    );
}

#[test]
fn cdc_5_9_zero_under_gated_merges_every_non_green_posture_blocks() {
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/x"]).unwrap();

    let empty = CheckStatusProjection::new();
    assert!(matches!(
        evaluate_merge_gate(&policy, &empty, &head, &[]),
        MergeGateOutcome::Blocked { .. }
    ));

    for state in [
        CheckState::Failure,
        CheckState::Error,
        CheckState::Cancelled,
        CheckState::Neutral,
        CheckState::Queued,
        CheckState::InProgress,
    ] {
        let mut proj = CheckStatusProjection::new();
        consumer_apply(
            &mut proj,
            &producer_fact("x", state, true, TrustTier::Trusted),
        );
        assert!(
            matches!(
                evaluate_merge_gate(&policy, &proj, &head, &[]),
                MergeGateOutcome::Blocked { .. }
            ),
            "state {state:?} must block (only a success admits)"
        );
    }

    let mut proj = CheckStatusProjection::new();
    consumer_apply(
        &mut proj,
        &producer_fact("x", CheckState::Success, true, TrustTier::Trusted),
    );
    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[]),
        MergeGateOutcome::Admitted
    );
}
