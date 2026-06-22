//! # The CDC pair for contract 5.9 — Refs' SUB-ANCHOR resolution half (the Refs half of X-1)
//! (REF-P19 / P-335)
//!
//! **Contract:** index row 5.9 (the Git↔CI `CheckStatus` seam). CI is the PRODUCER (the
//! `ci.check.updated` facts + the 11.8 sealed CI log segments); Git owns the merge gate + the
//! `check_status` projection. **Refs' role is narrow:** the SUB-ANCHOR resolution of
//! `check-<context>` / `step-<n>` — Refs proves only that the check/step anchors resolve correctly
//! through the ONE ladder (C-6), incl. resolving the `#step-<n>` `details_ref` through the sealed log
//! segments (11.8). The seam itself (out-of-order supersession at the projection, fork-success-neutral
//! gating, the merge-queue wake) is the Git+CI X-1 deliverable (GIT-D10/CI-D8).
//!
//! - **PROVIDER** = CI's producer half — the CI-owned `CheckStatus` fact (decoded from the OPAQUE
//!   `ci.check.updated` payload via the shared typed view `myelin_git::check_status::CheckStatus`) and
//!   the 11.8 sealed-log `#step-<n>` resolution. This pair consumes the SAME frozen shapes the Bus
//!   carriage (`crates/myelin-events/tests/cdc_5_9_check_seam_carriage.rs`) and Git's consumer
//!   (`crates/myelin-git/tests/cdc_5_9_check_status_consumer.rs`) pin — no second struct, no drift.
//! - **CONSUMER** = Refs' sub-anchor resolver ([`myelin_refs_service::CiOwner`]) mapping a CI `#sub`
//!   KIND through the ONE ladder onto the frozen `live/moved/outdated/gone` state: a `check-<context>`
//!   → the CURRENT (latest-by-`run_attempt`) check state; a `step-<n>` → the sealed-log resolution.
//!
//! The dated green artifact: every `check-<context>` / `step-<n>` resolves through the one ladder to
//! the correct state with the ROOT carried (REF-D9 on the CI anchors); an out-of-order
//! `ci.check.updated` resolves the LATEST by `run_attempt` at the sub-anchor level. If 5.9's
//! `CheckStatus` shape or the `check-`/`step-` grammar drifts, this stops passing — that is the
//! contract.

use std::sync::Arc;
use std::sync::Mutex;

use myelin_events::ArtifactRef;
use myelin_git::check_status::{
    ApplyOutcome, CheckContext, CheckState, CheckStatus, GitOid, HumanisedRef, Timestamp, TrustTier,
};
use myelin_refs_service::{
    resolve_sub_outcome, CiOwner, ProjectOutcome, ProjectionFlag, StepAnchorResolver,
    StepResolution,
};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

/// **PROVIDER side of 5.9** — a CI-owned `CheckStatus` fact in the shared typed view. The Bus carries
/// it OPAQUE; the consumer decodes it into THIS struct (no second struct — the carriage seam is the
/// one shape). `run_attempt` is the supersession authority (never wall-clock).
fn ci_fact(
    commit: &str,
    ctx: &str,
    attempt: u32,
    state: CheckState,
    run: &str,
    step: u32,
) -> CheckStatus {
    CheckStatus {
        tenant: tenant(),
        repo: ArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid(commit.into()),
        context: CheckContext::ci(ctx),
        state,
        required: true,
        run: ArtifactRef(format!("myelin://acme/ci/run/{run}")),
        run_attempt: attempt,
        trust_tier: TrustTier::Trusted,
        details_ref: ArtifactRef(format!("myelin://acme/ci/run/{run}#step-{step}")),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: std::collections::BTreeMap::new(),
        },
        started_at: Timestamp("2026-06-21T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-21T00:01:00Z".into())),
        cost_settled: true,
    }
}

/// **PROVIDER side of 5.9 (the 11.8 leg)** — a scripted `#step-<n>` `details_ref` resolution standing
/// in for the sealed CI log segments (`myelin_storage::CiLogTier`, proven REAL in
/// `integration_ref_p19_ci_producer.rs`). Here the CDC pins the SHAPE of the resolution; the live
/// sealed-segment proof is the integration test.
#[derive(Default)]
struct ScriptedSteps {
    by_anchor: Mutex<std::collections::BTreeMap<String, StepResolution>>,
}
impl ScriptedSteps {
    fn set(&self, anchor: &ArtifactRef, res: StepResolution) {
        self.by_anchor.lock().unwrap().insert(anchor.0.clone(), res);
    }
}
impl StepAnchorResolver for ScriptedSteps {
    fn resolve_step(&self, anchor: &ArtifactRef) -> StepResolution {
        self.by_anchor
            .lock()
            .unwrap()
            .get(&anchor.0)
            .cloned()
            .unwrap_or(StepResolution::Gone)
    }
}

/// **CONSUMER side of 5.9** — a `check-<context>` sub-anchor resolves through the ONE ladder to the
/// CURRENT check state. A `success` → LIVE; an in-flight check → OUTDATED (not-yet-final); a `failure`
/// → LIVE (the failing verdict renders; whether it BLOCKS the merge is Git's gate, not Refs').
#[test]
fn cdc_5_9_check_context_anchor_resolves_through_the_one_ladder() {
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "build");

    // success → LIVE (no flag).
    owner.ingest_check(
        &anchor,
        &ci_fact("abc123", "build", 1, CheckState::Success, "1", 3),
    );
    assert!(matches!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::Live(p) if p.flag.is_none()
    ));

    // a superseding in-flight re-run → OUTDATED.
    owner.ingest_check(
        &anchor,
        &ci_fact("abc123", "build", 2, CheckState::InProgress, "2", 3),
    );
    assert!(matches!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::Live(p) if p.flag == Some(ProjectionFlag::Outdated)
    ));
}

/// **CONSUMER side of 5.9** — an out-of-order `ci.check.updated` resolves the LATEST by `run_attempt`
/// at the sub-anchor level (the Refs half of the X-1 monotonic supersession). The late lower attempt is
/// DROPPED; the sub-anchor never regresses.
#[test]
fn cdc_5_9_out_of_order_check_resolves_latest_by_run_attempt() {
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "build");

    // The higher attempt (success) is applied first; the stale lower attempt (failure) arrives late.
    assert_eq!(
        owner.ingest_check(
            &anchor,
            &ci_fact("abc123", "build", 2, CheckState::Success, "2", 3)
        ),
        ApplyOutcome::Superseded { current_attempt: 2 }
    );
    assert_eq!(
        owner.ingest_check(
            &anchor,
            &ci_fact("abc123", "build", 1, CheckState::Failure, "1", 3)
        ),
        ApplyOutcome::DroppedStale {
            incoming_attempt: 1,
            current_attempt: 2
        }
    );
    // The sub-anchor resolves the latest-by-attempt success (LIVE), never the stale failure.
    assert!(matches!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::Live(_)
    ));
    assert_eq!(
        owner
            .current_row(&ci_fact("abc123", "build", 2, CheckState::Success, "2", 3))
            .unwrap()
            .state,
        CheckState::Success
    );
}

/// **CONSUMER side of 5.9 (the 11.8 leg)** — a `step-<n>` `details_ref` resolves through the sealed
/// log segments: LIVE iff the jump-to-failure resolves to bytes; GONE for an unknown/pruned step (the
/// root run still resolves, the embed shows the parent); ERASED for a crypto-shredded segment. The ONE
/// ladder, root carried.
#[test]
fn cdc_5_9_step_details_ref_resolves_through_the_sealed_log_ladder() {
    let owner = CiOwner::new();
    let steps = Arc::new(ScriptedSteps::default());
    let live = CiOwner::step_anchor("acme", "run-7", 2);
    let gone = CiOwner::step_anchor("acme", "run-7", 99);
    let erased = CiOwner::step_anchor("acme", "run-9", 1);
    steps.set(&live, StepResolution::Live { byte_len: 27 });
    steps.set(&gone, StepResolution::Gone);
    steps.set(&erased, StepResolution::Erased);
    owner.wire_step_resolver(steps);

    // LIVE — the jump-to-failure resolves to the exact failing step's bytes.
    assert!(matches!(
        resolve_sub_outcome(&owner, &live),
        ProjectOutcome::Live(_)
    ));
    // GONE — an unknown/pruned step tombstones (sub_gone); the root run is carried by the chokepoint.
    assert_eq!(resolve_sub_outcome(&owner, &gone), ProjectOutcome::SubGone);
    // ERASED — a crypto-shredded segment tombstones (erased).
    assert_eq!(resolve_sub_outcome(&owner, &erased), ProjectOutcome::Erased);
}
