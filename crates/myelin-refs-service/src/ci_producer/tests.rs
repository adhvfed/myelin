//! Unit drills for the REF-P19 / P-335 CI sub-anchor resolution (the Refs half of X-1). The ladder
//! arms (`check-<context>` → the supersession-current state; `step-<n>` → the 11.8 sealed-log
//! resolution), the out-of-order `run_attempt` supersession AT THE SUB-ANCHOR LEVEL, the
//! root-always-carried rule, and the default-deny leak guard — each pinned by a test a mutation flips.

use super::*;
use crate::ladder::resolve_sub_outcome;
use crate::resolve::ProjectOutcome;
use myelin_git::check_status::{
    ApplyOutcome, CheckContext, CheckState, CheckStatus, GitOid, HumanisedRef, Timestamp, TrustTier,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use std::collections::BTreeMap;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("u-alice".into()),
        PrincipalKind::Human,
        tenant(),
    )
}

/// A decoded `ci.check.updated` fact for `(commit, context, attempt, state)` — the CI-owned struct the
/// Bus carries opaque (`run_attempt` is the supersession authority).
fn fact(commit: &str, ctx: &str, attempt: u32, state: CheckState) -> CheckStatus {
    CheckStatus {
        tenant: tenant(),
        repo: ArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid(commit.into()),
        context: CheckContext::ci(ctx),
        state,
        required: true,
        run: ArtifactRef(format!("myelin://acme/ci/run/{attempt}")),
        run_attempt: attempt,
        trust_tier: TrustTier::Trusted,
        details_ref: ArtifactRef(format!("myelin://acme/ci/run/{attempt}#step-3")),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: BTreeMap::new(),
        },
        started_at: Timestamp("2026-06-21T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-21T00:01:00Z".into())),
        cost_settled: true,
    }
}

/// A recorded `#step-<n>` resolver (the 11.8 seam stand-in) — scripts the resolution per anchor URN.
#[derive(Default)]
struct RecordedStepResolver {
    by_anchor: Mutex<BTreeMap<String, StepResolution>>,
}
impl RecordedStepResolver {
    fn set(&self, anchor: &ArtifactRef, res: StepResolution) {
        self.by_anchor.lock().unwrap().insert(anchor.0.clone(), res);
    }
}
impl StepAnchorResolver for RecordedStepResolver {
    fn resolve_step(&self, anchor: &ArtifactRef) -> StepResolution {
        self.by_anchor
            .lock()
            .unwrap()
            .get(&anchor.0)
            .cloned()
            // An anchor the seam never saw is GONE (a pruned run / unknown step — never a leak).
            .unwrap_or(StepResolution::Gone)
    }
}

// ── check-<context> resolves to the current state through the ONE ladder (C-6) ──

#[test]
fn a_successful_check_anchor_resolves_live() {
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "build");
    owner.ingest_check(&anchor, &fact("abc123", "build", 1, CheckState::Success));

    let outcome = resolve_sub_outcome(&owner, &anchor);
    assert!(
        matches!(outcome, ProjectOutcome::Live(p) if p.flag.is_none()),
        "a success check resolves LIVE (no flag)"
    );
}

#[test]
fn an_in_flight_check_anchor_resolves_outdated() {
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "build");
    owner.ingest_check(&anchor, &fact("abc123", "build", 1, CheckState::InProgress));

    // An in-flight (queued/in_progress) check renders but flags "not yet final" — OUTDATED, never a
    // hard fail (the merge gate's blocking decision is Git's, GIT-D10).
    let outcome = resolve_sub_outcome(&owner, &anchor);
    assert!(
        matches!(&outcome, ProjectOutcome::Live(p)
            if p.flag == Some(crate::resolve::ProjectionFlag::Outdated)),
        "an in-flight check resolves OUTDATED (not-yet-final), got {outcome:?}"
    );
}

#[test]
fn a_failure_check_anchor_resolves_live_not_a_tombstone() {
    // Refs resolves only that the anchor points at the right STATE — a `failure` is a LIVE render of
    // the failing verdict (whether it BLOCKS the merge is Git's gate, not Refs'). Never a tombstone.
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "test");
    owner.ingest_check(&anchor, &fact("abc123", "test", 1, CheckState::Failure));
    assert!(matches!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::Live(_)
    ));
}

// ── the HEADLINE: out-of-order supersession honoured AT THE SUB-ANCHOR LEVEL (X-1) ──

#[test]
fn out_of_order_ci_check_resolves_the_latest_by_run_attempt() {
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "build");

    // The HIGHER attempt (a re-run, success) arrives FIRST; the stale LOWER attempt (failure) arrives
    // LATE (the at-least-once transport reordered them). The sub-anchor must resolve the LATEST by
    // run_attempt — the success — NEVER the physically-last-arrived failure.
    let hi = owner.ingest_check(&anchor, &fact("abc123", "build", 2, CheckState::Success));
    assert_eq!(hi, ApplyOutcome::Superseded { current_attempt: 2 });

    let lo = owner.ingest_check(&anchor, &fact("abc123", "build", 1, CheckState::Failure));
    assert_eq!(
        lo,
        ApplyOutcome::DroppedStale {
            incoming_attempt: 1,
            current_attempt: 2
        },
        "the late lower attempt is DROPPED (monotonic run_attempt supersession)"
    );

    // The sub-anchor resolves the latest-by-attempt (the success), NOT the last-arrived (the failure).
    let outcome = resolve_sub_outcome(&owner, &anchor);
    assert!(
        matches!(outcome, ProjectOutcome::Live(_)),
        "resolves the latest-by-attempt success"
    );
    let row = owner
        .current_row(&fact("abc123", "build", 2, CheckState::Success))
        .expect("a current row");
    assert_eq!(
        row.run_attempt, 2,
        "the high-water mark is the re-run attempt"
    );
    assert_eq!(
        row.state,
        CheckState::Success,
        "the current state is the re-run success, never the stale failure"
    );
}

#[test]
fn a_higher_attempt_arriving_later_supersedes() {
    // The in-order case: attempt 1 (failure) then a re-run attempt 2 (success) — the success wins.
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "build");
    owner.ingest_check(&anchor, &fact("abc123", "build", 1, CheckState::Failure));
    let outcome = owner.ingest_check(&anchor, &fact("abc123", "build", 2, CheckState::Success));
    assert_eq!(outcome, ApplyOutcome::Superseded { current_attempt: 2 });
    assert!(matches!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::Live(_)
    ));
}

// ── step-<n> details_ref resolves through the 11.8 sealed CI log segments ──

#[test]
fn a_step_anchor_resolving_to_bytes_is_live() {
    let owner = CiOwner::new();
    let steps = Arc::new(RecordedStepResolver::default());
    let anchor = CiOwner::step_anchor("acme", "run-7", 2);
    steps.set(&anchor, StepResolution::Live { byte_len: 13 });
    owner.wire_step_resolver(steps);

    let outcome = resolve_sub_outcome(&owner, &anchor);
    match outcome {
        ProjectOutcome::Live(p) => {
            assert!(
                p.state.contains("13 bytes"),
                "the projection reflects the jump-to-failure target bytes, got {p:?}"
            );
        }
        other => panic!("a resolvable #step-<n> is LIVE, got {other:?}"),
    }
}

#[test]
fn a_step_anchor_for_an_unknown_step_is_a_root_carrying_tombstone() {
    let owner = CiOwner::new();
    let steps = Arc::new(RecordedStepResolver::default());
    let anchor = CiOwner::step_anchor("acme", "run-7", 99);
    steps.set(&anchor, StepResolution::Gone);
    owner.wire_step_resolver(steps);

    // GONE → SubGone (the root run still resolves; the chokepoint carries the root — the embed shows
    // the parent run, never a hard 404).
    assert_eq!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::SubGone
    );
}

#[test]
fn a_crypto_shredded_step_segment_is_erased() {
    let owner = CiOwner::new();
    let steps = Arc::new(RecordedStepResolver::default());
    let anchor = CiOwner::step_anchor("acme", "run-7", 2);
    steps.set(&anchor, StepResolution::Erased);
    owner.wire_step_resolver(steps);
    assert_eq!(resolve_sub_outcome(&owner, &anchor), ProjectOutcome::Erased);
}

#[test]
fn a_step_anchor_with_no_resolver_wired_is_gone_not_a_leak() {
    // Defensive: a `#step-<n>` resolved before the 11.8 seam is wired degrades GONE (the root carries),
    // NEVER a fabricated LIVE projection.
    let owner = CiOwner::new();
    let anchor = CiOwner::step_anchor("acme", "run-7", 2);
    assert_eq!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::SubGone
    );
}

// ── the bare root + the leak guard ──

#[test]
fn a_bare_ci_root_is_live() {
    let owner = CiOwner::new();
    let root = CiOwner::run_root("acme", "run-7");
    assert!(matches!(
        resolve_sub_outcome(&owner, &root),
        ProjectOutcome::Live(_)
    ));
}

#[test]
fn default_deny_a_viewer_with_no_ci_read_is_denied_at_the_root() {
    // The leak invariant's authz half: an ungranted viewer gets Deny on the root (the chokepoint turns
    // it into Tombstone{denied} — the check/step state never leaks).
    let owner = CiOwner::new();
    let root = CiOwner::check_root("acme", "abc123");
    let perm = Permission(crate::VIEW_PERMISSION.into());
    assert_eq!(
        owner
            .check_view(&tenant(), &region(), &root, &viewer(), &perm)
            .unwrap(),
        Decision::Deny,
        "an ungranted viewer is default-denied at the CI root (no state leaks)"
    );
    // A granted viewer is allowed.
    owner.grant_view(&tenant(), &region(), &viewer(), &root);
    assert_eq!(
        owner
            .check_view(&tenant(), &region(), &root, &viewer(), &perm)
            .unwrap(),
        Decision::Allow
    );
}

#[test]
fn an_unreported_check_anchor_resolves_outdated_not_a_leak() {
    // A `check-<context>` the CI producer half hasn't reported yet resolves OUTDATED (in-flight
    // default) — never a fabricated success, never a hard 404.
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "never-reported");
    assert!(matches!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::Live(p) if p.flag == Some(crate::resolve::ProjectionFlag::Outdated)
    ));
}

#[test]
fn the_ci_owner_token_is_the_canonical_ci_subsystem() {
    assert_eq!(CI_OWNER_TOKEN, "ci");
}
