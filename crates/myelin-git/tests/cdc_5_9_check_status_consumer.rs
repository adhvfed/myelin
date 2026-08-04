use myelin_git::check_status::{
    gate_outcome, supersedes, ApplyOutcome, CheckContext, CheckState, CheckStatus,
    CheckStatusProjection, GateOutcome, GitOid, HumanisedRef, RequiredSetPolicy, Timestamp,
    TrustTier, CHECK_STATUS_PROJECTION_DDL,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::BTreeMap;

fn producer_opaque_payload(
    commit: &str,
    provider_name: &str,
    attempt: u32,
    state: CheckState,
    trust: TrustTier,
) -> serde_json::Value {
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), provider_name.to_string());
    let fact = CheckStatus {
        tenant: TenantId("acme".into()),
        repo: ArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid(commit.into()),
        context: CheckContext::ci(provider_name),
        state,
        required: true,
        run: ArtifactRef("myelin://acme/ci/run/9".into()),
        run_attempt: attempt,
        trust_tier: trust,
        details_ref: ArtifactRef("myelin://acme/ci/run/9#step-2".into()),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args,
        },
        started_at: Timestamp("2026-06-21T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-21T00:01:00Z".into())),
        cost_settled: true,
    };
    serde_json::to_value(&fact).expect("the 5.9 fact serialises to the opaque Bus payload")
}

fn consumer_decode(opaque: serde_json::Value) -> CheckStatus {
    serde_json::from_value(opaque).expect("the opaque Bus payload decodes to Git's consumer view")
}

#[test]
fn cdc_5_9_producer_opaque_payload_decodes_to_consumer_view() {
    let opaque = producer_opaque_payload(
        "abc123",
        "build",
        1,
        CheckState::Success,
        TrustTier::Trusted,
    );
    assert_eq!(opaque["commit_oid"], "abc123");
    assert_eq!(opaque["context"]["name"], "build");
    assert_eq!(opaque["state"], "success");
    assert_eq!(opaque["trust_tier"], "trusted");
    assert_eq!(opaque["run_attempt"], 1);

    let fact = consumer_decode(opaque);
    assert_eq!(fact.commit_oid, GitOid("abc123".into()));
    assert_eq!(fact.context, CheckContext::ci("build"));
    assert_eq!(fact.state, CheckState::Success);
    assert_eq!(fact.trust_tier, TrustTier::Trusted);
}

#[test]
fn cdc_5_9_consumer_supersedes_and_gates() {
    let mut proj = CheckStatusProjection::new();
    let build = CheckContext::ci("build");

    let a1 = consumer_decode(producer_opaque_payload(
        "c1",
        "build",
        1,
        CheckState::Failure,
        TrustTier::Trusted,
    ));
    assert_eq!(
        proj.apply(&a1),
        ApplyOutcome::Superseded { current_attempt: 1 }
    );

    let a2 = consumer_decode(producer_opaque_payload(
        "c1",
        "build",
        2,
        CheckState::Success,
        TrustTier::Trusted,
    ));
    assert_eq!(
        proj.apply(&a2),
        ApplyOutcome::Superseded { current_attempt: 2 }
    );

    assert_eq!(
        proj.apply(&a1),
        ApplyOutcome::DroppedStale {
            incoming_attempt: 1,
            current_attempt: 2
        },
        "a late lower attempt is dropped (supersession is monotonic, X-1)"
    );

    let policy = RequiredSetPolicy::requiring(vec![build]);
    assert_eq!(
        gate_outcome(&policy, &proj, &GitOid("c1".into()), &[]),
        GateOutcome::AllRequiredGreen
    );
}

#[test]
fn cdc_5_9_frozen_shape_surfaces() {
    assert!(supersedes(2, 1));
    assert!(supersedes(1, 1));
    assert!(!supersedes(1, 2));

    assert!(CHECK_STATUS_PROJECTION_DDL
        .contains("tenant_id, region, repo_ref, commit_oid, context_provider, context_name"));
    assert!(CHECK_STATUS_PROJECTION_DDL.contains("run_attempt"));

    assert_ne!(CheckContext::ci("build"), CheckContext::external("build"));
}
