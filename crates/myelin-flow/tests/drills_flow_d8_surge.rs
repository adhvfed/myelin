use myelin_flow::{run_flow_surge, FlowShedGate, FLOW_SURGE_MULTIPLIER};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_substrate::shed::{RunClass, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

fn surge_multiplier_from_thresholds() -> u32 {
    let t = Thresholds::load_canonical().expect("the versioned thresholds file must load");
    let m = t.surge.multiplier;
    assert!(m > 0, "the surge multiplier must be a positive factor");
    m
}

fn derived_agent_storm_ops(surging: &TenantId, base_requests: u64, multiplier: u32) -> u64 {
    let m = Multiplier::custom(multiplier).expect("a positive surge multiplier");
    let gen = LoadGenerator::new(
        base_requests,
        m,
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![surging.clone()],
    )
    .expect("a non-empty tenant list");
    let mut sink = RecordingSink::default();
    gen.drive(&mut sink);
    let agent_ops = sink
        .received
        .iter()
        .filter(|r| r.load_kind == LoadPrincipalKind::Agent)
        .count() as u64;
    assert!(
        agent_ops > 0,
        "the agent-skewed surge mix must issue agent workflow-start ops (the storm the Flow lane sheds)"
    );
    agent_ops
}

#[test]
fn flow_d8_agent_workflow_surge_human_holds_agent_sheds_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, FLOW_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented Flow default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening - EI-01 §3)"
    );

    let surging = TenantId("noisy-flow-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    let storm_ops = derived_agent_storm_ops(&surging, 32, multiplier);

    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let mut gate =
        FlowShedGate::from_thresholds(&thresholds).expect("WorkflowAgentLane budget present");
    let report = run_flow_surge(&mut gate, &surging, &quiet, storm_ops, multiplier);

    assert!(
        report.is_flow_d8_green(),
        "FLOW-D8 must be GREEN: {report:?}"
    );
    assert!(
        report.surging_agent_shed_count > 0,
        "the agent-initiated-workflow storm MUST be absorbed by SHEDDING (429+Retry-After), not unbounded latency"
    );
    assert!(
        report.agent_shed_retry_after_secs > 0,
        "every agent-lane shed carries a Retry-After (the no-amplification guarantee)"
    );
    assert_eq!(
        report.surging_human_shed_count, 0,
        "the protected human-initiated lane HELD on the surging tenant (shed-last)"
    );
    assert!(
        report.quiet_human_admitted,
        "the quiet co-tenant's human-initiated workflow was admitted within budget (untouched)"
    );
    assert_eq!(
        report.cross_tenant_impact, 0,
        "cross-tenant impact is 0 - the storm is contained to the surging tenant"
    );

    println!(
        "[P-476 FLOW-D8 GREEN 2026-06-25] {} (storm_ops={storm_ops} derived from the P-S02 \
         generator at {multiplier}× agent-mention surge)",
        report.summary()
    );
}

#[test]
fn flow_d8_quiet_tenant_human_admitted_even_when_surging_tenant_fully_saturated() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let mut gate =
        FlowShedGate::from_thresholds(&thresholds).expect("WorkflowAgentLane budget present");
    let surging = TenantId("noisy".into());
    let quiet = TenantId("quiet".into());
    let cap = thresholds
        .shed_budget(Surface::WorkflowAgentLane)
        .expect("present")
        .per_tenant_in_flight_cap;

    for _ in 0..(cap * 4) {
        let _ = gate.admit_class(&surging, RunClass::Agent);
    }
    assert!(
        gate.shed_count(RunClass::Agent) > 0,
        "the saturated surging tenant's agent lane sheds"
    );

    assert_eq!(
        gate.in_flight(&quiet),
        0,
        "the quiet tenant's workflow-start budget is independent of the surging tenant's storm"
    );
    assert!(
        gate.admit_class(&quiet, RunClass::Human).is_ok(),
        "the surging tenant's workflow storm must NEVER shed another tenant's human (cross-tenant 0)"
    );
}

#[test]
fn flow_d8_an_unbounded_lane_reads_red() {
    let huge = SurfaceBudget {
        per_tenant_in_flight_cap: 1_000_000,
        human_lane_reservation: 200_000,
        retry_after_secs: 10,
    };
    let mut gate = FlowShedGate::with_budget(huge);
    let report = run_flow_surge(
        &mut gate,
        &TenantId("noisy".into()),
        &TenantId("quiet".into()),
        100,
        FLOW_SURGE_MULTIPLIER,
    );
    assert_eq!(
        report.surging_agent_shed_count, 0,
        "the unbounded lane swallowed the storm (no shed) - the failure mode the gate catches"
    );
    assert!(
        !report.is_flow_d8_green(),
        "an unbounded workflow lane (storm not absorbed by shedding) MUST read RED - never a silent pass"
    );
}
