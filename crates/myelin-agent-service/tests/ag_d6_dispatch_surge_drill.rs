use myelin_agent_service::{
    run_agent_dispatch_surge, AgentDispatchSurgeGate, RetryAfterHonouringRuntime,
    AGENT_DISPATCH_SURGE_MULTIPLIER,
};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_storage::{AgentRunGate, CostLedger, MicroUsd};
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
        "the agent-skewed surge mix must issue agent dispatch ops (the storm the agent lane sheds)"
    );
    agent_ops
}

#[test]
fn ag_d6_agent_dispatch_surge_human_holds_agent_sheds_reserve_refuses_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, AGENT_DISPATCH_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented agent default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening - EI-01 §3)"
    );

    let surging = TenantId("noisy-agent-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    let storm_ops = derived_agent_storm_ops(&surging, 32, multiplier);

    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let mut lane =
        AgentDispatchSurgeGate::from_thresholds(&thresholds).expect("AgentMention budget present");
    let mut reserve_gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let mut runtime = RetryAfterHonouringRuntime::new();

    let report = run_agent_dispatch_surge(
        &mut lane,
        &mut reserve_gate,
        &mut ledger,
        &mut runtime,
        &surging,
        &quiet,
        storm_ops,
        MicroUsd(100),
        MicroUsd(800),
        multiplier,
    );

    assert!(report.is_ag_d6_green(), "AG-D6 must be GREEN: {report:?}");
    assert!(
        report.surging_agent_shed_count > 0,
        "the agent-dispatch storm MUST be absorbed by SHEDDING (429+Retry-After), not unbounded latency"
    );
    assert!(
        report.agent_shed_retry_after_secs > 0,
        "every agent-lane shed carries a Retry-After (the no-amplification guarantee)"
    );
    assert_eq!(
        report.surging_human_shed_count, 0,
        "the protected human lane HELD on the surging tenant (shed-last - humans never queue behind agents)"
    );
    assert!(
        report.surging_human_admitted,
        "the surging tenant's OWN human dispatch was admitted within its reserved slots"
    );
    assert!(
        report.quiet_human_admitted,
        "the quiet co-tenant's human dispatch was admitted within budget (untouched)"
    );
    assert_eq!(
        report.cross_tenant_impact, 0,
        "cross-tenant impact is 0 - the storm is contained to the surging tenant"
    );
    assert!(
        report.reserve_refusals > 0,
        "the reserve/settle gate REFUSED the over-budget runs (the wallet front shed)"
    );
    assert_eq!(
        report.inflight_interrupt_count, 0,
        "NEVER interrupt in-flight - the headline zero (11.7 / AG-D11)"
    );

    assert_eq!(
        runtime.immediate_retries(),
        0,
        "the runtime honoured Retry-After - ZERO immediate retries (no retry storm)"
    );
    assert!(
        runtime.backoff_total_secs() > 0,
        "the runtime backed off for the advertised Retry-After on every shed"
    );

    println!(
        "[P-478 AG-D6 GREEN 2026-06-25] {} (storm_ops={storm_ops} derived from the P-S02 \
         generator at {multiplier}× agent-mention surge; measured AgentMention cap 96/24)",
        report.summary()
    );
}

#[test]
fn ag_d6_quiet_tenant_human_admitted_even_when_surging_tenant_fully_saturated() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let mut lane =
        AgentDispatchSurgeGate::from_thresholds(&thresholds).expect("AgentMention budget present");
    let surging = TenantId("noisy".into());
    let quiet = TenantId("quiet".into());
    let cap = thresholds
        .shed_budget(Surface::AgentMention)
        .expect("present")
        .per_tenant_in_flight_cap;

    for _ in 0..(cap * 4) {
        let _ = lane.admit_dispatch(&surging, RunClass::Agent);
    }
    assert!(
        lane.shed_count(RunClass::Agent) > 0,
        "the saturated surging tenant's agent lane sheds"
    );

    assert_eq!(
        lane.in_flight(&quiet),
        0,
        "the quiet tenant's dispatch budget is independent of the surging tenant's storm"
    );
    assert!(
        lane.admit_dispatch(&quiet, RunClass::Human).is_ok(),
        "the surging tenant's dispatch storm must NEVER shed another tenant's human (cross-tenant 0)"
    );
}

#[test]
fn ag_d6_an_unbounded_lane_and_unlimited_wallet_reads_red() {
    let huge = SurfaceBudget {
        per_tenant_in_flight_cap: 1_000_000,
        human_lane_reservation: 200_000,
        retry_after_secs: 10,
    };
    let mut lane = AgentDispatchSurgeGate::with_budget(huge);
    let mut reserve_gate = AgentRunGate::new();
    let mut ledger = CostLedger::new();
    let mut runtime = RetryAfterHonouringRuntime::new();
    let report = run_agent_dispatch_surge(
        &mut lane,
        &mut reserve_gate,
        &mut ledger,
        &mut runtime,
        &TenantId("noisy".into()),
        &TenantId("quiet".into()),
        100,
        MicroUsd(100),
        MicroUsd(1_000_000),
        AGENT_DISPATCH_SURGE_MULTIPLIER,
    );
    assert_eq!(
        report.surging_agent_shed_count, 0,
        "the unbounded lane swallowed the storm (no shed) - the failure mode the gate catches"
    );
    assert_eq!(
        report.reserve_refusals, 0,
        "the unlimited wallet refused nothing - the failure mode the reserve gate catches"
    );
    assert!(
        !report.is_ag_d6_green(),
        "an unbounded agent lane + unlimited wallet (storm not absorbed) MUST read RED - never a silent pass"
    );
}
