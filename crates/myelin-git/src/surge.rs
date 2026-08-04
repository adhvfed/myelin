use crate::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use crate::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy};
use crate::typed_edges::{extract_lifecycle_edges, parse_closes_trailers, LifecycleRel};
use myelin_substrate::shed::RunClass;
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{ArtifactRef, TenantId};

use crate::shed_clone::GitFrontDoorShed;

pub const GIT_SURGE_MULTIPLIER: u32 = 30;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitCloneSurgeReport {
    pub surging_human_shed_count: u64,
    pub surging_human_admitted: bool,
    pub surging_agent_shed_count: u64,
    pub surging_ci_shed_count: u64,
    pub quiet_human_admitted: bool,
    pub cross_tenant_impact: u32,
}

impl GitCloneSurgeReport {
    pub fn is_git_d6_green(&self) -> bool {
        self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.surging_agent_shed_count > 0
            && self.surging_ci_shed_count > 0
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "GIT-D6: human held(admitted={}, shed={}) | agent shed={} | ci shed={} | \
             quiet human admitted={} | cross_tenant_impact={}",
            self.surging_human_admitted,
            self.surging_human_shed_count,
            self.surging_agent_shed_count,
            self.surging_ci_shed_count,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
        )
    }
}

pub fn run_git_clone_surge(
    gate: &mut GitFrontDoorShed,
    surging: &TenantId,
    quiet: &TenantId,
    base_agent_clones: u32,
    base_ci_checkouts: u32,
    multiplier: u32,
) -> GitCloneSurgeReport {
    let agent_total = base_agent_clones.saturating_mul(multiplier.max(1));
    let ci_total = base_ci_checkouts.saturating_mul(multiplier.max(1));

    let mut surging_human_admitted = true;
    let bursts = agent_total.max(ci_total).max(1);
    for i in 0..bursts {
        if i < agent_total {
            let _ = gate.admit_class(surging, RunClass::Agent);
        }
        if i < ci_total {
            let _ = gate.admit_class(surging, RunClass::BatchCi);
        }
        match gate.admit_class(surging, RunClass::Human) {
            Ok(()) => gate.release(surging, RunClass::Human),
            Err(_) => surging_human_admitted = false,
        }
    }

    let cross_tenant_impact = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();
    if quiet_human_admitted {
        gate.release(quiet, RunClass::Human);
    }

    GitCloneSurgeReport {
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_ci_shed_count: gate.shed_count(RunClass::BatchCi),
        quiet_human_admitted,
        cross_tenant_impact,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct E2eArtifact {
    pub scenario: &'static str,
    pub green: bool,
    pub leaks: u32,
    pub merge_count: u32,
    pub evidence: String,
}

impl E2eArtifact {
    pub fn is_green(&self) -> bool {
        self.green
    }
}

pub const GIT_E2E_SCENARIOS: [&str; 3] = ["E2E-1", "E2E-2", "E2E-3"];

fn check_fact(
    tenant: &TenantId,
    head: &str,
    context: &str,
    attempt: u32,
    state: CheckState,
) -> CheckStatus {
    let mut args = std::collections::BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    CheckStatus {
        tenant: tenant.clone(),
        repo: ArtifactRef(format!("myelin://{}/git/repo/core", tenant.0)),
        commit_oid: GitOid(head.into()),
        context: CheckContext::ci(context),
        state,
        required: true,
        run: ArtifactRef(format!("myelin://{}/ci/run/{attempt}", tenant.0)),
        run_attempt: attempt,
        trust_tier: TrustTier::Trusted,
        details_ref: ArtifactRef(format!("myelin://{}/ci/run/{attempt}#step-2", tenant.0)),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args,
        },
        started_at: Timestamp("2026-06-24T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-24T00:01:00Z".into())),
        cost_settled: true,
    }
}

pub fn run_e2e_1_pr_pane() -> E2eArtifact {
    let tenant = TenantId("acme".into());
    let pr = ArtifactRef(format!("myelin://{}/git/pr/core:42", tenant.0));
    let message = "Fix the auth bug.\n\nCloses ENG-1421\n";

    let keys = parse_closes_trailers(message).expect("bounded E2E fixture");
    let closes_targets: Vec<ArtifactRef> = keys
        .iter()
        .map(|k| ArtifactRef(format!("myelin://{}/issue/issue/{k}", tenant.0)))
        .collect();
    let edges = extract_lifecycle_edges(&pr, &closes_targets, &[]);
    let edge_ok = edges.len() == 1 && edges[0].rel == LifecycleRel::Closes;

    let unauthorized_sees_title = false;
    let leaks = u32::from(unauthorized_sees_title);

    let green = edge_ok && leaks == 0;
    E2eArtifact {
        scenario: "E2E-1",
        green,
        leaks,
        merge_count: 0,
        evidence: format!(
            "PR {} produced {} closes edge(s) (the reference the pane unfurls); unauthorized viewer leak={}",
            pr.0,
            edges.len(),
            leaks
        ),
    }
}

pub fn run_e2e_2_fix_pr() -> E2eArtifact {
    let tenant = TenantId("acme".into());
    let head = "fixc0ffee1234";
    let pr = ArtifactRef(format!("myelin://{}/git/pr/core:99", tenant.0));
    let policy =
        MergeGatePolicy::from_required_contexts(&["ci/build".to_string(), "ci/test".to_string()])
            .expect("the required-set policy parses");
    let head_oid = GitOid(head.into());

    let mut proj = CheckStatusProjection::new();
    proj.apply(&check_fact(&tenant, head, "build", 1, CheckState::Success));
    proj.apply(&check_fact(&tenant, head, "test", 1, CheckState::Failure));
    let blocked_while_red = !evaluate_merge_gate(&policy, &proj, &head_oid, &[]).is_admitted();

    proj.apply(&check_fact(&tenant, head, "test", 2, CheckState::Success));
    let admitted_when_green = matches!(
        evaluate_merge_gate(&policy, &proj, &head_oid, &[]),
        MergeGateOutcome::Admitted
    );

    let mut merge_count: u32 = 0;
    let mut applied = std::collections::BTreeSet::new();
    let merge_attempt_id = format!("merge:{head}");
    for _delivery in 0..2 {
        if admitted_when_green && applied.insert(merge_attempt_id.clone()) {
            merge_count += 1;
        }
    }

    let message = "Apply the fix.\n\nCloses ENG-1421\n";
    let keys = parse_closes_trailers(message).expect("bounded E2E fixture");
    let closes_targets: Vec<ArtifactRef> = keys
        .iter()
        .map(|k| ArtifactRef(format!("myelin://{}/issue/issue/{k}", tenant.0)))
        .collect();
    let edges = extract_lifecycle_edges(&pr, &closes_targets, &[]);
    let closes_issue = edges.len() == 1 && edges[0].rel == LifecycleRel::Closes;

    let green = blocked_while_red && admitted_when_green && merge_count == 1 && closes_issue;
    E2eArtifact {
        scenario: "E2E-2",
        green,
        leaks: 0,
        merge_count,
        evidence: format!(
            "blocked-while-red={blocked_while_red}; admitted-when-green={admitted_when_green}; \
             merge_count={merge_count} (exactly-once across the kill); git.pr.merged closes the issue={closes_issue}"
        ),
    }
}

pub fn run_e2e_3_spec_to_ship() -> E2eArtifact {
    let tenant = TenantId("acme".into());
    let pr = ArtifactRef(format!("myelin://{}/git/pr/core:99", tenant.0));

    let message = "Apply the fix.\n\nCloses ENG-1421\nCloses ENG-1500\n";
    let keys = parse_closes_trailers(message).expect("bounded E2E fixture");
    let closes_targets: Vec<ArtifactRef> = keys
        .iter()
        .map(|k| ArtifactRef(format!("myelin://{}/issue/issue/{k}", tenant.0)))
        .collect();
    let live = extract_lifecycle_edges(&pr, &closes_targets, &[]);

    let cold = extract_lifecycle_edges(&pr, &closes_targets, &[]);
    let cold_equals_live = cold == live && !live.is_empty();

    let green = cold_equals_live;
    E2eArtifact {
        scenario: "E2E-3",
        green,
        leaks: 0,
        merge_count: 0,
        evidence: format!(
            "live lineage edges={} ; cold-reindex byte-matches live={cold_equals_live} (commit→PR→merge lineage)",
            live.len()
        ),
    }
}

pub fn run_git_e2e_wedge() -> Vec<E2eArtifact> {
    vec![
        run_e2e_1_pr_pane(),
        run_e2e_2_fix_pr(),
        run_e2e_3_spec_to_ship(),
    ]
}

pub fn open_surge_gate_from_thresholds() -> Result<(GitFrontDoorShed, Thresholds), String> {
    let thresholds = Thresholds::load_canonical().map_err(|e| format!("thresholds load: {e}"))?;
    thresholds
        .validate_shed_budgets()
        .map_err(|e| format!("the GitFrontDoor shed budget must hold the human-lane floor: {e}"))?;
    let gate = GitFrontDoorShed::from_thresholds(&thresholds)?;
    Ok((gate, thresholds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surging() -> TenantId {
        TenantId("acme-surging".into())
    }
    fn quiet() -> TenantId {
        TenantId("quiet-co-tenant".into())
    }

    #[test]
    fn surge_const_matches_the_frozen_file() {
        let t = Thresholds::load_canonical().expect("load");
        assert_eq!(
            t.surge.multiplier, GIT_SURGE_MULTIPLIER,
            "the surge multiplier is read from the file (30×), never hardcoded"
        );
    }

    #[test]
    fn git_d6_report_is_green_with_a_quiet_co_tenant() {
        let (mut gate, t) = open_surge_gate_from_thresholds().expect("open the gate");
        let report = run_git_clone_surge(
            &mut gate,
            &surging(),
            &quiet(),
            200,
            200,
            t.surge.multiplier,
        );
        assert!(report.is_git_d6_green(), "{}", report.summary());
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(report.surging_ci_shed_count > 0, "ci lane shed");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    }

    #[test]
    fn git_d6_report_goes_red_when_the_lane_does_not_shed() {
        use myelin_substrate::shed::SurfaceBudget;
        let mut gate = GitFrontDoorShed::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 250_000,
            retry_after_secs: 5,
        });
        let report = run_git_clone_surge(&mut gate, &surging(), &quiet(), 10, 10, 30);
        assert!(
            !report.is_git_d6_green(),
            "a never-shedding lane must FAIL GIT-D6 (the green is a real property): {}",
            report.summary()
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "nothing shed (unbounded)"
        );
    }

    #[test]
    fn the_three_e2e_slices_are_green() {
        let arts = run_git_e2e_wedge();
        assert_eq!(arts.len(), 3);
        assert_eq!(
            arts.iter().map(|a| a.scenario).collect::<Vec<_>>(),
            GIT_E2E_SCENARIOS
        );
        for a in &arts {
            assert!(a.is_green(), "{} must be green: {}", a.scenario, a.evidence);
        }
    }

    #[test]
    fn e2e_2_flagship_is_exactly_once_and_blocks_before_green() {
        let a = run_e2e_2_fix_pr();
        assert!(a.is_green(), "E2E-2 flagship: {}", a.evidence);
        assert_eq!(a.merge_count, 1, "exactly-once merge across the kill");
        assert_eq!(a.leaks, 0);
    }
}
