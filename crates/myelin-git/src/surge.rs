use myelin_substrate::shed::RunClass;
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

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
}
