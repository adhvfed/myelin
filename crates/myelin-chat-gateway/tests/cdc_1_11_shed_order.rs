//! # CDC — contract **1.11**: the protected-human-lane shed order + the per-surface shed-budget
//! FLOOR, the CHAT CONNECTION-TIER consumer (CHAT-P10 / P-404).
//!
//! Chat OWNS the connection-storm + agent-mention-storm SURFACES of contract 1.11 (OQ-K's CHAT
//! rows); it does NOT own the shed engine — the lane / run-class / budget table are the SUBSTRATE's
//! frozen primitive ([`myelin_substrate::shed`]), already wired by Git + CI. This CDC carries BOTH
//! sides of the seam:
//!  - **PROVIDER side (chat owns the SURFACE→`(Surface, RunClass)` mapping).** Chat is the provider
//!    of the connection-tier-specific decision: which substrate `Surface` + `RunClass` each chat
//!    [`LiveSurface`] derives to (the OQ-K CHAT-row mapping). The substrate is the provider of the
//!    shed ENGINE; chat is the provider of the surface mapping that wires onto it.
//!  - **CONSUMER side (the connection tier applies the substrate engine).**
//!
//! This CDC pins chat's side so there is NO local divergence + NO second engine (EI-01 §7):
//!  - **the SAME substrate engine** — chat's [`ShedGovernor`] is a thin wiring over the substrate
//!    [`ShedLane`]; the run-class order it relies on is the substrate `RunClass`
//!    (speculative → batch/CI → agent → human-last), not a chat re-definition;
//!  - **the chat surfaces map to the substrate surfaces** — the connection-tier live frames ride
//!    `Surface::ConnectionTier`, the agent partials ride `Surface::AgentMention` (OQ-K's CHAT rows);
//!  - **the budget is read FROM THE THRESHOLDS FILE** — `[[shed_budgets]] surface = "ConnectionTier"
//!    / "AgentMention"`, the OQ-K v1 floors, never a guessed default;
//!  - **the shed order holds** — under storm, presence/agent shed before the human message lane; 0
//!    human-lane drops while a lower-priority lane still sheds (VISION §3).

use myelin_chat_gateway::shed::{LiveSurface, ShedGovernor, ShedVerdict};
use myelin_substrate::shed::{RunClass, ShedBudgetTable, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

// ============================================================================================
// PROVIDER 1.11 — chat owns the surface→(Surface, RunClass) mapping that wires the substrate engine
// ============================================================================================

/// **PROVIDER 1.11 — chat's live surfaces map onto the SUBSTRATE `(Surface, RunClass)` in the frozen
/// shed order.** No second shed order: the order chat relies on IS the substrate `RunClass` order
/// (a lower class sheds first). The connection-tier frames ride `ConnectionTier`; the agent partials
/// ride `AgentMention`; the human message lane is the protected `Human` lane. Chat is the PROVIDER of
/// this mapping; the substrate is the provider of the engine the mapping wires onto.
#[test]
fn chat_surfaces_wire_the_substrate_shed_order_no_second_engine() {
    // presence/read-state → ConnectionTier + Speculative (the lowest-promise, shed first).
    assert_eq!(
        LiveSurface::Speculative.substrate_surface(),
        Surface::ConnectionTier
    );
    assert_eq!(LiveSurface::Speculative.run_class(), RunClass::Speculative);
    assert_eq!(LiveSurface::ReadState.run_class(), RunClass::Speculative);
    // typing → batch/CI rung.
    assert_eq!(LiveSurface::Typing.run_class(), RunClass::BatchCi);
    // agent streaming partials → the AgentMention storm surface + the Agent lane.
    assert_eq!(
        LiveSurface::AgentPartial.substrate_surface(),
        Surface::AgentMention
    );
    assert_eq!(LiveSurface::AgentPartial.run_class(), RunClass::Agent);
    // the human message delivery → the protected Human lane on the connection tier.
    assert_eq!(
        LiveSurface::HumanMessage.substrate_surface(),
        Surface::ConnectionTier
    );
    assert_eq!(LiveSurface::HumanMessage.run_class(), RunClass::Human);
    assert!(LiveSurface::HumanMessage.is_protected_human_lane());

    // the SUBSTRATE run-class order IS the shed order chat relies on (no chat-side re-definition).
    assert!(RunClass::Speculative < RunClass::BatchCi);
    assert!(RunClass::BatchCi < RunClass::Agent);
    assert!(RunClass::Agent < RunClass::Human);
}

/// **CONSUMER 1.11 — the per-surface budget FLOOR is read from the THRESHOLDS FILE (the OQ-K v1
/// floor), never a guessed default.** Chat's `from_thresholds` opens both lanes against the file's
/// `ConnectionTier` + `AgentMention` rows; a missing row is a LOUD error. The file's numbers equal
/// the substrate `v1_floor` table (one v1 floor, not two — EI-01 §7).
#[test]
fn chat_reads_the_per_surface_budget_floor_from_the_thresholds_file() {
    let thresholds = Thresholds::load_canonical().expect("the thresholds file loads");

    // the two chat surfaces have a budget row in the file (OQ-K's CHAT rows).
    let conn = thresholds
        .shed_budget(Surface::ConnectionTier)
        .expect("ConnectionTier shed budget in the thresholds file");
    let agent = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("AgentMention shed budget in the thresholds file");

    // every surface is BOUNDED (an unbounded one is the cascade, EI-02 §5).
    assert!(
        conn.per_tenant_in_flight_cap > 0,
        "ConnectionTier is bounded"
    );
    assert!(
        agent.per_tenant_in_flight_cap > 0,
        "AgentMention is bounded"
    );
    // the connection tier reserves a HUMAN lane (interactive humans shed last).
    assert!(
        conn.human_lane_reservation > 0,
        "the connection tier reserves a human lane (OQ-K CHAT row)"
    );
    assert!(
        conn.human_lane_reservation <= conn.per_tenant_in_flight_cap,
        "the reservation is within the cap"
    );

    // ONE v1 floor, not two: the file's numbers equal the substrate v1_floor table (REUSED).
    let table = ShedBudgetTable::v1_floor();
    assert_eq!(
        conn,
        table.budget(Surface::ConnectionTier),
        "the thresholds ConnectionTier row IS the substrate v1 floor (one floor, not two)"
    );
    assert_eq!(agent, table.budget(Surface::AgentMention));

    // and the governor opens cleanly from the file (the production constructor).
    let gov = ShedGovernor::from_thresholds(&thresholds);
    assert!(
        gov.is_ok(),
        "the chat governor opens from the thresholds file"
    );
}

// ============================================================================================
// CONSUMER 1.11 — the shed order holds under storm pressure (0 human-lane drops)
// ============================================================================================

/// **CONSUMER 1.11 — under storm, presence sheds FIRST and the human lane holds (0 human-lane drops
/// while presence sheds).** Driven at a small deterministic budget through the chat wiring.
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

    // presence sheds first (the lowest-promise lane).
    let mut presence_shed = false;
    for _ in 0..16 {
        if let ShedVerdict::Shed { .. } = gov.admit(&t, LiveSurface::Speculative) {
            presence_shed = true;
            break;
        }
    }
    assert!(presence_shed, "presence sheds first under storm");

    // the human lane holds (0 drops) — it uses the reserved slots.
    assert!(gov.admit(&t, LiveSurface::HumanMessage).is_delivered());
    assert_eq!(
        gov.shed_count(LiveSurface::HumanMessage),
        0,
        "0 human-lane drops while presence sheds"
    );
}

/// **CONSUMER 1.11 — the agent lane sheds before the human lane (humans never queue behind agent
/// runs).** The agent partials ride the dedicated AgentMention surface; saturate it → it sheds; the
/// human message on the connection tier still delivers.
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

/// **CONSUMER 1.11 — shedding is per-tenant (one tenant's storm never sheds another's human — the
/// substrate blast-radius guarantee, inherited by the chat wiring).**
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
