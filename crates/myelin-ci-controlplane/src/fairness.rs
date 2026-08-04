use std::collections::BTreeMap;

use crate::scheduler::Lane;

pub const ADVANCE_DEFICIT_QUERY: &str = "\
INSERT INTO fair_deficit (tenant_id, region, fair_key, deficit, last_served)
VALUES ($1, $2, $3, -$4, now())
ON CONFLICT (tenant_id, region, fair_key) DO UPDATE
SET deficit = fair_deficit.deficit - $4,
    last_served = now()
RETURNING deficit";

pub const REPLENISH_DEFICIT_QUERY: &str = "\
UPDATE fair_deficit
SET deficit = LEAST(deficit + $2, $3)
WHERE region = $1";

pub const IN_FLIGHT_COUNT_QUERY: &str = "\
SELECT count(*) AS in_flight
FROM job_queue
WHERE tenant_id = $1
  AND region = $2
  AND state IN ('leased', 'running')";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanTier {
    Free,
    Pro,
    Enterprise,
}

impl PlanTier {
    pub fn quantum_weight(self) -> i64 {
        match self {
            PlanTier::Free => 1,
            PlanTier::Pro => 2,
            PlanTier::Enterprise => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PlanTier::Free => "free",
            PlanTier::Pro => "pro",
            PlanTier::Enterprise => "enterprise",
        }
    }
}

pub const BASE_QUANTUM: i64 = 1;

pub const DEFICIT_CEILING: i64 = 64;

pub const DEFAULT_TENANT_IN_FLIGHT_CAP: u32 = 64;

#[derive(Clone, Debug, Default)]
pub struct FairShare {
    deficits: BTreeMap<(String, String, String), i64>,
    tiers: BTreeMap<(String, String, String), PlanTier>,
}

impl FairShare {
    pub fn new() -> Self {
        FairShare::default()
    }

    fn key(tenant_id: &str, region: &str, fair_key: &str) -> (String, String, String) {
        (
            tenant_id.to_string(),
            region.to_string(),
            fair_key.to_string(),
        )
    }

    pub fn set_tier(&mut self, tenant_id: &str, region: &str, fair_key: &str, tier: PlanTier) {
        self.tiers
            .insert(Self::key(tenant_id, region, fair_key), tier);
    }

    pub fn deficit(&self, tenant_id: &str, region: &str, fair_key: &str) -> i64 {
        self.deficits
            .get(&Self::key(tenant_id, region, fair_key))
            .copied()
            .unwrap_or(0)
    }

    pub fn advance_on_claim(&mut self, tenant_id: &str, region: &str, fair_key: &str) -> i64 {
        let entry = self
            .deficits
            .entry(Self::key(tenant_id, region, fair_key))
            .or_insert(0);
        *entry -= BASE_QUANTUM;
        *entry
    }

    pub fn replenish(&mut self, region: &str) {
        let updates: Vec<((String, String, String), i64)> = self
            .deficits
            .keys()
            .filter(|(_, r, _)| r == region)
            .map(|k| {
                let weight = self.tiers.get(k).copied().unwrap_or(PlanTier::Free);
                (k.clone(), BASE_QUANTUM * weight.quantum_weight())
            })
            .collect();
        for (k, quantum) in updates {
            let d = self.deficits.entry(k).or_insert(0);
            *d = (*d + quantum).min(DEFICIT_CEILING);
        }
    }

    pub fn set_deficit(&mut self, tenant_id: &str, region: &str, fair_key: &str, deficit: i64) {
        self.deficits
            .insert(Self::key(tenant_id, region, fair_key), deficit);
    }
}

#[derive(Clone, Debug)]
pub struct Backpressure {
    cap: u32,
    in_flight: BTreeMap<(String, String), u32>,
    backpressured: BTreeMap<(String, String), u64>,
}

impl Default for Backpressure {
    fn default() -> Self {
        Backpressure::with_cap(DEFAULT_TENANT_IN_FLIGHT_CAP)
    }
}

impl Backpressure {
    pub fn new() -> Self {
        Backpressure::default()
    }

    pub fn with_cap(cap: u32) -> Self {
        Backpressure {
            cap,
            in_flight: BTreeMap::new(),
            backpressured: BTreeMap::new(),
        }
    }

    fn key(tenant_id: &str, region: &str) -> (String, String) {
        (tenant_id.to_string(), region.to_string())
    }

    pub fn in_flight(&self, tenant_id: &str, region: &str) -> u32 {
        self.in_flight
            .get(&Self::key(tenant_id, region))
            .copied()
            .unwrap_or(0)
    }

    pub fn admits(&self, tenant_id: &str, region: &str) -> bool {
        self.in_flight(tenant_id, region) < self.cap
    }

    pub fn on_claimed(&mut self, tenant_id: &str, region: &str) -> u32 {
        let e = self
            .in_flight
            .entry(Self::key(tenant_id, region))
            .or_insert(0);
        *e += 1;
        *e
    }

    pub fn on_released(&mut self, tenant_id: &str, region: &str) -> u32 {
        let e = self
            .in_flight
            .entry(Self::key(tenant_id, region))
            .or_insert(0);
        *e = e.saturating_sub(1);
        *e
    }

    pub fn record_backpressured(&mut self, tenant_id: &str, region: &str) {
        *self
            .backpressured
            .entry(Self::key(tenant_id, region))
            .or_insert(0) += 1;
    }

    pub fn backpressured_count(&self, tenant_id: &str, region: &str) -> u64 {
        self.backpressured
            .get(&Self::key(tenant_id, region))
            .copied()
            .unwrap_or(0)
    }
}

pub fn shed_order() -> [Lane; 3] {
    [Lane::Deploy, Lane::Batch, Lane::Interactive]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_on_claim_decrements_served_deficit() {
        let mut f = FairShare::new();
        assert_eq!(f.deficit("t", "fr-par", "t"), 0, "unseen key defaults to 0");
        let after = f.advance_on_claim("t", "fr-par", "t");
        assert_eq!(
            after, -BASE_QUANTUM,
            "the served key is decremented by one quantum"
        );
        assert_eq!(f.deficit("t", "fr-par", "t"), -BASE_QUANTUM);
        assert_eq!(f.advance_on_claim("t", "fr-par", "t"), -2 * BASE_QUANTUM);
    }

    #[test]
    fn replenish_is_plan_weighted() {
        let mut f = FairShare::new();
        for (k, tier) in [
            ("free", PlanTier::Free),
            ("pro", PlanTier::Pro),
            ("ent", PlanTier::Enterprise),
        ] {
            f.set_tier("t", "fr-par", k, tier);
            f.advance_on_claim("t", "fr-par", k);
        }
        f.replenish("fr-par");
        assert_eq!(f.deficit("t", "fr-par", "free"), 0, "Free replenishes by 1");
        assert_eq!(f.deficit("t", "fr-par", "pro"), 1, "Pro replenishes by 2");
        assert_eq!(
            f.deficit("t", "fr-par", "ent"),
            3,
            "Enterprise replenishes by 4 (the largest share)"
        );
    }

    #[test]
    fn replenish_clamps_at_ceiling() {
        let mut f = FairShare::new();
        f.set_tier("t", "fr-par", "k", PlanTier::Enterprise);
        f.set_deficit("t", "fr-par", "k", DEFICIT_CEILING - 1);
        f.replenish("fr-par");
        assert_eq!(
            f.deficit("t", "fr-par", "k"),
            DEFICIT_CEILING,
            "the deficit never exceeds the burst-credit ceiling"
        );
        f.replenish("fr-par");
        assert_eq!(f.deficit("t", "fr-par", "k"), DEFICIT_CEILING);
    }

    #[test]
    fn replenish_is_region_scoped() {
        let mut f = FairShare::new();
        f.advance_on_claim("t", "fr-par", "k");
        f.advance_on_claim("t", "nl-ams", "k");
        f.replenish("fr-par");
        assert_eq!(f.deficit("t", "fr-par", "k"), 0, "fr-par replenished");
        assert_eq!(
            f.deficit("t", "nl-ams", "k"),
            -1,
            "nl-ams untouched by an fr-par sweep (region-scoped, no cross-region bleed)"
        );
    }

    #[test]
    fn untiered_key_replenishes_at_free_base() {
        let mut f = FairShare::new();
        f.advance_on_claim("t", "fr-par", "k");
        f.replenish("fr-par");
        assert_eq!(
            f.deficit("t", "fr-par", "k"),
            0,
            "an untiered key replenishes by the Free base quantum (1), not more"
        );
    }

    #[test]
    fn plan_tier_weights_are_bounded_and_ordered() {
        assert_eq!(PlanTier::Free.quantum_weight(), 1);
        assert_eq!(PlanTier::Pro.quantum_weight(), 2);
        assert_eq!(PlanTier::Enterprise.quantum_weight(), 4);
        assert!(
            PlanTier::Free.quantum_weight() < PlanTier::Pro.quantum_weight()
                && PlanTier::Pro.quantum_weight() < PlanTier::Enterprise.quantum_weight(),
            "the tiers are strictly ordered (a paid tier recovers faster), and bounded (no ∞ share)"
        );
        assert_eq!(PlanTier::Pro.as_str(), "pro");
    }

    #[test]
    fn backpressure_admits_to_cap_then_holds() {
        let mut bp = Backpressure::with_cap(3);
        assert!(bp.admits("t", "fr-par"), "empty → admits");
        bp.on_claimed("t", "fr-par");
        bp.on_claimed("t", "fr-par");
        assert!(bp.admits("t", "fr-par"), "2 < 3 → still admits");
        bp.on_claimed("t", "fr-par");
        assert!(
            !bp.admits("t", "fr-par"),
            "at the cap the tenant is over-cap - the claim holds (queues gracefully)"
        );
        assert_eq!(bp.in_flight("t", "fr-par"), 3);
        bp.on_released("t", "fr-par");
        assert!(bp.admits("t", "fr-par"), "headroom re-opens on release");
    }

    #[test]
    fn backpressure_cap_is_per_tenant() {
        let mut bp = Backpressure::with_cap(2);
        bp.on_claimed("noisy", "fr-par");
        bp.on_claimed("noisy", "fr-par");
        assert!(
            !bp.admits("noisy", "fr-par"),
            "the noisy tenant is over-cap"
        );
        assert!(
            bp.admits("quiet", "fr-par"),
            "a different tenant is unaffected (per-tenant blast radius)"
        );
    }

    #[test]
    fn backpressure_release_saturates_at_zero() {
        let mut bp = Backpressure::with_cap(4);
        assert_eq!(bp.on_released("t", "fr-par"), 0, "release on empty stays 0");
        bp.on_claimed("t", "fr-par");
        assert_eq!(bp.on_released("t", "fr-par"), 0);
        assert_eq!(bp.on_released("t", "fr-par"), 0, "still 0, no underflow");
    }

    #[test]
    fn backpressure_count_is_a_per_tenant_signal() {
        let mut bp = Backpressure::new();
        assert_eq!(bp.backpressured_count("t", "fr-par"), 0);
        bp.record_backpressured("t", "fr-par");
        bp.record_backpressured("t", "fr-par");
        assert_eq!(bp.backpressured_count("t", "fr-par"), 2);
        assert_eq!(bp.backpressured_count("other", "fr-par"), 0, "per-tenant");
    }

    #[test]
    fn default_cap_matches_substrate_ci_dispatch_budget() {
        let substrate = myelin_substrate::ShedBudgetTable::v1_floor()
            .budget(myelin_substrate::ShedSurface::CiDispatch)
            .per_tenant_in_flight_cap;
        assert_eq!(
            DEFAULT_TENANT_IN_FLIGHT_CAP, substrate,
            "the CI scheduler cap MUST equal the substrate CiDispatch shed budget (one v1 floor)"
        );
    }

    #[test]
    fn lane_shed_order_holds_interactive_last() {
        let order = shed_order();
        assert_eq!(order, [Lane::Deploy, Lane::Batch, Lane::Interactive]);
        assert_eq!(
            *order.last().unwrap(),
            Lane::Interactive,
            "interactive is shed LAST (the protected human lane inside CI)"
        );
        let priorities: Vec<i32> = order.iter().map(|l| l.priority()).collect();
        assert!(
            priorities.windows(2).all(|w| w[0] < w[1]),
            "the shed order ascends in claim priority (lowest-priority lane shed first)"
        );
    }

    #[test]
    fn fairness_no_starvation_interactive_holds() {
        let region = "fr-par";
        let mut f = FairShare::new();
        let tenants = ["hot", "quiet1", "quiet2"];
        for t in tenants {
            f.set_tier(t, region, t, PlanTier::Free);
        }
        let mut backlog: BTreeMap<&str, u32> =
            [("hot", 10_000u32), ("quiet1", 5), ("quiet2", 5)].into();
        let mut served: BTreeMap<&str, u32> = [("hot", 0u32), ("quiet1", 0), ("quiet2", 0)].into();

        let window = 60;
        for round in 0..window {
            let pick = tenants
                .iter()
                .filter(|t| backlog[*t] > 0)
                .max_by_key(|t| (f.deficit(t, region, t), std::cmp::Reverse(**t)))
                .copied();
            if let Some(t) = pick {
                *backlog.get_mut(t).unwrap() -= 1;
                *served.get_mut(t).unwrap() += 1;
                f.advance_on_claim(t, region, t);
            }
            if round % 3 == 2 {
                f.replenish(region);
            }
        }

        assert_eq!(
            served["quiet1"], 5,
            "quiet1 is fully served - DRR did not let the hot tenant starve it"
        );
        assert_eq!(
            served["quiet2"], 5,
            "quiet2 is fully served - no starvation under a 10k-matrix neighbour"
        );
        assert!(
            served["hot"] > 0 && served["hot"] < window,
            "the hot tenant progresses but is fairly interleaved, never monopolising"
        );

        assert!(
            Lane::Interactive.priority() > Lane::Batch.priority()
                && Lane::Batch.priority() > Lane::Deploy.priority(),
            "the interactive lane outranks batch/deploy strictly - fairness never reorders across lanes"
        );
        assert_eq!(*shed_order().last().unwrap(), Lane::Interactive);
    }

    #[test]
    fn the_live_fairness_sql_matches_the_model() {
        assert!(
            ADVANCE_DEFICIT_QUERY.contains("ON CONFLICT")
                && ADVANCE_DEFICIT_QUERY.contains("deficit = fair_deficit.deficit - $4"),
            "ADVANCE UPSERTs a per-fair_key decrement (the served key drops in deficit DESC)"
        );
        assert!(
            REPLENISH_DEFICIT_QUERY.contains("LEAST(deficit + $2, $3)")
                && REPLENISH_DEFICIT_QUERY.contains("WHERE region = $1"),
            "REPLENISH adds a (plan-weighted) quantum clamped at the ceiling, region-scoped"
        );
        assert!(
            IN_FLIGHT_COUNT_QUERY.contains("state IN ('leased', 'running')")
                && IN_FLIGHT_COUNT_QUERY.contains("tenant_id = $1"),
            "the in-flight count is the tenant's leased+running jobs (the bounded run-queue load)"
        );
    }
}
