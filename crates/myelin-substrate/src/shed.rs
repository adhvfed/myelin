use myelin_identity::PrincipalKind;
use myelin_tenancy::TenantId;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunClass {
    Speculative,
    BatchCi,
    Agent,
    Human,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunClassHeader {
    Speculative,
    BatchCi,
}

impl RunClass {
    pub fn derive(kind: &PrincipalKind, header: Option<RunClassHeader>) -> RunClass {
        let ceiling = match kind {
            PrincipalKind::Human => RunClass::Human,
            PrincipalKind::Agent { .. } => RunClass::Agent,
            PrincipalKind::Service => RunClass::BatchCi,
        };
        let requested = match header {
            None => ceiling,
            Some(RunClassHeader::Speculative) => RunClass::Speculative,
            Some(RunClassHeader::BatchCi) => RunClass::BatchCi,
        };
        requested.min(ceiling)
    }

    pub fn lane(self) -> &'static str {
        match self {
            RunClass::Speculative => "speculative",
            RunClass::BatchCi => "batch_ci",
            RunClass::Agent => "agent",
            RunClass::Human => "human",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShedDecision {
    Admit,
    Shed { retry_after_secs: u64 },
}

impl ShedDecision {
    pub fn is_admitted(self) -> bool {
        matches!(self, ShedDecision::Admit)
    }
}

#[derive(Clone, Debug)]
pub struct BoundedQueue {
    in_flight: u32,
    capacity: u32,
    shed_count: u64,
}

impl BoundedQueue {
    pub fn new(capacity: u32) -> BoundedQueue {
        BoundedQueue {
            in_flight: 0,
            capacity,
            shed_count: 0,
        }
    }

    pub fn try_acquire(&mut self) -> bool {
        if self.in_flight < self.capacity {
            self.in_flight += 1;
            true
        } else {
            self.shed_count += 1;
            false
        }
    }

    pub fn release(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    pub fn shed_count(&self) -> u64 {
        self.shed_count
    }
}

#[derive(Clone, Debug)]
pub struct ShedBudgetTable {
    rows: HashMap<Surface, SurfaceBudget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceBudget {
    pub per_tenant_in_flight_cap: u32,
    pub human_lane_reservation: u32,
    pub retry_after_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShedBudgetError {
    Unbounded(Surface),
    ReservationOverCap {
        surface: Surface,
        reservation: u32,
        cap: u32,
    },
    HumanLaneStarved {
        surface: Surface,
        reservation: u32,
        floor: u32,
        cap: u32,
    },
}

impl std::fmt::Display for ShedBudgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShedBudgetError::Unbounded(s) => {
                write!(f, "shed budget for {s:?} is unbounded (cap == 0) - every surface must be bounded (§7.1)")
            }
            ShedBudgetError::ReservationOverCap { surface, reservation, cap } => write!(
                f,
                "shed budget for {surface:?} reserves {reservation} of a cap of {cap} - the reservation cannot exceed the cap"
            ),
            ShedBudgetError::HumanLaneStarved { surface, reservation, floor, cap } => write!(
                f,
                "shed budget for {surface:?} reserves {reservation} of {cap} - BELOW the measured human-lane floor {floor}: \
                 the human lane would be starved under surge. You cannot tune the human lane into starvation (P-S33, EI-01 §3)."
            ),
        }
    }
}

impl std::error::Error for ShedBudgetError {}

impl SurfaceBudget {
    pub const HUMAN_LANE_FLOOR_BPS: u32 = 2000;

    pub fn human_lane_floor(cap: u32) -> u32 {
        let frac = (u64::from(cap) * u64::from(Self::HUMAN_LANE_FLOOR_BPS)).div_ceil(10_000) as u32;
        frac.max(1)
    }

    pub fn validate_tuned(&self, surface: Surface) -> Result<(), ShedBudgetError> {
        let cap = self.per_tenant_in_flight_cap;
        if cap == 0 {
            return Err(ShedBudgetError::Unbounded(surface));
        }
        if self.human_lane_reservation > cap {
            return Err(ShedBudgetError::ReservationOverCap {
                surface,
                reservation: self.human_lane_reservation,
                cap,
            });
        }
        if surface.reserves_human_lane() {
            let floor = Self::human_lane_floor(cap);
            if self.human_lane_reservation < floor {
                return Err(ShedBudgetError::HumanLaneStarved {
                    surface,
                    reservation: self.human_lane_reservation,
                    floor,
                    cap,
                });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    CiDispatch,
    CollabOpStream,
    ConnectionTier,
    AgentMention,
    GitFrontDoor,
    RefsBacklinkRead,
    RefsRefCreate,
    SearchQuery,
    WorkflowAgentLane,
    HttpIntake,
}

impl Surface {
    pub fn reserves_human_lane(self) -> bool {
        match self {
            Surface::CiDispatch => false,
            Surface::CollabOpStream
            | Surface::ConnectionTier
            | Surface::AgentMention
            | Surface::GitFrontDoor
            | Surface::RefsBacklinkRead
            | Surface::RefsRefCreate
            | Surface::SearchQuery
            | Surface::WorkflowAgentLane
            | Surface::HttpIntake => true,
        }
    }
}

impl ShedBudgetTable {
    pub fn v1_floor() -> ShedBudgetTable {
        let mut rows = HashMap::new();
        rows.insert(
            Surface::CiDispatch,
            SurfaceBudget {
                per_tenant_in_flight_cap: 64,
                human_lane_reservation: 0,
                retry_after_secs: 5,
            },
        );
        rows.insert(
            Surface::CollabOpStream,
            SurfaceBudget {
                per_tenant_in_flight_cap: 128,
                human_lane_reservation: 32,
                retry_after_secs: 2,
            },
        );
        rows.insert(
            Surface::ConnectionTier,
            SurfaceBudget {
                per_tenant_in_flight_cap: 256,
                human_lane_reservation: 64,
                retry_after_secs: 3,
            },
        );
        rows.insert(
            Surface::AgentMention,
            SurfaceBudget {
                per_tenant_in_flight_cap: 96,
                human_lane_reservation: 24,
                retry_after_secs: 10,
            },
        );
        rows.insert(
            Surface::GitFrontDoor,
            SurfaceBudget {
                // sized to the edge's 4 global git-wire slots: machines hold at
                // most 3 per tenant, so a human fetch always has a slot.
                per_tenant_in_flight_cap: 4,
                human_lane_reservation: 1,
                retry_after_secs: 5,
            },
        );
        rows.insert(
            Surface::RefsBacklinkRead,
            SurfaceBudget {
                per_tenant_in_flight_cap: 192,
                human_lane_reservation: 48,
                retry_after_secs: 3,
            },
        );
        rows.insert(
            Surface::RefsRefCreate,
            SurfaceBudget {
                per_tenant_in_flight_cap: 96,
                human_lane_reservation: 24,
                retry_after_secs: 5,
            },
        );
        rows.insert(
            Surface::SearchQuery,
            SurfaceBudget {
                per_tenant_in_flight_cap: 160,
                human_lane_reservation: 40,
                retry_after_secs: 3,
            },
        );
        rows.insert(
            Surface::WorkflowAgentLane,
            SurfaceBudget {
                per_tenant_in_flight_cap: 96,
                human_lane_reservation: 24,
                retry_after_secs: 10,
            },
        );
        rows.insert(
            Surface::HttpIntake,
            SurfaceBudget {
                // sized under the edge's 64 global dispatch slots: one tenant's
                // machine lanes hold at most 36, leaving headroom for humans
                // and other tenants before the flat backstop trips.
                per_tenant_in_flight_cap: 48,
                human_lane_reservation: 12,
                retry_after_secs: 5,
            },
        );
        ShedBudgetTable { rows }
    }

    pub fn from_rows(rows: HashMap<Surface, SurfaceBudget>) -> ShedBudgetTable {
        ShedBudgetTable { rows }
    }

    pub fn budget(&self, surface: Surface) -> SurfaceBudget {
        self.rows[&surface]
    }

    pub fn surfaces(&self) -> impl Iterator<Item = Surface> + '_ {
        self.rows.keys().copied()
    }

    pub fn validate(&self) -> Result<(), ShedBudgetError> {
        for surface in [
            Surface::CiDispatch,
            Surface::CollabOpStream,
            Surface::ConnectionTier,
            Surface::AgentMention,
            Surface::GitFrontDoor,
            Surface::RefsBacklinkRead,
            Surface::RefsRefCreate,
            Surface::SearchQuery,
            Surface::WorkflowAgentLane,
            Surface::HttpIntake,
        ] {
            if let Some(b) = self.rows.get(&surface) {
                b.validate_tuned(surface)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ShedLane {
    surface: Surface,
    budget: SurfaceBudget,
    tenants: HashMap<TenantId, TenantInFlight>,
    shed_counts: HashMap<RunClass, u64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct TenantInFlight {
    human: u32,
    non_human: u32,
}

impl TenantInFlight {
    fn total(self) -> u32 {
        self.human + self.non_human
    }
}

impl ShedLane {
    pub fn new(surface: Surface) -> ShedLane {
        ShedLane::with_budget(surface, ShedBudgetTable::v1_floor().budget(surface))
    }

    pub fn with_budget(surface: Surface, budget: SurfaceBudget) -> ShedLane {
        ShedLane {
            surface,
            budget,
            tenants: HashMap::new(),
            shed_counts: HashMap::new(),
        }
    }

    pub fn admit(&mut self, tenant: &TenantId, class: RunClass) -> ShedDecision {
        let cap = self.budget.per_tenant_in_flight_cap;
        let reserved = self.budget.human_lane_reservation.min(cap);
        let cur = self.tenants.get(tenant).copied().unwrap_or_default();

        let admit = match class {
            RunClass::Human => cur.total() < cap,
            other => {
                let non_human_budget = cap.saturating_sub(reserved);
                let step = (non_human_budget / 8).max(1);
                let ceiling = match other {
                    RunClass::Speculative => non_human_budget.saturating_sub(2 * step),
                    RunClass::BatchCi => non_human_budget.saturating_sub(step),
                    RunClass::Agent => non_human_budget,
                    RunClass::Human => unreachable!("human handled above"),
                };
                cur.non_human < ceiling && cur.total() < cap
            }
        };

        if admit {
            let entry = self.tenants.entry(tenant.clone()).or_default();
            if class == RunClass::Human {
                entry.human += 1;
            } else {
                entry.non_human += 1;
            }
            ShedDecision::Admit
        } else {
            *self.shed_counts.entry(class).or_insert(0) += 1;
            ShedDecision::Shed {
                retry_after_secs: self.budget.retry_after_secs,
            }
        }
    }

    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        if let Some(entry) = self.tenants.get_mut(tenant) {
            if class == RunClass::Human {
                entry.human = entry.human.saturating_sub(1);
            } else {
                entry.non_human = entry.non_human.saturating_sub(1);
            }
        }
    }

    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.shed_counts.get(&class).copied().unwrap_or(0)
    }

    pub fn total_shed_count(&self) -> u64 {
        self.shed_counts.values().sum()
    }

    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.tenants
            .get(tenant)
            .copied()
            .unwrap_or_default()
            .total()
    }

    pub fn surface(&self) -> Surface {
        self.surface
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalKind, RuntimeRef};

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    fn agent_kind() -> PrincipalKind {
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt".into()),
            on_behalf_of: None,
        }
    }

    #[test]
    fn derive_maps_kind_to_lane_and_never_up_classes() {
        assert_eq!(
            RunClass::derive(&PrincipalKind::Human, None),
            RunClass::Human
        );
        assert_eq!(RunClass::derive(&agent_kind(), None), RunClass::Agent);
        assert_eq!(
            RunClass::derive(&PrincipalKind::Service, None),
            RunClass::BatchCi
        );

        assert_eq!(
            RunClass::derive(&PrincipalKind::Human, Some(RunClassHeader::Speculative)),
            RunClass::Speculative
        );
        assert_eq!(
            RunClass::derive(&PrincipalKind::Human, Some(RunClassHeader::BatchCi)),
            RunClass::BatchCi
        );

        assert_eq!(
            RunClass::derive(&PrincipalKind::Service, Some(RunClassHeader::Speculative)),
            RunClass::Speculative
        );
        assert_eq!(
            RunClass::derive(&agent_kind(), Some(RunClassHeader::BatchCi)),
            RunClass::BatchCi
        );
    }

    #[test]
    fn shed_priority_order_is_speculative_then_batch_then_agent_then_human() {
        assert!(RunClass::Speculative < RunClass::BatchCi);
        assert!(RunClass::BatchCi < RunClass::Agent);
        assert!(RunClass::Agent < RunClass::Human);
    }

    #[test]
    fn shed_order_sheds_speculative_then_batch_ci_then_agent_then_human_last() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 4,
            retry_after_secs: 5,
        };
        let mut lane = ShedLane::with_budget(Surface::HttpIntake, budget);
        let t = tenant("acme");

        for _ in 0..4 {
            assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
        }
        assert!(matches!(
            lane.admit(&t, RunClass::Speculative),
            ShedDecision::Shed { .. }
        ));
        assert_eq!(lane.admit(&t, RunClass::BatchCi), ShedDecision::Admit);
        assert!(matches!(
            lane.admit(&t, RunClass::BatchCi),
            ShedDecision::Shed { .. }
        ));
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
        assert!(matches!(
            lane.admit(&t, RunClass::Agent),
            ShedDecision::Shed { .. }
        ));
        assert_eq!(lane.admit(&t, RunClass::Human), ShedDecision::Admit);

        assert_eq!(lane.shed_count(RunClass::Speculative), 1);
        assert_eq!(lane.shed_count(RunClass::BatchCi), 1);
        assert_eq!(lane.shed_count(RunClass::Agent), 1);
        assert_eq!(
            lane.shed_count(RunClass::Human),
            0,
            "the human lane has NOT been shed"
        );
    }

    #[test]
    fn human_lane_is_shed_last_only_in_true_saturation() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 5,
            human_lane_reservation: 2,
            retry_after_secs: 7,
        };
        let mut lane = ShedLane::with_budget(Surface::ConnectionTier, budget);
        let t = tenant("acme");
        for _ in 0..3 {
            assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
        }
        assert!(
            matches!(lane.admit(&t, RunClass::Agent), ShedDecision::Shed { .. }),
            "agent shed at cap-reserved"
        );
        assert_eq!(lane.admit(&t, RunClass::Human), ShedDecision::Admit);
        assert_eq!(lane.admit(&t, RunClass::Human), ShedDecision::Admit);
        match lane.admit(&t, RunClass::Human) {
            ShedDecision::Shed { retry_after_secs } => assert_eq!(retry_after_secs, 7),
            ShedDecision::Admit => panic!("a fully-saturated surface must shed even the human"),
        }
        assert_eq!(lane.shed_count(RunClass::Human), 1);
    }

    #[test]
    fn shedding_is_per_tenant_one_tenants_surge_never_sheds_anothers_human() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 3,
        };
        let mut lane = ShedLane::with_budget(Surface::HttpIntake, budget);
        let noisy = tenant("noisy");
        let quiet = tenant("quiet");

        for _ in 0..3 {
            assert_eq!(lane.admit(&noisy, RunClass::Agent), ShedDecision::Admit);
        }
        assert!(matches!(
            lane.admit(&noisy, RunClass::Agent),
            ShedDecision::Shed { .. }
        ));
        assert_eq!(lane.admit(&noisy, RunClass::Human), ShedDecision::Admit);
        assert!(
            matches!(
                lane.admit(&noisy, RunClass::Human),
                ShedDecision::Shed { .. }
            ),
            "noisy saturated"
        );

        assert_eq!(
            lane.in_flight(&quiet),
            0,
            "the quiet tenant's budget is independent"
        );
        assert_eq!(
            lane.admit(&quiet, RunClass::Human),
            ShedDecision::Admit,
            "the noisy tenant's surge must NEVER shed another tenant's human"
        );
        assert_eq!(lane.admit(&quiet, RunClass::Agent), ShedDecision::Admit);
    }

    #[test]
    fn release_frees_a_slot() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        };
        let mut lane = ShedLane::with_budget(Surface::HttpIntake, budget);
        let t = tenant("acme");
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
        assert!(matches!(
            lane.admit(&t, RunClass::Agent),
            ShedDecision::Shed { .. }
        ));
        lane.release(&t, RunClass::Agent);
        assert_eq!(
            lane.admit(&t, RunClass::Agent),
            ShedDecision::Admit,
            "a released slot is reusable"
        );
    }

    #[test]
    fn bounded_queue_fast_fails_rather_than_growing_unboundedly() {
        let mut q = BoundedQueue::new(2);
        assert!(q.try_acquire(), "first permit");
        assert!(q.try_acquire(), "second permit");
        assert!(
            !q.try_acquire(),
            "a full bounded queue fast-fails (sheds), never grows latency"
        );
        assert_eq!(
            q.in_flight(),
            2,
            "in-flight never exceeds the bound (Little's Law)"
        );
        assert_eq!(
            q.shed_count(),
            1,
            "the shed is counted (the bounded-everything signal)"
        );
        q.release();
        assert!(q.try_acquire(), "a released slot is reusable");
        assert_eq!(q.in_flight(), 2);
    }

    #[test]
    fn bounded_queue_release_saturates_at_zero() {
        let mut q = BoundedQueue::new(1);
        q.release();
        assert_eq!(q.in_flight(), 0, "a double/stray release never wraps");
        assert!(q.try_acquire());
    }

    #[test]
    fn v1_floor_table_covers_every_surface_with_a_bounded_reserved_lane() {
        let table = ShedBudgetTable::v1_floor();
        for surface in [
            Surface::CiDispatch,
            Surface::CollabOpStream,
            Surface::ConnectionTier,
            Surface::AgentMention,
            Surface::GitFrontDoor,
            Surface::RefsBacklinkRead,
            Surface::RefsRefCreate,
            Surface::SearchQuery,
            Surface::WorkflowAgentLane,
            Surface::HttpIntake,
        ] {
            let b = table.budget(surface);
            assert!(
                b.per_tenant_in_flight_cap > 0,
                "{surface:?} must be bounded (§7.1)"
            );
            assert!(
                b.human_lane_reservation <= b.per_tenant_in_flight_cap,
                "{surface:?} reservation within the cap"
            );
            assert!(
                b.retry_after_secs > 0,
                "{surface:?} sheds with a Retry-After"
            );
        }
        assert_eq!(table.budget(Surface::CiDispatch).human_lane_reservation, 0);
        assert!(table.budget(Surface::CollabOpStream).human_lane_reservation > 0);
        assert!(table.budget(Surface::ConnectionTier).human_lane_reservation > 0);
        assert!(table.budget(Surface::AgentMention).human_lane_reservation > 0);
        assert!(table.budget(Surface::GitFrontDoor).human_lane_reservation > 0);
        assert!(
            table
                .budget(Surface::RefsBacklinkRead)
                .human_lane_reservation
                > 0
        );
        assert!(table.budget(Surface::RefsRefCreate).human_lane_reservation > 0);
        assert!(table.budget(Surface::SearchQuery).human_lane_reservation > 0);
        assert!(
            table
                .budget(Surface::WorkflowAgentLane)
                .human_lane_reservation
                > 0
        );
    }

    #[test]
    fn the_tuned_table_validates_against_the_human_lane_floor() {
        let table = ShedBudgetTable::v1_floor();
        table
            .validate()
            .expect("the tuned shed-budget table must hold the human-lane floor on every surface");
        for surface in [
            Surface::CollabOpStream,
            Surface::ConnectionTier,
            Surface::AgentMention,
            Surface::GitFrontDoor,
            Surface::RefsBacklinkRead,
            Surface::RefsRefCreate,
            Surface::SearchQuery,
            Surface::WorkflowAgentLane,
            Surface::HttpIntake,
        ] {
            let b = table.budget(surface);
            assert!(
                b.human_lane_reservation
                    >= SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap),
                "{surface:?} reserves {} of {} - at-or-above the measured human-lane floor {}",
                b.human_lane_reservation,
                b.per_tenant_in_flight_cap,
                SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap),
            );
        }
    }

    #[test]
    fn a_budget_tuned_below_the_human_lane_floor_fails_the_gate() {
        let starved = SurfaceBudget {
            per_tenant_in_flight_cap: 256,
            human_lane_reservation: 4,
            retry_after_secs: 3,
        };
        let err = starved
            .validate_tuned(Surface::ConnectionTier)
            .expect_err("a starved human lane must be rejected");
        match err {
            ShedBudgetError::HumanLaneStarved {
                surface,
                reservation,
                floor,
                cap,
            } => {
                assert_eq!(surface, Surface::ConnectionTier);
                assert_eq!(reservation, 4);
                assert_eq!(cap, 256);
                assert_eq!(floor, SurfaceBudget::human_lane_floor(256));
                assert!(reservation < floor, "the regression caught the starvation");
            }
            other => panic!("expected HumanLaneStarved, got {other:?}"),
        }

        let mut rows = HashMap::new();
        rows.insert(Surface::ConnectionTier, starved);
        let bad = ShedBudgetTable { rows };
        assert!(
            matches!(
                bad.validate(),
                Err(ShedBudgetError::HumanLaneStarved { .. })
            ),
            "the table validation gate catches a starved human lane"
        );
    }

    #[test]
    fn ci_dispatch_is_exempt_from_the_human_lane_floor_but_still_bounded() {
        let ci = SurfaceBudget {
            per_tenant_in_flight_cap: 64,
            human_lane_reservation: 0,
            retry_after_secs: 5,
        };
        ci.validate_tuned(Surface::CiDispatch)
            .expect("CI dispatch reserves no human lane (the batch lane) - valid");
        assert!(!Surface::CiDispatch.reserves_human_lane());

        assert!(matches!(
            SurfaceBudget {
                per_tenant_in_flight_cap: 64,
                human_lane_reservation: 0,
                retry_after_secs: 5,
            }
            .validate_tuned(Surface::HttpIntake),
            Err(ShedBudgetError::HumanLaneStarved { .. })
        ));

        assert!(matches!(
            SurfaceBudget {
                per_tenant_in_flight_cap: 0,
                human_lane_reservation: 0,
                retry_after_secs: 5,
            }
            .validate_tuned(Surface::CiDispatch),
            Err(ShedBudgetError::Unbounded(Surface::CiDispatch))
        ));
    }

    #[test]
    fn a_reservation_over_the_cap_is_rejected() {
        let over = SurfaceBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 20,
            retry_after_secs: 5,
        };
        assert!(matches!(
            over.validate_tuned(Surface::HttpIntake),
            Err(ShedBudgetError::ReservationOverCap {
                cap: 10,
                reservation: 20,
                ..
            })
        ));
    }

    #[test]
    fn human_lane_floor_is_twenty_percent_rounded_up_min_one() {
        assert_eq!(SurfaceBudget::HUMAN_LANE_FLOOR_BPS, 2000);
        assert_eq!(SurfaceBudget::human_lane_floor(200), 40);
        assert_eq!(SurfaceBudget::human_lane_floor(256), 52);
        assert_eq!(SurfaceBudget::human_lane_floor(1), 1);
        assert_eq!(SurfaceBudget::human_lane_floor(3), 1);
    }

    #[test]
    fn derive_then_admit_end_to_end_protects_the_human_over_an_agent_surge() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 2,
            retry_after_secs: 5,
        };
        let mut lane = ShedLane::with_budget(Surface::AgentMention, budget);
        let t = tenant("acme");
        let agent = agent_kind();
        let c = RunClass::derive(&agent, None);
        assert_eq!(c, RunClass::Agent);
        assert_eq!(lane.admit(&t, c), ShedDecision::Admit);
        assert_eq!(lane.admit(&t, c), ShedDecision::Admit);
        assert!(
            matches!(lane.admit(&t, c), ShedDecision::Shed { .. }),
            "the agent lane sheds"
        );
        let h = RunClass::derive(&PrincipalKind::Human, None);
        assert_eq!(lane.admit(&t, h), ShedDecision::Admit);
    }

    #[test]
    fn is_admitted_is_true_only_for_admit() {
        assert!(ShedDecision::Admit.is_admitted(), "Admit is admitted");
        assert!(
            !ShedDecision::Shed {
                retry_after_secs: 5
            }
            .is_admitted(),
            "Shed is NOT admitted"
        );
    }

    #[test]
    fn validate_tuned_admits_reservation_equal_to_cap_rejects_strictly_over() {
        let at_cap = SurfaceBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 10,
            retry_after_secs: 5,
        };
        assert!(
            at_cap.validate_tuned(Surface::HttpIntake).is_ok(),
            "reservation == cap is valid (not over) - strict `>`"
        );
        let over = SurfaceBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 11,
            retry_after_secs: 5,
        };
        assert!(
            matches!(
                over.validate_tuned(Surface::HttpIntake),
                Err(ShedBudgetError::ReservationOverCap { .. })
            ),
            "reservation strictly over cap is rejected"
        );
    }

    #[test]
    fn validate_tuned_admits_reservation_exactly_at_the_human_lane_floor() {
        assert_eq!(
            SurfaceBudget::human_lane_floor(10),
            2,
            "floor(10) == 2 (20%)"
        );
        let at_floor = SurfaceBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 2,
            retry_after_secs: 5,
        };
        assert!(
            at_floor.validate_tuned(Surface::HttpIntake).is_ok(),
            "reservation == floor is valid (not starved) - strict `<`"
        );
        let below = SurfaceBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 1,
            retry_after_secs: 5,
        };
        assert!(
            matches!(
                below.validate_tuned(Surface::HttpIntake),
                Err(ShedBudgetError::HumanLaneStarved { floor: 2, .. })
            ),
            "reservation strictly below the floor starves the human lane"
        );
    }

    #[test]
    fn in_flight_tracks_admits_and_the_graded_speculative_ceiling_uses_two_steps() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 24,
            human_lane_reservation: 0,
            retry_after_secs: 5,
        };
        let mut lane = ShedLane::with_budget(Surface::AgentMention, budget);
        let t = tenant("acme");

        for i in 0..18 {
            assert_eq!(
                lane.admit(&t, RunClass::Speculative),
                ShedDecision::Admit,
                "speculative admit #{i} is under the 18-ceiling"
            );
        }
        assert_eq!(
            lane.in_flight(&t),
            18,
            "in_flight reports the 18 admitted (not 0) - the blast-radius signal"
        );
        assert!(
            matches!(
                lane.admit(&t, RunClass::Speculative),
                ShedDecision::Shed { .. }
            ),
            "the 19th speculative run sheds at the 2*step ceiling (18) - strict `<`"
        );

        lane.release(&t, RunClass::Speculative);
        assert_eq!(
            lane.in_flight(&t),
            17,
            "release decremented in_flight by one"
        );
        assert_eq!(
            lane.admit(&t, RunClass::Speculative),
            ShedDecision::Admit,
            "with a freed slot the speculative run is admitted again"
        );
    }

    #[test]
    fn total_in_flight_is_strictly_bounded_by_cap_even_with_humans_present() {
        let cap = 8u32;
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: cap,
            human_lane_reservation: 0,
            retry_after_secs: 5,
        };
        let mut lane = ShedLane::with_budget(Surface::AgentMention, budget);
        let t = tenant("acme");

        for i in 0..3 {
            assert_eq!(
                lane.admit(&t, RunClass::Human),
                ShedDecision::Admit,
                "human admit #{i} fits"
            );
        }
        for i in 0..5 {
            assert_eq!(
                lane.admit(&t, RunClass::Agent),
                ShedDecision::Admit,
                "agent admit #{i} fits while total < cap and non_human < ceiling"
            );
        }
        assert_eq!(
            lane.in_flight(&t),
            cap,
            "total in-flight is exactly the cap (8)"
        );
        assert!(
            matches!(lane.admit(&t, RunClass::Agent), ShedDecision::Shed { .. }),
            "the run that would push total OVER the cap sheds - `cur.total() < cap` is strict"
        );
        assert_eq!(
            lane.in_flight(&t),
            cap,
            "in_flight never exceeds the cap (bounded-everything, contract 1.11)"
        );
    }
}
