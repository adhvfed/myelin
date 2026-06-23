//! # The CDC PROVIDER half of contract 5.9 — CI's `ci.check.updated` producer (CI-P18 → P-361, M4)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.9 (the
//! Git↔CI `CheckStatus` seam — **CI is the PRODUCER**; `ci.check.updated` + the `run_attempt`
//! source). Owning architecture:
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §4 (the seam, produced) + `01-tech-and-data-model.md` §3.2 (the `check_attempt` counter).
//! Reconciliation: `00-reconciliation-decisions.md` X-1 (the FROZEN `CheckStatus` struct).
//!
//! ## What this CDC pins (the check-fact half of 5.9 — the PROVIDER's promise)
//! CI assembles the `ci.check.updated` payload through [`myelin_ci_controlplane::check_status_payload`]
//! (CI never depends on Git in PRODUCTION code — it produces the frozen JSON shape). This CDC proves
//! the PROVIDER ↔ CONSUMER no-drift property: CI's assembled payload decodes BYTE-IDENTICALLY into
//! Git's frozen consumer view `myelin_git::check_status::CheckStatus` via the real consumer
//! `CheckStatusConsumer::decode` — the exact decode the live Git consumer leg runs. If CI's producer
//! shape ever diverged from the frozen 5.9 struct, this decode would FAIL (a loud contract break).
//!
//! The CONSUMER half (Git's `check_status` projection + the monotonic `run_attempt` supersession +
//! the merge gate) is proven in `myelin-git`; the END-TO-END seam GATE (GIT-D10 / CI-D8, the
//! `ci.result` rollup → merge-queue wake, 0 double-merge) is CI-P19 (P-362) — the floor this CDC
//! provider half names. This is a DEV-DEP-ONLY decode (the test surface), acyclic exactly like
//! `myelin-ci-dispatch → myelin-git`.

use myelin_ci_controlplane::{
    check_status_payload, CheckEmitContext, CheckProvider, CheckState, CostPosture, TrustTier,
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

/// **The PROVIDER promise: CI's assembled `ci.check.updated` payload decodes into Git's FROZEN
/// `CheckStatus` (no drift — the check-fact half of 5.9).** CI produces the shape; Git's REAL consumer
/// decode (`CheckStatusConsumer::decode`) accepts it byte-for-byte. Every frozen field round-trips.
#[test]
fn ci_producer_payload_decodes_into_git_frozen_checkstatus() {
    let ctx = emit_ctx(2, TrustTier::Trusted);
    let payload = check_status_payload(
        &ctx,
        CheckProvider::Ci,
        "build",
        CheckState::Success,
        true,
        CostPosture::Settled,
        None,
    );

    // The REAL Git consumer decode — the exact decode the live consumer leg runs.
    let fact = CheckStatusConsumer::decode(&payload)
        .expect("CI's producer payload decodes into Git's frozen 5.9 CheckStatus (no drift)");

    // Every frozen 5.9 field round-tripped into Git's typed view.
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
        fact.details_ref.0, "myelin://acme/ci/run/run-42#summary",
        "a success anchors on the run summary"
    );
    assert_eq!(
        fact.summary.template_key, "ci.check.success",
        "the summary is a HumanisedRef (template_key, args) — never a raw string (7.3)"
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

/// **A FORK run is recorded faithfully with `trust_tier = untrusted_fork` — CI never endorses it.**
/// The producer stamps the tier from provenance; Git's consumer decodes `UntrustedFork` and treats it
/// as neutral-for-gating until endorsed (the poisoned-pipeline defence — CI reports, Git gates).
#[test]
fn ci_producer_stamps_untrusted_fork_git_decodes_it_faithfully() {
    let ctx = emit_ctx(1, TrustTier::UntrustedFork);
    let payload = check_status_payload(
        &ctx,
        CheckProvider::Ci,
        "test",
        CheckState::Success,
        true,
        CostPosture::Settled,
        None,
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
        "stamped from provenance — CI never endorses a fork (X-1)"
    );
}

/// **A FAILURE fact decodes with the `#step-<n>` jump-to-failure `details_ref`, the failure summary,
/// and a not-yet-settled cost posture.** The producer assembles the failure shape; Git's consumer
/// decodes the `Failure` state, the failing-step anchor, and `cost_settled: false` (still pending
/// the settle bookend).
#[test]
fn ci_producer_failure_fact_decodes_with_step_anchor_and_unsettled() {
    let ctx = emit_ctx(3, TrustTier::Trusted);
    let payload = check_status_payload(
        &ctx,
        CheckProvider::Ci,
        "build",
        CheckState::Failure,
        true,
        CostPosture::Unsettled,
        Some(7),
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

/// **A malformed (incomplete) payload FAILS Git's decode — the seam shape is a real gate.** A payload
/// missing a frozen field is a LOUD decode error (Git dead-letters it), proving the CDC's no-drift
/// assertion above is a real constraint, not a vacuous pass.
#[test]
fn an_incomplete_payload_fails_the_git_decode() {
    // A payload missing `summary` / `tenant` / etc. (the OLD divergent loose shape).
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
