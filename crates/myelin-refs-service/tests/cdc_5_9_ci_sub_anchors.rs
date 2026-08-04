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

#[test]
fn cdc_5_9_check_context_anchor_resolves_through_the_one_ladder() {
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "build");

    owner.ingest_check(
        &anchor,
        &ci_fact("abc123", "build", 1, CheckState::Success, "1", 3),
    );
    assert!(matches!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::Live(p) if p.flag.is_none()
    ));

    owner.ingest_check(
        &anchor,
        &ci_fact("abc123", "build", 2, CheckState::InProgress, "2", 3),
    );
    assert!(matches!(
        resolve_sub_outcome(&owner, &anchor),
        ProjectOutcome::Live(p) if p.flag == Some(ProjectionFlag::Outdated)
    ));
}

#[test]
fn cdc_5_9_out_of_order_check_resolves_latest_by_run_attempt() {
    let owner = CiOwner::new();
    let anchor = CiOwner::check_anchor("acme", "abc123", "build");

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

    assert!(matches!(
        resolve_sub_outcome(&owner, &live),
        ProjectOutcome::Live(_)
    ));
    assert_eq!(resolve_sub_outcome(&owner, &gone), ProjectOutcome::SubGone);
    assert_eq!(resolve_sub_outcome(&owner, &erased), ProjectOutcome::Erased);
}
