use myelin_substrate::shed::{RunClass, ShedDecision, ShedLane, Surface, SurfaceBudget};
use myelin_substrate::thresholds::{CiSurge, ThresholdError, Thresholds};
use myelin_tenancy::TenantId;

#[derive(Clone, Debug)]
pub struct CiSurgeControls {
    ci_surge: CiSurge,
    multiplier: u32,
}

impl CiSurgeControls {
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<CiSurgeControls, String> {
        let ci_surge = thresholds.ci_surge.clone();
        if !ci_surge.is_well_formed() {
            return Err(
                "ci_surge thresholds are not well-formed (a vacuous bar - EI-01 §3)".into(),
            );
        }
        let shed_cap = thresholds
            .shed_budget(Surface::CiDispatch)
            .map_err(|e: ThresholdError| format!("CiDispatch shed budget unavailable: {e}"))?
            .per_tenant_in_flight_cap;
        if ci_surge.per_tenant_in_flight_cap != shed_cap {
            return Err(format!(
                "ci_surge cap {} != CiDispatch shed cap {} - the scheduler cap and the public-surface \
                 shed budget MUST agree (one number, not two)",
                ci_surge.per_tenant_in_flight_cap, shed_cap
            ));
        }
        Ok(CiSurgeControls {
            ci_surge,
            multiplier: thresholds.surge.multiplier,
        })
    }

    pub fn per_tenant_in_flight_cap(&self) -> u32 {
        self.ci_surge.per_tenant_in_flight_cap
    }

    pub fn multiplier(&self) -> u32 {
        self.multiplier
    }

    pub fn drr_base_quantum(&self) -> i64 {
        self.ci_surge.drr_base_quantum
    }

    pub fn drr_deficit_ceiling(&self) -> i64 {
        self.ci_surge.drr_deficit_ceiling
    }

    pub fn starvation_wait_p99_max_ticks(&self) -> u64 {
        self.ci_surge.starvation_wait_p99_max_ticks
    }

    pub fn hierarchical_promotion_owed(&self, measured_wait_p99_ticks: u64) -> bool {
        self.ci_surge
            .hierarchical_promotion_owed_for(measured_wait_p99_ticks)
    }

    pub fn prewarm_buffer_for(&self, arrival_rate: u32) -> u32 {
        self.ci_surge.prewarm_buffer_for(arrival_rate)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiDispatchShed {
    pub lane: RunClass,
    pub retry_after_secs: u64,
}

pub struct CiSurgeGate {
    lane: ShedLane,
}

impl CiSurgeGate {
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<CiSurgeGate, String> {
        let budget = thresholds
            .shed_budget(Surface::CiDispatch)
            .map_err(|e| format!("CI shed budget for CiDispatch unavailable: {e}"))?;
        Ok(CiSurgeGate {
            lane: ShedLane::with_budget(Surface::CiDispatch, budget),
        })
    }

    pub fn with_budget(budget: SurfaceBudget) -> CiSurgeGate {
        CiSurgeGate {
            lane: ShedLane::with_budget(Surface::CiDispatch, budget),
        }
    }

    pub fn admit(&mut self, tenant: &TenantId, class: RunClass) -> Result<(), CiDispatchShed> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(CiDispatchShed {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        self.lane.release(tenant, class);
    }

    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }
}

#[derive(Clone, Debug, Default)]
pub struct StarvationHistogram {
    waits: Vec<u64>,
}

impl StarvationHistogram {
    pub fn new() -> StarvationHistogram {
        StarvationHistogram::default()
    }

    pub fn record_wait(&mut self, wait_ticks: u64) {
        self.waits.push(wait_ticks);
    }

    pub fn len(&self) -> usize {
        self.waits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waits.is_empty()
    }

    pub fn wait_p99_ticks(&self) -> u64 {
        if self.waits.is_empty() {
            return 0;
        }
        let mut sorted = self.waits.clone();
        sorted.sort_unstable();
        let n = sorted.len();
        let rank = ((99 * n) as f64 / 100.0).ceil() as usize;
        let idx = rank.saturating_sub(1).min(n - 1);
        sorted[idx]
    }

    pub fn max_wait_ticks(&self) -> u64 {
        self.waits.iter().copied().max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fairness::{BASE_QUANTUM, DEFAULT_TENANT_IN_FLIGHT_CAP};

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    fn canonical_controls() -> CiSurgeControls {
        let t = Thresholds::load_canonical().expect("load canonical thresholds");
        CiSurgeControls::from_thresholds(&t).expect("CI-surge controls from the canonical file")
    }

    #[test]
    fn controls_cap_equals_the_cidispatch_shed_budget() {
        let t = Thresholds::load_canonical().expect("load");
        let controls = CiSurgeControls::from_thresholds(&t).expect("controls");
        let shed_cap = t
            .shed_budget(Surface::CiDispatch)
            .unwrap()
            .per_tenant_in_flight_cap;
        assert_eq!(controls.per_tenant_in_flight_cap(), shed_cap);
        assert_eq!(
            controls.per_tenant_in_flight_cap(),
            DEFAULT_TENANT_IN_FLIGHT_CAP
        );
        assert_eq!(controls.drr_base_quantum(), BASE_QUANTUM);
    }

    #[test]
    fn mismatched_cap_is_a_loud_error() {
        let mut t = Thresholds::load_canonical().expect("load");
        t.ci_surge.per_tenant_in_flight_cap = 999;
        assert!(
            CiSurgeControls::from_thresholds(&t).is_err(),
            "a scheduler cap that disagrees with the shed budget is a loud error (one number, not two)"
        );
    }

    #[test]
    fn vacuous_controls_are_a_loud_error() {
        let mut t = Thresholds::load_canonical().expect("load");
        t.ci_surge.starvation_wait_p99_max_ticks = 0;
        assert!(CiSurgeControls::from_thresholds(&t).is_err());
    }

    #[test]
    fn interactive_holds_while_batch_sheds() {
        let mut gate = CiSurgeGate::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 8,
            human_lane_reservation: 0,
            retry_after_secs: 5,
        });
        let acme = tenant("acme");
        for _ in 0..16 {
            let _ = gate.admit(&acme, RunClass::BatchCi);
        }
        assert!(
            gate.shed_count(RunClass::BatchCi) > 0,
            "over its graded ceiling the batch lane sheds (429 + Retry-After)"
        );
        match gate.admit(&acme, RunClass::BatchCi) {
            Err(shed) => assert_eq!(shed.retry_after_secs, 5),
            Ok(()) => panic!("the saturated batch lane must shed"),
        }
        assert!(
            gate.admit(&acme, RunClass::Human).is_ok(),
            "the interactive PR-check is held last - admitted while batch sheds"
        );
        assert_eq!(gate.shed_count(RunClass::Human), 0, "0 interactive shed");
    }

    #[test]
    fn cap_is_per_tenant() {
        let mut gate = CiSurgeGate::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 8,
            human_lane_reservation: 0,
            retry_after_secs: 5,
        });
        let noisy = tenant("noisy");
        let quiet = tenant("quiet");
        for _ in 0..16 {
            let _ = gate.admit(&noisy, RunClass::BatchCi);
        }
        assert!(
            gate.shed_count(RunClass::BatchCi) > 0,
            "noisy's batch lane sheds"
        );
        assert!(
            gate.admit(&quiet, RunClass::BatchCi).is_ok(),
            "a different tenant is unaffected (per-tenant blast radius)"
        );
    }

    #[test]
    fn wait_p99_is_nearest_rank() {
        let mut h = StarvationHistogram::new();
        assert_eq!(
            h.wait_p99_ticks(),
            0,
            "empty → 0 (no contention, no starvation)"
        );
        for w in 1..=100 {
            h.record_wait(w);
        }
        assert_eq!(h.wait_p99_ticks(), 99);
        assert_eq!(h.max_wait_ticks(), 100);
        assert_eq!(h.len(), 100);
    }

    #[test]
    fn hierarchical_promotion_gated_on_starvation_p99() {
        let controls = canonical_controls();
        let trigger = controls.starvation_wait_p99_max_ticks();
        assert!(
            !controls.hierarchical_promotion_owed(trigger),
            "at the trigger → within budget"
        );
        assert!(
            controls.hierarchical_promotion_owed(trigger + 1),
            "over the trigger → the hierarchical scheduler is owed (CI-P29)"
        );
    }

    #[test]
    fn prewarm_sizing_is_measured_and_bounded() {
        let controls = canonical_controls();
        assert_eq!(controls.prewarm_buffer_for(0), 0, "idle → no pre-warm");
        assert!(
            controls.prewarm_buffer_for(100) > 0,
            "a busy pool keeps a warm buffer"
        );
        let huge = controls.prewarm_buffer_for(1_000_000);
        assert!(
            huge > 0 && huge <= 64,
            "the warm buffer is clamped (never unbounded)"
        );
    }
}
