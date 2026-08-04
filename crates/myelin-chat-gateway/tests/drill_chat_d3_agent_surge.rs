use myelin_chat_gateway::{run_chat_surge, surge_governor_from_thresholds, CHAT_SURGE_MULTIPLIER};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_substrate::shed::{Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

fn surge_multiplier_from_thresholds() -> u32 {
    let t = Thresholds::load_canonical().expect("the versioned thresholds file must load");
    let m = t.surge.multiplier;
    assert!(m > 0, "the surge multiplier must be a positive factor");
    m
}

fn derived_storm_frames(surging: &TenantId, base_requests: u64, multiplier: u32) -> (u64, u64) {
    let m = Multiplier::custom(multiplier).expect("a positive surge multiplier");
    let gen = LoadGenerator::new(
        base_requests,
        m,
        PrincipalMix::agent_skewed(),
        StormProfile::connection_storm(),
        vec![surging.clone()],
    )
    .expect("a non-empty tenant list");
    let mut sink = RecordingSink::default();
    gen.drive(&mut sink);
    let agent_frames = sink
        .received
        .iter()
        .filter(|r| r.load_kind == LoadPrincipalKind::Agent)
        .count() as u64;
    let human_frames = sink
        .received
        .iter()
        .filter(|r| r.load_kind == LoadPrincipalKind::Human)
        .count() as u64;
    assert!(
        agent_frames > 0,
        "the agent-skewed surge mix must issue agent frames (the storm the chat lane sheds)"
    );
    assert!(
        human_frames > 0,
        "the agent-skewed surge mix carries a thin human lane that must survive"
    );
    (agent_frames, human_frames)
}

#[test]
fn chat_d3_agent_surge_human_holds_agent_sheds_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, CHAT_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented Chat default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening - EI-01 §3)"
    );

    let surging = TenantId("noisy-chat-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    let (storm_frames, human_frames) = derived_storm_frames(&surging, 32, multiplier);

    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let mut gov =
        surge_governor_from_thresholds(&thresholds).expect("chat surge governor from file");
    let report = run_chat_surge(
        &mut gov,
        &surging,
        &quiet,
        storm_frames,
        human_frames,
        multiplier,
    );

    assert!(
        report.is_chat_d3_green(),
        "CHAT-D3 must be GREEN: {report:?}"
    );
    assert!(
        report.surging_agent_shed_count > 0,
        "the agent message storm MUST be absorbed by SHEDDING (429+Retry-After), not unbounded latency"
    );
    assert!(
        report.surging_presence_shed_count > 0,
        "the presence/speculative lane sheds first under the connection storm"
    );
    assert_eq!(
        report.surging_human_shed_count, 0,
        "the protected human message lane HELD on the surging tenant (shed-last; 0 drops)"
    );
    assert_eq!(
        report.surging_human_delivered, human_frames,
        "every human message frame was delivered (the human lane never queues behind agent runs)"
    );
    assert!(
        report.quiet_human_delivered,
        "the quiet co-tenant's human delivery was admitted within budget (untouched by the storm)"
    );
    assert_eq!(
        report.cross_tenant_impact, 0,
        "cross-tenant impact is 0 - the storm is contained to the surging tenant"
    );

    println!(
        "[P-500 CHAT-D3 GREEN 2026-06-25] {} (storm_frames={storm_frames} human_frames={human_frames} \
         derived from the P-S02 generator at {multiplier}× agent message/connection surge)",
        report.summary()
    );
}

#[test]
fn chat_d3_quiet_tenant_human_delivered_even_when_surging_tenant_fully_saturated() {
    use myelin_chat_gateway::{LiveSurface, ShedGovernor};

    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let mut gov = ShedGovernor::from_thresholds(&thresholds).expect("governor from file");
    gov.set_under_pressure(true);
    let surging = TenantId("noisy".into());
    let quiet = TenantId("quiet".into());
    let cap = thresholds
        .shed_budget(Surface::ConnectionTier)
        .expect("present")
        .per_tenant_in_flight_cap;

    for _ in 0..(cap * 4) {
        let _ = gov.admit(&surging, LiveSurface::Speculative);
    }
    assert!(
        gov.shed_count(LiveSurface::Speculative) > 0,
        "the saturated surging tenant's presence lane sheds"
    );

    assert_eq!(
        gov.in_flight(&quiet, LiveSurface::HumanMessage),
        0,
        "the quiet tenant's connection-tier budget is independent of the surging tenant's storm"
    );
    assert!(
        gov.admit(&quiet, LiveSurface::HumanMessage).is_delivered(),
        "the surging tenant's chat storm must NEVER shed another tenant's human (cross-tenant 0)"
    );
}

#[test]
fn chat_d3_an_unbounded_lane_reads_red() {
    use myelin_chat_gateway::ShedGovernor;
    let huge = SurfaceBudget {
        per_tenant_in_flight_cap: 1_000_000,
        human_lane_reservation: 200_000,
        retry_after_secs: 3,
    };
    let mut gov = ShedGovernor::with_budgets(huge, huge);
    let report = run_chat_surge(
        &mut gov,
        &TenantId("noisy".into()),
        &TenantId("quiet".into()),
        100,
        100,
        CHAT_SURGE_MULTIPLIER,
    );
    assert_eq!(
        report.surging_agent_shed_count, 0,
        "the unbounded lane swallowed the storm (no shed) - the failure mode the gate catches"
    );
    assert!(
        !report.is_chat_d3_green(),
        "an unbounded chat lane (storm not absorbed by shedding) MUST read RED - never a silent pass"
    );
}
