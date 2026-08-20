//! Per-tenant, principal-classed admission control for the edge.
//!
//! Wraps `myelin_substrate::shed::ShedLane` around the gateway's request
//! dispatch so a machine-lane storm (agents, CI) on one tenant sheds with
//! 429 + Retry-After while human requests keep flowing. The flat semaphores
//! in `server.rs` remain the process-wide memory backstop; the budgets here
//! must stay small enough to bind before those caps do for a single tenant.

use std::sync::{Arc, Mutex, MutexGuard};

use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

pub const RUN_CLASS_HEADER: &str = "x-myelin-run-class";

#[derive(Clone)]
pub struct EdgeShed {
    http: Arc<Mutex<ShedLane>>,
    git: Arc<Mutex<ShedLane>>,
}

impl EdgeShed {
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<EdgeShed, String> {
        let http = thresholds
            .shed_budget(Surface::HttpIntake)
            .map_err(|e| format!("HttpIntake shed budget unavailable: {e}"))?;
        let git = thresholds
            .shed_budget(Surface::GitFrontDoor)
            .map_err(|e| format!("GitFrontDoor shed budget unavailable: {e}"))?;
        http.validate_tuned(Surface::HttpIntake)
            .map_err(|e| e.to_string())?;
        git.validate_tuned(Surface::GitFrontDoor)
            .map_err(|e| e.to_string())?;
        Ok(EdgeShed::with_budgets(http, git))
    }

    /// The in-code fallback table; production overrides from thresholds.toml.
    pub fn v1_floor() -> EdgeShed {
        let table = myelin_substrate::shed::ShedBudgetTable::v1_floor();
        EdgeShed::with_budgets(
            table.budget(Surface::HttpIntake),
            table.budget(Surface::GitFrontDoor),
        )
    }

    pub fn with_budgets(http: SurfaceBudget, git: SurfaceBudget) -> EdgeShed {
        EdgeShed {
            http: Arc::new(Mutex::new(ShedLane::with_budget(Surface::HttpIntake, http))),
            git: Arc::new(Mutex::new(ShedLane::with_budget(
                Surface::GitFrontDoor,
                git,
            ))),
        }
    }

    fn lane(&self, surface: Surface) -> &Arc<Mutex<ShedLane>> {
        match surface {
            Surface::GitFrontDoor => &self.git,
            _ => &self.http,
        }
    }

    /// Admit or shed one request. On admit the returned permit releases the
    /// in-flight slot when dropped; on shed the caller owes the client a
    /// 429 with the returned Retry-After seconds.
    pub fn admit(
        &self,
        surface: Surface,
        tenant: &TenantId,
        class: RunClass,
    ) -> Result<ShedPermit, u64> {
        let lane = self.lane(surface);
        let decision = lock_lane(lane).admit(tenant, class);
        match decision {
            ShedDecision::Admit => Ok(ShedPermit {
                lane: lane.clone(),
                tenant: tenant.clone(),
                class,
            }),
            ShedDecision::Shed { retry_after_secs } => Err(retry_after_secs),
        }
    }
}

// A poisoned lane means a panic while the lock was held; the counters are
// plain integers, so recovering the inner state beats refusing all traffic.
fn lock_lane(lane: &Mutex<ShedLane>) -> MutexGuard<'_, ShedLane> {
    lane.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct ShedPermit {
    lane: Arc<Mutex<ShedLane>>,
    tenant: TenantId,
    class: RunClass,
}

impl Drop for ShedPermit {
    fn drop(&mut self) {
        lock_lane(&self.lane).release(&self.tenant, self.class);
    }
}

pub fn run_class_header(value: Option<&str>) -> Option<RunClassHeader> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "speculative" => Some(RunClassHeader::Speculative),
        "batch-ci" | "batch_ci" => Some(RunClassHeader::BatchCi),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::PrincipalKind;

    fn tiny(cap: u32, human: u32) -> SurfaceBudget {
        SurfaceBudget {
            per_tenant_in_flight_cap: cap,
            human_lane_reservation: human,
            retry_after_secs: 7,
        }
    }

    fn tenant(name: &str) -> TenantId {
        TenantId(name.into())
    }

    #[test]
    fn a_machine_storm_sheds_while_the_human_lane_admits() {
        let shed = EdgeShed::with_budgets(tiny(4, 2), tiny(2, 1));
        let t = tenant("acme");

        let a1 = shed.admit(Surface::HttpIntake, &t, RunClass::Agent);
        let a2 = shed.admit(Surface::HttpIntake, &t, RunClass::Agent);
        assert!(a1.is_ok() && a2.is_ok(), "machines fill their lane first");
        let a3 = shed.admit(Surface::HttpIntake, &t, RunClass::Agent);
        assert_eq!(
            a3.err(),
            Some(7),
            "the third machine request sheds with the tuned Retry-After"
        );

        let h = shed.admit(Surface::HttpIntake, &t, RunClass::Human);
        assert!(
            h.is_ok(),
            "the reserved human lane admits while machines are saturated"
        );
    }

    #[test]
    fn dropping_the_permit_releases_the_slot() {
        let shed = EdgeShed::with_budgets(tiny(4, 2), tiny(2, 1));
        let t = tenant("acme");
        let a1 = shed
            .admit(Surface::HttpIntake, &t, RunClass::Agent)
            .unwrap();
        let _a2 = shed
            .admit(Surface::HttpIntake, &t, RunClass::Agent)
            .unwrap();
        assert!(shed
            .admit(Surface::HttpIntake, &t, RunClass::Agent)
            .is_err());
        drop(a1);
        assert!(
            shed.admit(Surface::HttpIntake, &t, RunClass::Agent).is_ok(),
            "a released slot is admittable again"
        );
    }

    #[test]
    fn tenants_are_bulkheaded_from_each_other() {
        let shed = EdgeShed::with_budgets(tiny(4, 2), tiny(2, 1));
        let acme = tenant("acme");
        let globex = tenant("globex");
        let _a1 = shed
            .admit(Surface::HttpIntake, &acme, RunClass::Agent)
            .unwrap();
        let _a2 = shed
            .admit(Surface::HttpIntake, &acme, RunClass::Agent)
            .unwrap();
        assert!(shed
            .admit(Surface::HttpIntake, &acme, RunClass::Agent)
            .is_err());
        assert!(
            shed.admit(Surface::HttpIntake, &globex, RunClass::Agent)
                .is_ok(),
            "one tenant's storm never sheds another tenant"
        );
    }

    #[test]
    fn git_and_http_surfaces_have_independent_lanes() {
        let shed = EdgeShed::with_budgets(tiny(4, 2), tiny(2, 1));
        let t = tenant("acme");
        let _g = shed
            .admit(Surface::GitFrontDoor, &t, RunClass::Agent)
            .unwrap();
        assert!(
            shed.admit(Surface::GitFrontDoor, &t, RunClass::Agent)
                .is_err(),
            "the git lane's machine budget (cap 2, human 1) is one slot"
        );
        assert!(
            shed.admit(Surface::HttpIntake, &t, RunClass::Agent).is_ok(),
            "the http lane is unaffected by git saturation"
        );
    }

    #[test]
    fn the_run_class_header_only_ever_demotes() {
        let human = PrincipalKind::Human;
        let agent = PrincipalKind::Agent {
            runtime_ref: myelin_identity::RuntimeRef("rt".into()),
            on_behalf_of: None,
        };
        assert_eq!(
            RunClass::derive(&human, run_class_header(Some("batch-ci"))),
            RunClass::BatchCi,
            "self-demotion is allowed"
        );
        assert_eq!(
            RunClass::derive(&agent, run_class_header(Some("nonsense"))),
            RunClass::Agent,
            "an unknown header value is ignored, the principal ceiling stands"
        );
        assert_eq!(
            RunClass::derive(&agent, run_class_header(None)),
            RunClass::Agent
        );
        assert_eq!(
            RunClass::derive(&agent, run_class_header(Some("speculative"))),
            RunClass::Speculative
        );
    }
}
