use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_refs_service::{run_refs_surge, RefsShedGate, REFS_SURGE_MULTIPLIER};
use myelin_substrate::shed::{Surface, SurfaceBudget};
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
        "the agent-skewed surge mix must issue agent Refs ops (the storm the Refs lane sheds)"
    );
    agent_ops
}

#[test]
fn ref_d10_backlink_read_surge_human_holds_agent_sheds_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, REFS_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented Refs default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening - EI-01 §3)"
    );

    let surging = TenantId("noisy-refs-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    let storm_ops = derived_agent_storm_ops(&surging, 32, multiplier);

    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let mut gate = RefsShedGate::backlink_read_from_thresholds(&thresholds)
        .expect("RefsBacklinkRead budget present");
    let report = run_refs_surge(&mut gate, &surging, &quiet, storm_ops, multiplier);

    assert!(
        report.is_ref_d10_green(),
        "REF-D10 must be GREEN: {report:?}"
    );
    assert!(
        report.surging_agent_shed_count > 0,
        "the agent backlink-read storm MUST be absorbed by SHEDDING (429+Retry-After), not unbounded latency"
    );
    assert_eq!(
        report.surging_human_shed_count, 0,
        "the protected human read lane HELD on the surging tenant (shed-last)"
    );
    assert!(
        report.quiet_human_admitted,
        "the quiet co-tenant's human read was admitted within budget (untouched by the storm)"
    );
    assert_eq!(
        report.cross_tenant_impact, 0,
        "cross-tenant impact is 0 - the storm is contained to the surging tenant"
    );

    println!(
        "[P-453 REF-D10 BACKLINK-READ GREEN 2026-06-24] {} (storm_ops={storm_ops} derived from the \
         P-S02 generator at {multiplier}× agent-mention surge)",
        report.summary()
    );
}

#[test]
fn ref_d10_ref_create_surge_human_holds_agent_sheds_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    let surging = TenantId("noisy-refs-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());
    let storm_ops = derived_agent_storm_ops(&surging, 32, multiplier);

    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let mut gate = RefsShedGate::ref_create_from_thresholds(&thresholds)
        .expect("RefsRefCreate budget present");
    let report = run_refs_surge(&mut gate, &surging, &quiet, storm_ops, multiplier);

    assert!(
        report.is_ref_d10_green(),
        "REF-D10 ref-create must be GREEN: {report:?}"
    );
    assert!(
        report.surging_agent_shed_count > 0,
        "the agent ref-creation storm sheds"
    );
    assert_eq!(
        report.surging_human_shed_count, 0,
        "the human ref-create lane held"
    );
    assert!(
        report.quiet_human_admitted,
        "the quiet co-tenant's human ref-create held"
    );
    assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");

    println!(
        "[P-453 REF-D10 REF-CREATE GREEN 2026-06-24] {} (storm_ops={storm_ops} derived from the \
         P-S02 generator at {multiplier}× agent surge)",
        report.summary()
    );
}

#[test]
fn ref_d10_quiet_tenant_human_admitted_even_when_surging_tenant_fully_saturated() {
    use myelin_substrate::shed::RunClass;

    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let mut gate = RefsShedGate::backlink_read_from_thresholds(&thresholds)
        .expect("RefsBacklinkRead budget present");
    let surging = TenantId("noisy".into());
    let quiet = TenantId("quiet".into());
    let cap = thresholds
        .shed_budget(Surface::RefsBacklinkRead)
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
        "the quiet tenant's Refs budget is independent of the surging tenant's storm"
    );
    assert!(
        gate.admit_class(&quiet, RunClass::Human).is_ok(),
        "the surging tenant's Refs storm must NEVER shed another tenant's human (cross-tenant 0)"
    );
}

#[test]
fn ref_d10_an_unbounded_lane_reads_red() {
    let huge = SurfaceBudget {
        per_tenant_in_flight_cap: 1_000_000,
        human_lane_reservation: 200_000,
        retry_after_secs: 3,
    };
    let mut gate = RefsShedGate::with_budget(Surface::RefsBacklinkRead, huge);
    let report = run_refs_surge(
        &mut gate,
        &TenantId("noisy".into()),
        &TenantId("quiet".into()),
        100,
        REFS_SURGE_MULTIPLIER,
    );
    assert_eq!(
        report.surging_agent_shed_count, 0,
        "the unbounded lane swallowed the storm (no shed) - the failure mode the gate catches"
    );
    assert!(
        !report.is_ref_d10_green(),
        "an unbounded Refs lane (storm not absorbed by shedding) MUST read RED - never a silent pass"
    );
}
