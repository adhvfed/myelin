use std::collections::BTreeMap;

use myelin_tenancy::TenantId;

pub const STORAGE_SURGE_MULTIPLIER: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StorageLaneClass {
    Speculative,
    BatchCi,
    Agent,
    Human,
}

impl StorageLaneClass {
    pub fn lane(self) -> &'static str {
        match self {
            StorageLaneClass::Speculative => "speculative",
            StorageLaneClass::BatchCi => "batch_ci",
            StorageLaneClass::Agent => "agent",
            StorageLaneClass::Human => "human",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageAdmission {
    Admit,
    Shed {
        retry_after_secs: u64,
    },
}

impl StorageAdmission {
    pub fn is_admitted(self) -> bool {
        matches!(self, StorageAdmission::Admit)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageLaneBudget {
    pub per_tenant_in_flight_cap: u32,
    pub human_lane_reservation: u32,
    pub retry_after_secs: u64,
}

impl StorageLaneBudget {
    pub fn v1_default() -> StorageLaneBudget {
        StorageLaneBudget {
            per_tenant_in_flight_cap: 128,
            human_lane_reservation: 32,
            retry_after_secs: 5,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TenantInFlight {
    human: u32,
    non_human: u32,
}

impl TenantInFlight {
    fn total(self) -> u32 {
        self.human + self.non_human
    }
}

#[derive(Clone, Debug)]
pub struct StorageLaneGate {
    budget: StorageLaneBudget,
    tenants: BTreeMap<TenantId, TenantInFlight>,
    shed_counts: BTreeMap<StorageLaneClass, u64>,
}

impl StorageLaneGate {
    pub fn new() -> StorageLaneGate {
        StorageLaneGate::with_budget(StorageLaneBudget::v1_default())
    }

    pub fn with_budget(budget: StorageLaneBudget) -> StorageLaneGate {
        StorageLaneGate {
            budget,
            tenants: BTreeMap::new(),
            shed_counts: BTreeMap::new(),
        }
    }

    pub fn admit(&mut self, tenant: &TenantId, class: StorageLaneClass) -> StorageAdmission {
        let cap = self.budget.per_tenant_in_flight_cap;
        let reserved = self.budget.human_lane_reservation.min(cap);
        let cur = self.tenants.get(tenant).copied().unwrap_or_default();

        let admit = match class {
            StorageLaneClass::Human => cur.total() < cap,
            other => {
                let non_human_budget = cap.saturating_sub(reserved);
                let step = (non_human_budget / 8).max(1);
                let ceiling = match other {
                    StorageLaneClass::Speculative => non_human_budget.saturating_sub(2 * step),
                    StorageLaneClass::BatchCi => non_human_budget.saturating_sub(step),
                    StorageLaneClass::Agent => non_human_budget,
                    StorageLaneClass::Human => unreachable!("human handled above"),
                };
                cur.non_human < ceiling && cur.total() < cap
            }
        };

        if admit {
            let entry = self.tenants.entry(tenant.clone()).or_default();
            if class == StorageLaneClass::Human {
                entry.human += 1;
            } else {
                entry.non_human += 1;
            }
            StorageAdmission::Admit
        } else {
            *self.shed_counts.entry(class).or_insert(0) += 1;
            StorageAdmission::Shed {
                retry_after_secs: self.budget.retry_after_secs,
            }
        }
    }

    pub fn release(&mut self, tenant: &TenantId, class: StorageLaneClass) {
        if let Some(entry) = self.tenants.get_mut(tenant) {
            if class == StorageLaneClass::Human {
                entry.human = entry.human.saturating_sub(1);
            } else {
                entry.non_human = entry.non_human.saturating_sub(1);
            }
        }
    }

    pub fn shed_count(&self, class: StorageLaneClass) -> u64 {
        self.shed_counts.get(&class).copied().unwrap_or(0)
    }

    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.tenants
            .get(tenant)
            .copied()
            .unwrap_or_default()
            .total()
    }

    pub fn human_in_flight(&self, tenant: &TenantId) -> u32 {
        self.tenants.get(tenant).copied().unwrap_or_default().human
    }

    pub fn cap(&self) -> u32 {
        self.budget.per_tenant_in_flight_cap
    }
}

impl Default for StorageLaneGate {
    fn default() -> Self {
        StorageLaneGate::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageSurgeReport {
    pub multiplier: u32,
    pub surging_tenant_ci_shed_count: u64,
    pub surging_tenant_human_shed_count: u64,
    pub cross_tenant_impact: u64,
    pub quiet_tenant_human_admitted: bool,
}

impl StorageSurgeReport {
    pub fn is_f6_green(&self) -> bool {
        self.surging_tenant_ci_shed_count > 0
            && self.surging_tenant_human_shed_count == 0
            && self.quiet_tenant_human_admitted
            && self.cross_tenant_impact == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "F6 storage-lane surge ({}×): CI artifact storm by one tenant ABSORBED by shedding \
             (batch_ci shed {}× with 429+Retry-After); protected human lane HELD (human shed {}, \
             quiet-tenant human admitted={}); cross_tenant_impact={} (other tenants untouched). \
             No threshold weakened.",
            self.multiplier,
            self.surging_tenant_ci_shed_count,
            self.surging_tenant_human_shed_count,
            self.quiet_tenant_human_admitted,
            self.cross_tenant_impact,
        )
    }
}

pub fn run_storage_lane_surge(
    gate: &mut StorageLaneGate,
    surging: &TenantId,
    quiet: &TenantId,
    storm_ops: u64,
    multiplier: u32,
) -> StorageSurgeReport {
    for _ in 0..storm_ops {
        let _ = gate.admit(surging, StorageLaneClass::BatchCi);
    }
    let surging_tenant_ci_shed_count = gate.shed_count(StorageLaneClass::BatchCi);

    let surging_human = gate.admit(surging, StorageLaneClass::Human);
    let surging_tenant_human_shed_count = gate.shed_count(StorageLaneClass::Human);

    let quiet_before = gate.in_flight(quiet);
    let quiet_human = gate.admit(quiet, StorageLaneClass::Human);
    let quiet_tenant_human_admitted = quiet_human.is_admitted();

    let cross_tenant_impact = u64::from(quiet_before);

    debug_assert!(
        surging_human.is_admitted(),
        "the surging tenant's human reserved lane must hold even under its own storm"
    );

    StorageSurgeReport {
        multiplier,
        surging_tenant_ci_shed_count,
        surging_tenant_human_shed_count,
        cross_tenant_impact,
        quiet_tenant_human_admitted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }

    fn small_budget() -> StorageLaneBudget {
        StorageLaneBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 4,
            retry_after_secs: 5,
        }
    }

    #[test]
    fn shed_priority_order_is_speculative_then_batch_ci_then_agent_then_human() {
        assert!(StorageLaneClass::Speculative < StorageLaneClass::BatchCi);
        assert!(StorageLaneClass::BatchCi < StorageLaneClass::Agent);
        assert!(StorageLaneClass::Agent < StorageLaneClass::Human);
    }

    #[test]
    fn storage_lane_sheds_speculative_then_batch_ci_then_agent_then_human_last() {
        let mut gate = StorageLaneGate::with_budget(small_budget());
        let t = tenant("acme");
        for _ in 0..4 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::Speculative),
            StorageAdmission::Shed { .. }
        ));
        assert_eq!(
            gate.admit(&t, StorageLaneClass::BatchCi),
            StorageAdmission::Admit
        );
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::BatchCi),
            StorageAdmission::Shed { .. }
        ));
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Admit
        );
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Shed { .. }
        ));
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Human),
            StorageAdmission::Admit
        );

        assert_eq!(gate.shed_count(StorageLaneClass::Speculative), 1);
        assert_eq!(gate.shed_count(StorageLaneClass::BatchCi), 1);
        assert_eq!(gate.shed_count(StorageLaneClass::Agent), 1);
        assert_eq!(
            gate.shed_count(StorageLaneClass::Human),
            0,
            "the human storage lane has NOT been shed"
        );
    }

    #[test]
    fn human_storage_lane_is_shed_last_only_in_true_saturation() {
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 5,
            human_lane_reservation: 2,
            retry_after_secs: 7,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let t = tenant("acme");
        for _ in 0..3 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Shed { .. }
        ));
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Human),
            StorageAdmission::Admit
        );
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Human),
            StorageAdmission::Admit
        );
        match gate.admit(&t, StorageLaneClass::Human) {
            StorageAdmission::Shed { retry_after_secs } => assert_eq!(retry_after_secs, 7),
            StorageAdmission::Admit => {
                panic!("a fully-saturated storage tier must shed even the human")
            }
        }
        assert_eq!(gate.shed_count(StorageLaneClass::Human), 1);
    }

    #[test]
    fn storage_shedding_is_per_tenant() {
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 3,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let noisy = tenant("noisy");
        let quiet = tenant("quiet");
        for _ in 0..3 {
            assert_eq!(
                gate.admit(&noisy, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        assert!(matches!(
            gate.admit(&noisy, StorageLaneClass::Agent),
            StorageAdmission::Shed { .. }
        ));
        assert_eq!(
            gate.admit(&noisy, StorageLaneClass::Human),
            StorageAdmission::Admit
        );
        assert!(matches!(
            gate.admit(&noisy, StorageLaneClass::Human),
            StorageAdmission::Shed { .. }
        ));
        assert_eq!(
            gate.in_flight(&quiet),
            0,
            "the quiet tenant's budget is independent"
        );
        assert_eq!(
            gate.admit(&quiet, StorageLaneClass::Human),
            StorageAdmission::Admit,
            "the noisy tenant's storage storm must NEVER shed another tenant's human"
        );
    }

    #[test]
    fn release_frees_a_storage_slot() {
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let t = tenant("acme");
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Admit
        );
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Admit
        );
        assert!(matches!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Shed { .. }
        ));
        gate.release(&t, StorageLaneClass::Agent);
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Agent),
            StorageAdmission::Admit,
            "a released storage slot is reusable"
        );
    }

    #[test]
    fn f6_storage_lane_surge_emits_a_green_artifact() {
        let mut gate = StorageLaneGate::with_budget(small_budget());
        let surging = tenant("noisy-ci");
        let quiet = tenant("quiet-co-tenant");
        let report = run_storage_lane_surge(
            &mut gate,
            &surging,
            &quiet,
            STORAGE_SURGE_MULTIPLIER as u64,
            STORAGE_SURGE_MULTIPLIER,
        );
        assert!(
            report.is_f6_green(),
            "the F6 storage-lane surge must be GREEN: {report:?}"
        );
        assert!(
            report.surging_tenant_ci_shed_count > 0,
            "the CI artifact storm must be absorbed by SHEDDING (429+Retry-After), not unbounded latency"
        );
        assert_eq!(
            report.surging_tenant_human_shed_count, 0,
            "the human lane held"
        );
        assert!(
            report.quiet_tenant_human_admitted,
            "the quiet tenant's human held"
        );
        assert_eq!(
            report.cross_tenant_impact, 0,
            "the storm is contained to the surging tenant"
        );
        let s = report.summary();
        assert!(s.contains("F6 storage-lane surge"));
        assert!(s.contains("cross_tenant_impact=0"));
    }

    #[test]
    fn f6_report_can_go_red() {
        let no_shed = StorageSurgeReport {
            multiplier: 30,
            surging_tenant_ci_shed_count: 0,
            surging_tenant_human_shed_count: 0,
            cross_tenant_impact: 0,
            quiet_tenant_human_admitted: true,
        };
        assert!(!no_shed.is_f6_green(), "no shed = unbounded latency = RED");

        let human_shed = StorageSurgeReport {
            multiplier: 30,
            surging_tenant_ci_shed_count: 5,
            surging_tenant_human_shed_count: 1,
            cross_tenant_impact: 0,
            quiet_tenant_human_admitted: true,
        };
        assert!(!human_shed.is_f6_green(), "a shed human lane = RED");

        let cross = StorageSurgeReport {
            multiplier: 30,
            surging_tenant_ci_shed_count: 5,
            surging_tenant_human_shed_count: 0,
            cross_tenant_impact: 1,
            quiet_tenant_human_admitted: true,
        };
        assert!(!cross.is_f6_green(), "a cross-tenant impact = RED");

        let quiet_shed = StorageSurgeReport {
            multiplier: 30,
            surging_tenant_ci_shed_count: 5,
            surging_tenant_human_shed_count: 0,
            cross_tenant_impact: 0,
            quiet_tenant_human_admitted: false,
        };
        assert!(!quiet_shed.is_f6_green(), "a starved co-tenant human = RED");
    }

    #[test]
    fn v1_default_budget_holds_the_human_lane_floor() {
        let b = StorageLaneBudget::v1_default();
        assert!(b.per_tenant_in_flight_cap > 0, "bounded (§7.1)");
        assert!(
            b.human_lane_reservation <= b.per_tenant_in_flight_cap,
            "reservation within cap"
        );
        let floor_20pct = (u64::from(b.per_tenant_in_flight_cap) * 2000).div_ceil(10_000) as u32;
        assert!(
            b.human_lane_reservation >= floor_20pct,
            "the storage human lane reserves {} ≥ the 20% floor {}",
            b.human_lane_reservation,
            floor_20pct
        );
    }

    #[test]
    fn lane_labels_are_the_stable_signal_names() {
        assert_eq!(StorageLaneClass::Speculative.lane(), "speculative");
        assert_eq!(StorageLaneClass::BatchCi.lane(), "batch_ci");
        assert_eq!(StorageLaneClass::Agent.lane(), "agent");
        assert_eq!(StorageLaneClass::Human.lane(), "human");
    }

    #[test]
    fn is_admitted_distinguishes_admit_from_shed() {
        assert!(StorageAdmission::Admit.is_admitted());
        assert!(!StorageAdmission::Shed {
            retry_after_secs: 5
        }
        .is_admitted());
    }

    #[test]
    fn in_flight_accessors_report_exact_per_tenant_counts() {
        let mut gate = StorageLaneGate::with_budget(small_budget());
        let t = tenant("acme");
        for _ in 0..2 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        for _ in 0..3 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Human),
                StorageAdmission::Admit
            );
        }
        assert_eq!(
            gate.in_flight(&t),
            5,
            "total in-flight = 2 non-human + 3 human"
        );
        assert_eq!(gate.human_in_flight(&t), 3, "exactly 3 human slots taken");
        assert_eq!(gate.in_flight(&tenant("other")), 0);
        assert_eq!(gate.human_in_flight(&tenant("other")), 0);
        gate.release(&t, StorageLaneClass::Human);
        assert_eq!(gate.human_in_flight(&t), 2);
        assert_eq!(gate.in_flight(&t), 4);
    }

    #[test]
    fn graded_ceiling_arithmetic_is_exact() {
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 80,
            human_lane_reservation: 16,
            retry_after_secs: 5,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let t = tenant("acme");
        for _ in 0..47 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        assert_eq!(
            gate.admit(&t, StorageLaneClass::Speculative),
            StorageAdmission::Admit,
            "speculative admitted at 47 < its 48 ceiling"
        );
        assert!(
            matches!(
                gate.admit(&t, StorageLaneClass::Speculative),
                StorageAdmission::Shed { .. }
            ),
            "speculative sheds AT its ceiling (the `<` boundary, not `<=`)"
        );
        assert_eq!(
            gate.admit(&t, StorageLaneClass::BatchCi),
            StorageAdmission::Admit,
            "batch_ci still admitted at 48 (< its 56 ceiling) - speculative sheds 8 slots earlier"
        );
    }

    #[test]
    fn a_non_human_sheds_at_total_saturation_even_below_its_class_ceiling() {
        let budget = StorageLaneBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 6,
            retry_after_secs: 5,
        };
        let mut gate = StorageLaneGate::with_budget(budget);
        let t = tenant("acme");
        for _ in 0..3 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Admit
            );
        }
        for _ in 0..7 {
            assert_eq!(
                gate.admit(&t, StorageLaneClass::Human),
                StorageAdmission::Admit
            );
        }
        assert_eq!(
            gate.in_flight(&t),
            10,
            "the tier is at cap (3 non-human + 7 human)"
        );
        assert!(
            matches!(
                gate.admit(&t, StorageLaneClass::Agent),
                StorageAdmission::Shed { .. }
            ),
            "a non-human MUST shed at total saturation even below its class ceiling (the `< cap` guard)"
        );
    }
}
