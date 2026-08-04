use myelin_substrate::shed::{RunClass, ShedDecision, ShedLane, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LiveSurface {
    Speculative,
    ReadState,
    Typing,
    AgentPartial,
    HumanMessage,
}

impl LiveSurface {
    pub const ALL: [LiveSurface; 5] = [
        LiveSurface::Speculative,
        LiveSurface::ReadState,
        LiveSurface::Typing,
        LiveSurface::AgentPartial,
        LiveSurface::HumanMessage,
    ];

    pub fn substrate_surface(self) -> Surface {
        match self {
            LiveSurface::AgentPartial => Surface::AgentMention,
            _ => Surface::ConnectionTier,
        }
    }

    pub fn run_class(self) -> RunClass {
        match self {
            LiveSurface::Speculative | LiveSurface::ReadState => RunClass::Speculative,
            LiveSurface::Typing => RunClass::BatchCi,
            LiveSurface::AgentPartial => RunClass::Agent,
            LiveSurface::HumanMessage => RunClass::Human,
        }
    }

    pub fn is_protected_human_lane(self) -> bool {
        matches!(self, LiveSurface::HumanMessage)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShedVerdict {
    Deliver,
    Shed {
        retry_after_secs: u64,
    },
}

impl ShedVerdict {
    pub fn is_delivered(self) -> bool {
        matches!(self, ShedVerdict::Deliver)
    }
}

#[derive(Clone, Debug)]
pub struct ShedGovernor {
    connection_tier: ShedLane,
    agent_mention: ShedLane,
    under_pressure: bool,
}

impl ShedGovernor {
    pub fn new() -> ShedGovernor {
        ShedGovernor {
            connection_tier: ShedLane::new(Surface::ConnectionTier),
            agent_mention: ShedLane::new(Surface::AgentMention),
            under_pressure: false,
        }
    }

    pub fn from_thresholds(thresholds: &Thresholds) -> Result<ShedGovernor, String> {
        let conn = thresholds
            .shed_budget(Surface::ConnectionTier)
            .map_err(|e| format!("chat ConnectionTier shed budget unavailable: {e}"))?;
        let agent = thresholds
            .shed_budget(Surface::AgentMention)
            .map_err(|e| format!("chat AgentMention shed budget unavailable: {e}"))?;
        Ok(ShedGovernor {
            connection_tier: ShedLane::with_budget(Surface::ConnectionTier, conn),
            agent_mention: ShedLane::with_budget(Surface::AgentMention, agent),
            under_pressure: false,
        })
    }

    pub fn with_budgets(
        connection_tier: SurfaceBudget,
        agent_mention: SurfaceBudget,
    ) -> ShedGovernor {
        ShedGovernor {
            connection_tier: ShedLane::with_budget(Surface::ConnectionTier, connection_tier),
            agent_mention: ShedLane::with_budget(Surface::AgentMention, agent_mention),
            under_pressure: false,
        }
    }

    pub fn set_under_pressure(&mut self, under_pressure: bool) {
        self.under_pressure = under_pressure;
    }

    pub fn under_pressure(&self) -> bool {
        self.under_pressure
    }

    fn lane_mut(&mut self, surface: LiveSurface) -> &mut ShedLane {
        match surface.substrate_surface() {
            Surface::AgentMention => &mut self.agent_mention,
            _ => &mut self.connection_tier,
        }
    }

    fn lane(&self, surface: LiveSurface) -> &ShedLane {
        match surface.substrate_surface() {
            Surface::AgentMention => &self.agent_mention,
            _ => &self.connection_tier,
        }
    }

    pub fn admit(&mut self, tenant: &TenantId, surface: LiveSurface) -> ShedVerdict {
        let class = surface.run_class();
        match self.lane_mut(surface).admit(tenant, class) {
            ShedDecision::Admit => ShedVerdict::Deliver,
            ShedDecision::Shed { retry_after_secs } => ShedVerdict::Shed { retry_after_secs },
        }
    }

    pub fn on_drained(&mut self, tenant: &TenantId, surface: LiveSurface) {
        let class = surface.run_class();
        self.lane_mut(surface).release(tenant, class);
    }

    pub fn in_flight(&self, tenant: &TenantId, surface: LiveSurface) -> u32 {
        self.lane(surface).in_flight(tenant)
    }

    pub fn shed_count(&self, surface: LiveSurface) -> u64 {
        self.lane(surface).shed_count(surface.run_class())
    }
}

impl Default for ShedGovernor {
    fn default() -> Self {
        ShedGovernor::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    #[test]
    fn surfaces_map_to_substrate_in_shed_order() {
        assert_eq!(
            LiveSurface::Speculative.substrate_surface(),
            Surface::ConnectionTier
        );
        assert_eq!(LiveSurface::Speculative.run_class(), RunClass::Speculative);
        assert_eq!(LiveSurface::ReadState.run_class(), RunClass::Speculative);
        assert_eq!(LiveSurface::Typing.run_class(), RunClass::BatchCi);
        assert_eq!(
            LiveSurface::AgentPartial.substrate_surface(),
            Surface::AgentMention
        );
        assert_eq!(LiveSurface::AgentPartial.run_class(), RunClass::Agent);
        assert_eq!(
            LiveSurface::HumanMessage.substrate_surface(),
            Surface::ConnectionTier
        );
        assert_eq!(LiveSurface::HumanMessage.run_class(), RunClass::Human);
        assert!(LiveSurface::HumanMessage.is_protected_human_lane());

        assert!(RunClass::Speculative < RunClass::BatchCi);
        assert!(RunClass::BatchCi < RunClass::Agent);
        assert!(RunClass::Agent < RunClass::Human);
    }

    #[test]
    fn presence_sheds_first_human_lane_holds() {
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 5,
            human_lane_reservation: 2,
            retry_after_secs: 3,
        };
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        gov.set_under_pressure(true);
        let t = tenant();

        let mut presence_shed = false;
        for _ in 0..8 {
            match gov.admit(&t, LiveSurface::Speculative) {
                ShedVerdict::Deliver => {}
                ShedVerdict::Shed { .. } => {
                    presence_shed = true;
                    break;
                }
            }
        }
        assert!(presence_shed, "presence sheds under pressure");

        assert!(
            gov.admit(&t, LiveSurface::HumanMessage).is_delivered(),
            "the human lane holds while presence sheds"
        );
        assert_eq!(
            gov.shed_count(LiveSurface::HumanMessage),
            0,
            "0 human-lane drops"
        );
        assert!(
            gov.shed_count(LiveSurface::Speculative) > 0,
            "presence shed > 0"
        );
    }

    #[test]
    fn agent_lane_sheds_before_human_lane() {
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 5,
            human_lane_reservation: 2,
            retry_after_secs: 3,
        };
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        gov.set_under_pressure(true);
        let t = tenant();

        let mut agent_shed = false;
        for _ in 0..6 {
            if let ShedVerdict::Shed { .. } = gov.admit(&t, LiveSurface::AgentPartial) {
                agent_shed = true;
                break;
            }
        }
        assert!(agent_shed, "the agent lane sheds when over budget");
        assert!(
            gov.admit(&t, LiveSurface::HumanMessage).is_delivered(),
            "the human lane holds while the agent lane sheds"
        );
    }

    #[test]
    fn shedding_is_per_tenant() {
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 3,
        };
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        gov.set_under_pressure(true);
        let noisy = TenantId("noisy".into());
        let quiet = TenantId("quiet".into());

        for _ in 0..8 {
            let _ = gov.admit(&noisy, LiveSurface::Speculative);
        }
        assert_eq!(gov.in_flight(&quiet, LiveSurface::HumanMessage), 0);
        assert!(
            gov.admit(&quiet, LiveSurface::HumanMessage).is_delivered(),
            "one tenant's storm must NEVER shed another tenant's human"
        );
    }

    #[test]
    fn v1_floor_governor_opens_with_bounded_surfaces() {
        let mut gov = ShedGovernor::new();
        let t = tenant();
        assert!(gov.admit(&t, LiveSurface::HumanMessage).is_delivered());
        assert!(gov.admit(&t, LiveSurface::AgentPartial).is_delivered());
        assert!(gov.admit(&t, LiveSurface::Speculative).is_delivered());
    }
}
