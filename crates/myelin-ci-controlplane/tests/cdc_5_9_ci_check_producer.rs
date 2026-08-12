use myelin_ci_controlplane::{
    check_status_payload, CheckEmitContext, CheckProvider, CheckState, CheckStatusUpdate, TrustTier,
};
use myelin_git::check_status::{
    CheckProvider as GitProvider, CheckState as GitState, CheckStatusConsumer, TrustTier as GitTier,
};

const REPO: &str = "myelin://acme/git/repo/core";
const COMMIT: &str = "deadbeefcafe";

fn emit_ctx(attempt: u32, tier: TrustTier) -> CheckEmitContext {
    CheckEmitContext {
        tenant: "acme".into(),
        repo: REPO.into(),
        commit_oid: COMMIT.into(),
        run_ref: "myelin://acme/ci/run/run-42".into(),
        run_attempt: attempt,
        trust_tier: tier,
        started_at: "2026-06-23T00:00:00Z".into(),
        completed_at: Some("2026-06-23T00:01:30Z".into()),
    }
}

#[test]
fn ci_producer_payload_decodes_into_git_frozen_checkstatus() {
    let ctx = emit_ctx(2, TrustTier::Trusted);
    let payload = check_status_payload(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "build", CheckState::Success).settled(),
    );

    let fact = CheckStatusConsumer::decode(&payload)
        .expect("CI's producer payload decodes into Git's frozen 5.9 CheckStatus (no drift)");

    assert_eq!(fact.tenant.0, "acme", "the partition key");
    assert_eq!(fact.repo.0, REPO);
    assert_eq!(fact.commit_oid.0, COMMIT, "the seam key half");
    assert_eq!(fact.context.provider, GitProvider::Ci);
    assert_eq!(fact.context.name, "build", "the other key half");
    assert_eq!(fact.state, GitState::Success);
    assert!(fact.required, "CI reports required; Git decides the gate");
    assert_eq!(fact.run.0, "myelin://acme/ci/run/run-42");
    assert_eq!(fact.run_attempt, 2, "the monotonic supersession key");
    assert_eq!(
        fact.trust_tier,
        GitTier::Trusted,
        "the tier stamped by CI, read by Git (never recomputed)"
    );
    assert_eq!(
        fact.details_ref.0, "myelin://acme/ci/run/run-42",
        "a success anchors on the canonical run root"
    );
    assert_eq!(
        fact.summary.template_key, "ci.check.success",
        "the summary is a HumanisedRef (template_key, args) - never a raw string (7.3)"
    );
    assert_eq!(
        fact.summary.args.get("context").map(String::as_str),
        Some("build"),
        "the args carry the PII-free context name"
    );
    assert!(
        fact.cost_settled,
        "settled → cost_settled true (the check is final, X-1)"
    );
}

#[test]
fn ci_producer_stamps_untrusted_fork_git_decodes_it_faithfully() {
    let ctx = emit_ctx(1, TrustTier::UntrustedFork);
    let payload = check_status_payload(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "test", CheckState::Success).settled(),
    );
    let fact = CheckStatusConsumer::decode(&payload).expect("decodes");
    assert_eq!(
        fact.state,
        GitState::Success,
        "the fork's success is recorded"
    );
    assert_eq!(
        fact.trust_tier,
        GitTier::UntrustedFork,
        "stamped from provenance - CI never endorses a fork (X-1)"
    );
}

#[test]
fn ci_producer_failure_fact_decodes_with_step_anchor_and_unsettled() {
    let ctx = emit_ctx(3, TrustTier::Trusted);
    let payload = check_status_payload(
        &ctx,
        &CheckStatusUpdate::required(CheckProvider::Ci, "build", CheckState::Failure)
            .failed_at_step(7),
    );
    let fact = CheckStatusConsumer::decode(&payload).expect("decodes");
    assert_eq!(fact.state, GitState::Failure);
    assert_eq!(
        fact.details_ref.0, "myelin://acme/ci/run/run-42#step-7",
        "the #step-<n> jump-to-failure (OQ-D / 5.7)"
    );
    assert_eq!(fact.summary.template_key, "ci.check.failure");
    assert!(
        !fact.cost_settled,
        "terminal failure but NOT settled until the reserve/settle bookend closes (X-1)"
    );
}

#[test]
fn an_incomplete_payload_fails_the_git_decode() {
    let loose = serde_json::json!({
        "state": "success",
        "run_attempt": 1,
        "trust_tier": "trusted",
    });
    assert!(
        CheckStatusConsumer::decode(&loose).is_err(),
        "an incomplete payload is a LOUD decode failure (the seam shape is a real gate)"
    );
}
