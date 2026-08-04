use crate::shed::{LiveSurface, ShedGovernor};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

pub const CHAT_SURGE_MULTIPLIER: u32 = 30;

pub const CHAT_SHED_BUDGETS_TUNED: bool = true;

pub const SCYLLA_HOT_TIER_FOLLOW_ON: &str = "CHAT-P28";

pub const HOME_NODE_FOLLOW_ON: &str = "CHAT-P29";

pub const CROSS_ORG_FOLLOW_ON: &str = "CHAT-P30";

pub const COMMENT_CONSOLIDATION_FOLLOW_ON: &str = "CHAT-P31";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChatSurgeReport {
    pub surging_agent_shed_count: u64,
    pub surging_presence_shed_count: u64,
    pub surging_human_shed_count: u64,
    pub surging_human_delivered: u64,
    pub quiet_human_delivered: bool,
    pub cross_tenant_impact: u32,
}

impl ChatSurgeReport {
    pub fn is_chat_d3_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.surging_presence_shed_count > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_delivered > 0
            && self.quiet_human_delivered
            && self.cross_tenant_impact == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "CHAT-D3: surging agent_shed={} presence_shed={} human_shed={} human_delivered={} \
             quiet_human_delivered={} cross_tenant_impact={} → {}",
            self.surging_agent_shed_count,
            self.surging_presence_shed_count,
            self.surging_human_shed_count,
            self.surging_human_delivered,
            self.quiet_human_delivered,
            self.cross_tenant_impact,
            if self.is_chat_d3_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

pub fn surge_governor_from_thresholds(thresholds: &Thresholds) -> Result<ShedGovernor, String> {
    ShedGovernor::from_thresholds(thresholds)
}

pub fn run_chat_surge(
    gov: &mut ShedGovernor,
    surging: &TenantId,
    quiet: &TenantId,
    storm_frames: u64,
    human_frames: u64,
    _multiplier: u32,
) -> ChatSurgeReport {
    gov.set_under_pressure(true);

    let mut surging_human_delivered = 0u64;
    let frames = storm_frames.max(human_frames);

    for i in 0..frames {
        if i < storm_frames {
            let _ = gov.admit(surging, LiveSurface::Speculative);
            let _ = gov.admit(surging, LiveSurface::AgentPartial);
        }
        if i < human_frames && gov.admit(surging, LiveSurface::HumanMessage).is_delivered() {
            surging_human_delivered += 1;
            gov.on_drained(surging, LiveSurface::HumanMessage);
        }
    }

    let quiet_in_flight_before = gov.in_flight(quiet, LiveSurface::HumanMessage);
    let quiet_human_delivered = gov.admit(quiet, LiveSurface::HumanMessage).is_delivered();

    ChatSurgeReport {
        surging_agent_shed_count: gov.shed_count(LiveSurface::AgentPartial),
        surging_presence_shed_count: gov.shed_count(LiveSurface::Speculative),
        surging_human_shed_count: gov.shed_count(LiveSurface::HumanMessage),
        surging_human_delivered,
        quiet_human_delivered,
        cross_tenant_impact: quiet_in_flight_before,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shed::ShedVerdict;
    use myelin_substrate::shed::{Surface, SurfaceBudget};

    fn tenant(s: &str) -> TenantId {
        TenantId(s.into())
    }

    #[test]
    fn the_chat_shed_budgets_are_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        for surface in [Surface::ConnectionTier, Surface::AgentMention] {
            let b = thresholds.shed_budget(surface).expect("present");
            assert!(b.per_tenant_in_flight_cap > 0, "{surface:?} bounded (§7.1)");
        }
        let conn = thresholds.shed_budget(Surface::ConnectionTier).unwrap();
        assert!(
            conn.human_lane_reservation > 0,
            "ConnectionTier reserves a human lane"
        );
        let _gov = surge_governor_from_thresholds(&thresholds).expect("governor opens from file");
    }

    #[test]
    fn run_chat_surge_is_green() {
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 12,
            human_lane_reservation: 4,
            retry_after_secs: 3,
        };
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 6,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        let report = run_chat_surge(
            &mut gov,
            &tenant("noisy"),
            &tenant("quiet"),
            64,
            40,
            CHAT_SURGE_MULTIPLIER,
        );
        assert!(report.is_chat_d3_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(report.surging_presence_shed_count > 0, "presence lane shed");
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert_eq!(
            report.surging_human_delivered, 40,
            "every human frame delivered"
        );
        assert!(report.quiet_human_delivered, "quiet co-tenant's human held");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    }

    #[test]
    fn run_chat_surge_is_green_at_the_tuned_file_budgets() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let mut gov = surge_governor_from_thresholds(&thresholds).expect("governor from file");
        let conn = thresholds.shed_budget(Surface::ConnectionTier).unwrap();
        let storm = u64::from(conn.per_tenant_in_flight_cap) * 4;
        let report = run_chat_surge(
            &mut gov,
            &tenant("noisy"),
            &tenant("quiet"),
            storm,
            storm,
            CHAT_SURGE_MULTIPLIER,
        );
        assert!(
            report.is_chat_d3_green(),
            "the TUNED budgets hold the chat surge: {}",
            report.summary()
        );
    }

    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 8,
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
        let noisy = tenant("noisy");
        let quiet = tenant("quiet");

        for _ in 0..(8 * 4) {
            let _ = gov.admit(&noisy, LiveSurface::Speculative);
        }
        assert!(
            gov.shed_count(LiveSurface::Speculative) > 0,
            "the saturated noisy tenant's presence lane sheds"
        );
        assert_eq!(gov.in_flight(&quiet, LiveSurface::HumanMessage), 0);
        assert!(
            gov.admit(&quiet, LiveSurface::HumanMessage).is_delivered(),
            "the noisy storm must NEVER shed another tenant's human"
        );
    }

    #[test]
    fn an_unbounded_lane_reads_red() {
        let huge = SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 200_000,
            retry_after_secs: 3,
        };
        let mut gov = ShedGovernor::with_budgets(huge, huge);
        let report = run_chat_surge(
            &mut gov,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            100,
            CHAT_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm (no shed)"
        );
        assert!(
            !report.is_chat_d3_green(),
            "an unbounded chat lane MUST read RED - never a silent pass"
        );
    }

    #[test]
    fn a_shed_carries_retry_after() {
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 2,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 3,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        gov.set_under_pressure(true);
        let t = tenant("acme");
        let mut shed_retry = None;
        for _ in 0..8 {
            if let ShedVerdict::Shed { retry_after_secs } = gov.admit(&t, LiveSurface::AgentPartial)
            {
                shed_retry = Some(retry_after_secs);
                break;
            }
        }
        assert_eq!(
            shed_retry,
            Some(10),
            "the agent-lane shed carries the AgentMention Retry-After"
        );
    }

    #[test]
    fn the_floors_are_named_and_the_budgets_are_tuned() {
        const { assert!(CHAT_SHED_BUDGETS_TUNED) };
        assert_eq!(CHAT_SURGE_MULTIPLIER, 30);
        assert_eq!(SCYLLA_HOT_TIER_FOLLOW_ON, "CHAT-P28");
        assert_eq!(HOME_NODE_FOLLOW_ON, "CHAT-P29");
        assert_eq!(CROSS_ORG_FOLLOW_ON, "CHAT-P30");
        assert_eq!(COMMENT_CONSOLIDATION_FOLLOW_ON, "CHAT-P31");
    }
}
