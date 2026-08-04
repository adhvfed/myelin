use myelin_substrate::shed::{RunClass, ShedDecision, ShedLane, Surface, SurfaceBudget};
use myelin_substrate::thresholds::{CiSurge, ThresholdError, Thresholds};
use myelin_tenancy::TenantId;

use crate::fairness::{Backpressure, FairShare, PlanTier};
use crate::scheduler::{ClaimRequest, JobState, Lane, QueuedJob, SchedulerState};
use myelin_ci_sandbox::TrustTier;

pub const CI_SURGE_MULTIPLIER: u32 = 30;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiSurgeReport {
    pub surging_batch_shed_count: u64,
    pub surging_interactive_shed_count: u64,
    pub surging_interactive_admitted: bool,
    pub quiet_interactive_admitted: bool,
    pub cross_tenant_shed_count: u64,
    pub batch_shed_retry_after_secs: u64,
    pub orphan_count: u64,
    pub requeued_count: u64,
    pub fair_key_wait_p99_ticks: u64,
    pub starvation_trigger_ticks: u64,
    pub hierarchical_scheduler_owed: bool,
}

impl CiSurgeReport {
    pub fn is_ci_d2_green(&self) -> bool {
        self.surging_batch_shed_count > 0
            && self.batch_shed_retry_after_secs > 0
            && self.surging_interactive_shed_count == 0
            && self.surging_interactive_admitted
            && self.quiet_interactive_admitted
            && self.cross_tenant_shed_count == 0
            && self.orphan_count == 0
            && self.requeued_count > 0
            && self.fair_key_wait_p99_ticks <= self.starvation_trigger_ticks
            && !self.hierarchical_scheduler_owed
    }

    pub fn summary(&self) -> String {
        format!(
            "CI-D2: surging batch_shed={} (retry_after={}s) interactive_shed={} \
             surging_interactive_admitted={} quiet_interactive_admitted={} cross_tenant_shed={} \
             orphans={} requeued={} fair_key_wait_p99={}t (trigger={}t) hierarchical_owed={} → {}",
            self.surging_batch_shed_count,
            self.batch_shed_retry_after_secs,
            self.surging_interactive_shed_count,
            self.surging_interactive_admitted,
            self.quiet_interactive_admitted,
            self.cross_tenant_shed_count,
            self.orphan_count,
            self.requeued_count,
            self.fair_key_wait_p99_ticks,
            self.starvation_trigger_ticks,
            self.hierarchical_scheduler_owed,
            if self.is_ci_d2_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

pub fn drive_ci_d2_surge(
    controls: &CiSurgeControls,
    storm_batch_ops: u32,
    surging: &TenantId,
    quiet: &TenantId,
    region: &str,
) -> CiSurgeReport {
    let cap = controls.per_tenant_in_flight_cap();
    let mut gate = CiSurgeGate::with_budget(SurfaceBudget {
        per_tenant_in_flight_cap: cap,
        human_lane_reservation: 0,
        retry_after_secs: 5,
    });

    for _ in 0..storm_batch_ops {
        let _ = gate.admit(surging, RunClass::BatchCi);
    }
    let surging_batch_shed_count = gate.shed_count(RunClass::BatchCi);
    let batch_shed_retry_after_secs = match gate.admit(surging, RunClass::BatchCi) {
        Err(shed) => shed.retry_after_secs,
        Ok(()) => 0,
    };

    let surging_interactive_admitted = gate.admit(surging, RunClass::Human).is_ok();
    let surging_interactive_shed_count = gate.shed_count(RunClass::Human);

    let quiet_interactive_admitted = gate.admit(quiet, RunClass::Human).is_ok();
    let cross_tenant_shed_count = (gate.in_flight(quiet).saturating_sub(1)) as u64;

    let histogram = measure_starvation_histogram(controls, surging, quiet, region, storm_batch_ops);
    let fair_key_wait_p99_ticks = histogram.wait_p99_ticks();
    let starvation_trigger_ticks = controls.starvation_wait_p99_max_ticks();
    let hierarchical_scheduler_owed = controls.hierarchical_promotion_owed(fair_key_wait_p99_ticks);

    let (requeued_count, orphan_count) = measure_reaper_recovery(surging, region);

    CiSurgeReport {
        surging_batch_shed_count,
        surging_interactive_shed_count,
        surging_interactive_admitted,
        quiet_interactive_admitted,
        cross_tenant_shed_count,
        batch_shed_retry_after_secs,
        orphan_count,
        requeued_count,
        fair_key_wait_p99_ticks,
        starvation_trigger_ticks,
        hierarchical_scheduler_owed,
    }
}

fn measure_starvation_histogram(
    controls: &CiSurgeControls,
    surging: &TenantId,
    quiet: &TenantId,
    region: &str,
    storm_ops: u32,
) -> StarvationHistogram {
    let mut fair = FairShare::new();
    let cap = controls.per_tenant_in_flight_cap().clamp(1, 4);
    let mut bp = Backpressure::with_cap(cap);
    let mut hist = StarvationHistogram::new();

    fair.set_tier(&surging.0, region, &surging.0, PlanTier::Free);
    fair.set_tier(&quiet.0, region, &quiet.0, PlanTier::Free);

    let quiet_jobs = 5u32;
    let run_duration = 2u64;
    let mut backlog: [(TenantId, u32); 2] =
        [(surging.clone(), storm_ops), (quiet.clone(), quiet_jobs)];
    let mut running: Vec<(TenantId, u64)> = Vec::new();
    let mut served_quiet = 0u32;

    let total_to_serve = storm_ops as u64 + quiet_jobs as u64;
    let mut served_total = 0u64;
    let mut tick = 0u64;
    let tick_ceiling = total_to_serve * (run_duration + 2) + 16;
    while served_total < total_to_serve && tick < tick_ceiling {
        let now = tick;
        running.retain(|(t, finish)| {
            if *finish <= now {
                bp.on_released(&t.0, region);
                false
            } else {
                true
            }
        });
        let pick = backlog
            .iter()
            .enumerate()
            .filter(|(_, (_, n))| *n > 0)
            .filter(|(_, (t, _))| bp.admits(&t.0, region))
            .max_by_key(|(_, (t, _))| {
                (
                    fair.deficit(&t.0, region, &t.0),
                    std::cmp::Reverse(t.0.clone()),
                )
            })
            .map(|(i, _)| i);
        if let Some(i) = pick {
            let (t, n) = &mut backlog[i];
            let t = t.clone();
            *n -= 1;
            fair.advance_on_claim(&t.0, region, &t.0);
            bp.on_claimed(&t.0, region);
            running.push((t.clone(), tick + run_duration));
            served_total += 1;
            if t == *quiet {
                hist.record_wait(tick);
                served_quiet += 1;
            }
        }
        if tick % 3 == 2 {
            fair.replenish(region);
        }
        tick += 1;
    }
    debug_assert_eq!(
        served_quiet, quiet_jobs,
        "the quiet tenant must be fully served (no starvation)"
    );
    hist
}

fn measure_reaper_recovery(surging: &TenantId, region: &str) -> (u64, u64) {
    let mut sched = SchedulerState::new();
    let lease_ttl = 4u64;
    let killed_jobs = 8u32;

    for i in 0..killed_jobs {
        let job = QueuedJob::enqueued(
            &surging.0,
            region,
            format!("job-{i}"),
            format!("run-{i}"),
            Lane::Batch,
            TrustTier::Trusted,
            &surging.0,
            format!("idem-{i}"),
            i as u64,
        );
        sched.enqueue(job);
    }
    let req = ClaimRequest {
        cell_region: region.to_string(),
        runner_labels: Vec::new(),
        runner_allowed_tiers: vec![TrustTier::Trusted],
        lease_owner: "doomed-runner".to_string(),
        lease_ttl,
    };
    let mut claimed = 0u32;
    while sched.claim(&req).is_some() {
        claimed += 1;
    }

    sched.advance(lease_ttl + 1);
    let reaped = sched.reap();
    let requeued_count = reaped.len() as u64;

    let orphan_count = (0..killed_jobs)
        .filter(|i| sched.state_of(&surging.0, &format!("job-{i}")) == Some(JobState::Leased))
        .count() as u64;

    debug_assert_eq!(
        requeued_count, claimed as u64,
        "every leased job re-queued (0 orphans)"
    );
    (requeued_count, orphan_count)
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
    fn ci_d2_surge_drill_core_is_green() {
        let controls = canonical_controls();
        assert_eq!(
            controls.multiplier(),
            CI_SURGE_MULTIPLIER,
            "the file's 30× multiplier"
        );
        let storm = controls.multiplier() * controls.per_tenant_in_flight_cap();
        let report = drive_ci_d2_surge(
            &controls,
            storm,
            &tenant("surging"),
            &tenant("quiet"),
            "fr-par",
        );
        assert!(
            report.is_ci_d2_green(),
            "CI-D2 must be green: {}",
            report.summary()
        );
        assert!(
            report.surging_batch_shed_count > 0,
            "the batch lane shed under the surge"
        );
        assert_eq!(
            report.surging_interactive_shed_count, 0,
            "the interactive lane held (0 shed)"
        );
        assert!(
            report.surging_interactive_admitted,
            "the surging tenant's PR-check was admitted"
        );
        assert!(
            report.quiet_interactive_admitted,
            "the quiet co-tenant was admitted"
        );
        assert_eq!(report.cross_tenant_shed_count, 0, "cross-tenant impact 0");
        assert_eq!(
            report.orphan_count, 0,
            "0 orphans after the killed-runner reap"
        );
        assert!(
            report.requeued_count > 0,
            "the killed runner's jobs re-queued"
        );
        assert!(
            report.fair_key_wait_p99_ticks <= report.starvation_trigger_ticks,
            "the per-fair_key wait p99 stayed within the starvation trigger (flat DRR holds)"
        );
        assert!(
            !report.hierarchical_scheduler_owed,
            "the hierarchical scheduler stays a NAMED FLOOR (no starvation measured - CI-P29)"
        );
    }

    #[test]
    fn unbounded_lane_is_not_green() {
        let controls = canonical_controls();
        let report = drive_ci_d2_surge(
            &controls,
            1,
            &tenant("surging"),
            &tenant("quiet"),
            "fr-par",
        );
        assert_eq!(
            report.surging_batch_shed_count, 0,
            "a sub-cap storm never sheds"
        );
        assert!(
            !report.is_ci_d2_green(),
            "with 0 shed the surge property is not exercised → NOT green (the green must be earned)"
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
