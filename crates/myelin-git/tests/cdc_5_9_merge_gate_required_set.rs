//! # The CDC pair for contract 5.9 — the **required-set policy** half (the merge gate, GIT-P21/P-282)
//!
//! **Contract:** `contract-index.md` row 5.9 (the Git↔CI `CheckStatus` seam — the required-set policy:
//! `ruleset.required_contexts`; **CI reports facts, Git decides which contexts gate this `base_ref`**).
//! Owning architecture: `git-hosting/architecture/02-internals-and-algorithms.md` §6.2 (the merge gate)
//! + §6.3 (the fork / trust-tier gate). **Reconciliation:** X-1.
//!
//! ## The seam this pair pins (CI produces the fact; Git owns the required-set policy + the gate)
//! `cdc_5_9_check_status_consumer.rs` (GIT-P6/P-232) pinned the CONSUMER decode + supersession half.
//! THIS pair pins the **required-set-POLICY half** Git owns:
//!
//! - **PRODUCER (CI):** emits a `CheckStatus` fact per `(commit_oid, context)` with a `required` bool —
//!   CI's REPORT of which contexts IT thinks gate. The bool is ADVISORY (Δ1).
//! - **CONSUMER + GATE (Git):** the AUTHORITY on which contexts gate is Git's OWN `required`-set policy
//!   (the branch-protection ruleset's `required_contexts`), NOT the CI `required` bool. The gate reads
//!   the projection + the policy → admit/block.
//!
//! The load-bearing CDC assertion: **Git's required-set policy — not CI's `required` bool — decides the
//! gate.** A context CI marks `required = false` STILL gates if Git's ruleset names it; a context CI
//! marks `required = true` does NOT gate unless Git's ruleset names it. CI reports; Git decides.

use myelin_git::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use myelin_git::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy, UnmetReason};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::BTreeMap;

const HEAD: &str = "c0ffee";

/// **PRODUCER side of 5.9** — CI emits the `CheckStatus` fact carried OPAQUE over the Bus, carrying
/// CI's `required` bool (its REPORT). The consumer decodes exactly this shape.
fn producer_fact(context: &str, state: CheckState, ci_required: bool, trust: TrustTier) -> CheckStatus {
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    CheckStatus {
        tenant: TenantId("acme".into()),
        repo: ArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid(HEAD.into()),
        context: CheckContext::ci(context),
        state,
        required: ci_required, // CI's REPORT — advisory; Git's policy is the authority.
        run: ArtifactRef("myelin://acme/ci/run/7".into()),
        run_attempt: 1,
        trust_tier: trust,
        details_ref: ArtifactRef("myelin://acme/ci/run/7#step-1".into()),
        summary: HumanisedRef { template_key: "ci.check.updated".into(), args },
        started_at: Timestamp("2026-06-22T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
        cost_settled: true,
    }
}

/// **CONSUMER side of 5.9** — Git decodes the producer's opaque payload, applies it, and gates with its
/// OWN required-set policy. The round-trip proves no second struct (the opaque value decodes to exactly
/// Git's `CheckStatus`).
fn consumer_apply(proj: &mut CheckStatusProjection, fact: &CheckStatus) {
    let opaque: serde_json::Value = serde_json::to_value(fact).unwrap();
    let decoded: CheckStatus = serde_json::from_value(opaque).unwrap();
    assert_eq!(&decoded, fact, "the opaque Bus payload decodes to exactly Git's CheckStatus");
    proj.apply(&decoded);
}

/// **THE CDC: Git's required-set policy — NOT CI's `required` bool — decides the gate.** CI marks
/// `lint` as `required = false`; Git's ruleset NAMES `ci/lint` as required → the gate gates on it. CI
/// marks `build` as `required = true`; Git's ruleset does NOT name it → the gate ignores it. **CI
/// reports; Git decides which facts gate** (Δ1 / X-1).
#[test]
fn cdc_5_9_git_required_set_policy_overrides_the_ci_required_bool() {
    let head = GitOid(HEAD.into());
    let mut proj = CheckStatusProjection::new();

    // CI reports: build green (CI says required=true), lint FAILING (CI says required=FALSE).
    consumer_apply(&mut proj, &producer_fact("build", CheckState::Success, true, TrustTier::Trusted));
    consumer_apply(&mut proj, &producer_fact("lint", CheckState::Failure, false, TrustTier::Trusted));

    // Git's OWN policy: gate on `ci/lint` ONLY (NOT `ci/build`) — the inverse of CI's `required` bools.
    let policy = MergeGatePolicy::from_required_contexts(&["ci/lint"]).unwrap();

    // The gate BLOCKS — because Git's policy names lint (which is failing), DESPITE CI marking lint
    // required=false. CI's bool did NOT decide; Git's policy did.
    match evaluate_merge_gate(&policy, &proj, &head, &[]) {
        MergeGateOutcome::Blocked { unmet } => {
            assert_eq!(unmet[0].context, CheckContext::ci("lint"));
            assert_eq!(unmet[0].reason, UnmetReason::NotGreen { state: CheckState::Failure });
        }
        MergeGateOutcome::Admitted => {
            panic!("Git's policy names lint → the failing lint must block (CI's required bool is advisory)")
        }
    }

    // Conversely: a policy naming ONLY `ci/build` ADMITS — even though CI marked build required and lint
    // is failing, Git's policy does not gate on lint. Git decides.
    let build_only = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
    assert_eq!(
        evaluate_merge_gate(&build_only, &proj, &head, &[]),
        MergeGateOutcome::Admitted,
        "a context Git's policy does not name does not gate, regardless of CI's required bool"
    );
}

/// **THE CDC: the fork-endorsement half of the required-set policy (Δ3 — the poisoned-pipeline
/// defence).** A required context whose current row is an `untrusted_fork` success is NEUTRAL for
/// gating (blocked) until endorsed — a fork cannot turn its OWN required gate green. The endorsement is
/// the maintainer `approve_untrusted_ci` input (the LIVE resolution is GIT-P22).
#[test]
fn cdc_5_9_untrusted_fork_success_is_neutral_until_endorsed() {
    let head = GitOid(HEAD.into());
    let mut proj = CheckStatusProjection::new();
    // CI reports the fork's self-greened build as a SUCCESS, but stamps trust_tier = untrusted_fork
    // (Git reads it OFF the fact — it never recomputes trust).
    consumer_apply(&mut proj, &producer_fact("build", CheckState::Success, true, TrustTier::UntrustedFork));

    let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();

    // Un-endorsed → neutral-for-gating → blocked (0 forks green their own required gate).
    assert!(matches!(
        evaluate_merge_gate(&policy, &proj, &head, &[]),
        MergeGateOutcome::Blocked { .. }
    ));
    // Endorsed → admitted.
    assert_eq!(
        evaluate_merge_gate(&policy, &proj, &head, &[CheckContext::ci("build")]),
        MergeGateOutcome::Admitted
    );
}

/// **THE CDC: 0 under-gated merges — every non-green posture blocks.** The required-set gate is
/// fail-closed: a missing, a non-success, a pending, AND an un-endorsed-fork required context EACH
/// block. The gate admits ONLY when every required context is a current trusted/endorsed success.
#[test]
fn cdc_5_9_zero_under_gated_merges_every_non_green_posture_blocks() {
    let head = GitOid(HEAD.into());
    let policy = MergeGatePolicy::from_required_contexts(&["ci/x"]).unwrap();

    // missing → block
    let empty = CheckStatusProjection::new();
    assert!(matches!(
        evaluate_merge_gate(&policy, &empty, &head, &[]),
        MergeGateOutcome::Blocked { .. }
    ));

    // each non-success terminal/pending state → block
    for state in [
        CheckState::Failure,
        CheckState::Error,
        CheckState::Cancelled,
        CheckState::Neutral,
        CheckState::Queued,
        CheckState::InProgress,
    ] {
        let mut proj = CheckStatusProjection::new();
        consumer_apply(&mut proj, &producer_fact("x", state, true, TrustTier::Trusted));
        assert!(
            matches!(evaluate_merge_gate(&policy, &proj, &head, &[]), MergeGateOutcome::Blocked { .. }),
            "state {state:?} must block (only a success admits)"
        );
    }

    // a trusted success → admit (the ONLY admitting posture)
    let mut proj = CheckStatusProjection::new();
    consumer_apply(&mut proj, &producer_fact("x", CheckState::Success, true, TrustTier::Trusted));
    assert_eq!(evaluate_merge_gate(&policy, &proj, &head, &[]), MergeGateOutcome::Admitted);
}
