use myelin_chat_gateway::shed::{LiveSurface, ShedGovernor, ShedVerdict};
use myelin_substrate::shed::{RunClass, ShedBudgetTable, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

#[test]
fn chat_surfaces_wire_the_substrate_shed_order_no_second_engine() {
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
fn chat_reads_the_per_surface_budget_floor_from_the_thresholds_file() {
    let thresholds = Thresholds::load_canonical().expect("the thresholds file loads");

    let conn = thresholds
        .shed_budget(Surface::ConnectionTier)
        .expect("ConnectionTier shed budget in the thresholds file");
    let agent = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("AgentMention shed budget in the thresholds file");

    assert!(
        conn.per_tenant_in_flight_cap > 0,
        "ConnectionTier is bounded"
    );
    assert!(
        agent.per_tenant_in_flight_cap > 0,
        "AgentMention is bounded"
    );
    assert!(
        conn.human_lane_reservation > 0,
        "the connection tier reserves a human lane (OQ-K CHAT row)"
    );
    assert!(
        conn.human_lane_reservation <= conn.per_tenant_in_flight_cap,
        "the reservation is within the cap"
    );

    let table = ShedBudgetTable::v1_floor();
    assert_eq!(
        conn,
        table.budget(Surface::ConnectionTier),
        "the thresholds ConnectionTier row IS the substrate v1 floor (one floor, not two)"
    );
    assert_eq!(agent, table.budget(Surface::AgentMention));

    let gov = ShedGovernor::from_thresholds(&thresholds);
    assert!(
        gov.is_ok(),
        "the chat governor opens from the thresholds file"
    );
}

#[test]
fn under_storm_presence_sheds_first_human_lane_holds() {
    let conn = SurfaceBudget {
        per_tenant_in_flight_cap: 6,
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
    for _ in 0..16 {
        if let ShedVerdict::Shed { .. } = gov.admit(&t, LiveSurface::Speculative) {
            presence_shed = true;
            break;
        }
    }
    assert!(presence_shed, "presence sheds first under storm");

    assert!(gov.admit(&t, LiveSurface::HumanMessage).is_delivered());
    assert_eq!(
        gov.shed_count(LiveSurface::HumanMessage),
        0,
        "0 human-lane drops while presence sheds"
    );
}

#[test]
fn agent_lane_sheds_before_the_human_lane() {
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
    for _ in 0..8 {
        if let ShedVerdict::Shed { .. } = gov.admit(&t, LiveSurface::AgentPartial) {
            agent_shed = true;
            break;
        }
    }
    assert!(agent_shed, "the agent lane sheds when over budget");
    assert!(
        gov.admit(&t, LiveSurface::HumanMessage).is_delivered(),
        "the human lane holds while the agent lane sheds (humans never queue behind agents)"
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

    for _ in 0..16 {
        let _ = gov.admit(&noisy, LiveSurface::Speculative);
    }
    assert_eq!(gov.in_flight(&quiet, LiveSurface::HumanMessage), 0);
    assert!(
        gov.admit(&quiet, LiveSurface::HumanMessage).is_delivered(),
        "one tenant's storm must NEVER shed another tenant's human"
    );
}
