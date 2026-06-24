//! # REF-D10 — the Refs world-scale 30× surge + the protected-human-lane shed order (REF-P22 / P-453, M5)
//!
//! **Drill catalogue:** the **F6 surge family** (the master M5 surge family — SUB-D3 / GIT-D6 /
//! CI-D2). This is the **REFS-surface half**: the 30× **agent ref-creation + agent backlink-read**
//! surge (REF-D10, reference-graph.md drill ~355). **Architecture:** reference-graph.md §6.2 (*measure
//! before you shard*). **Contract-index:** row **1.11** (the protected-human-lane shed order tuned to
//! Refs' two surfaces), consumed row **5.3** at scale (the backlink read the shed order protects), row
//! **1.8** (the per-lane shed-count telemetry). **Doctrine:** EI-01 §3 (prove-it under 1×/10×/30×; the
//! multiplier is read from the FROZEN thresholds file, never hardcoded; never weaken a threshold to
//! pass — a red is a dated `claimed-not-proven` row), §2 (the protected human lane; per-tenant
//! blast-radius).
//!
//! ## What this drill proves (the three F6 properties on the Refs surfaces)
//! Under a 30× agent ref-creation + backlink-read storm by ONE tenant the Refs surge gate:
//! 1. **ABSORBS the storm by SHEDDING** the agent lane (`429 + Retry-After`), never by growing
//!    read/create latency unboundedly (Little's Law) — `surging_agent_shed_count > 0`;
//! 2. **HOLDS the protected human lane** — the surging tenant's OWN human backlink-read/ref-create is
//!    admitted within its reserved slots (shed-last on the noisy tenant too) AND an unrelated
//!    co-tenant's human op is admitted within budget;
//! 3. keeps **cross-tenant impact at 0** — the storm fills only the surging tenant's per-tenant Refs
//!    budget; the quiet co-tenant's lanes are untouched.
//!
//! ## The load is REAL (derived from the P-S02 generator), the multiplier is from the FILE (EI-01 §3)
//! The storm-op count is DERIVED from a real `myelin_harness::LoadGenerator` run at the surge
//! multiplier (the agent-mention storm profile, the agent-skewed mix) spread on the surging tenant —
//! never a hand-typed number. The surge multiplier is read from the workspace-root `thresholds.toml`
//! `[surge]` row (the versioned source of truth) and asserted to equal the documented default-to-beat
//! [`myelin_refs_service::REFS_SURGE_MULTIPLIER`] — a divergence would be a LOUD failure, never a
//! silent weakening.
//!
//! ## The REF-P11 SetExpr-lowering leak invariant still holds under shed
//! Shedding is a PRE-READ admission decision: an over-budget read is refused with `429` BEFORE any
//! backlink resolution runs, so a shed read returns no result (let alone a leaked one), and a read
//! that IS admitted still goes through the unchanged REF-P11 permission filter. The surge never
//! relaxes the `list_objects` filter — it only bounds concurrency. The leak invariant cannot regress
//! under shed because the shed gate sits IN FRONT of the filter, never inside it.
//!
//! ## Floors named (the prompt's honesty register — designed-not-built)
//! - **The hot-artifact reach index R4** (the Leopard-style flattened reach index — the named REF-P11
//!   floor's follow-on) is **REF-P23** ([`myelin_refs_service::R4_REACH_INDEX_FOLLOW_ON`]). This
//!   prompt is the surge/shed-order half ONLY.
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet).
//!   Here the load is the P-S02 generator at 30× across the surging tenant; the per-tenant fairness +
//!   shed-order + cross-tenant-0 PROPERTIES are complete + testable now and do not change shape when
//!   the real PgStore-backed edge index carries the load.
//!
//! Permanent-gate posture: re-run on every Refs-surface-touching change; contributes to the master
//! M5→M6 boundary (the F6 surge family green on the Refs surfaces).

use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_refs_service::{run_refs_surge, RefsShedGate, REFS_SURGE_MULTIPLIER};
use myelin_substrate::shed::{Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// Read the `[surge] multiplier` from the workspace-root `thresholds.toml` through the typed
/// [`Thresholds`] loader (the versioned source of truth, P-038) — the SAME loader every other
/// substrate drill uses, not a re-parse. A missing/unreadable file is a LOUD failure (EI-01 §3).
fn surge_multiplier_from_thresholds() -> u32 {
    let t = Thresholds::load_canonical().expect("the versioned thresholds file must load");
    let m = t.surge.multiplier;
    assert!(m > 0, "the surge multiplier must be a positive factor");
    m
}

/// Drive the P-S02 load generator at the surge multiplier (agent-mention storm profile, agent-skewed
/// mix) on `surging` and return the number of **agent** Refs ops the storm issues — the REAL derived
/// storm-op count (never hand-typed). Agent ref-creation + backlink-read project onto the agent lane.
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

/// **THE REF-D10 SURGE PROOF (the dated green artifact the DoD names), on the BACKLINK-READ surface.**
/// A 30× agent backlink-read storm by one tenant (the storm-op count derived from a real generator
/// run; the multiplier read from the FILE): the agent lane sheds (absorbed, not unbounded), the human
/// read lane HOLDS (surging tenant's own + the quiet tenant's), cross-tenant impact 0.
#[test]
fn ref_d10_backlink_read_surge_human_holds_agent_sheds_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, REFS_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented Refs default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening — EI-01 §3)"
    );

    let surging = TenantId("noisy-refs-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    // The storm-op count is DERIVED from a real generator run at the surge multiplier (base 32 → an
    // agent-op count well past the tuned RefsBacklinkRead non-human ceiling, so the storm MUST shed).
    let storm_ops = derived_agent_storm_ops(&surging, 32, multiplier);

    // Drive the storm at the Refs backlink-read surface, budget read FROM THE THRESHOLDS FILE.
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
        "cross-tenant impact is 0 — the storm is contained to the surging tenant"
    );

    println!(
        "[P-453 REF-D10 BACKLINK-READ GREEN 2026-06-24] {} (storm_ops={storm_ops} derived from the \
         P-S02 generator at {multiplier}× agent-mention surge)",
        report.summary()
    );
}

/// **THE REF-D10 SURGE PROOF, on the REF-CREATION surface.** A 30× agent ref-creation storm: the agent
/// lane sheds, the human ref-create lane holds, cross-tenant impact 0.
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

/// **MANDATORY: the cross-tenant-0 property is REAL — the quiet tenant's human is admitted DURING the
/// surge, never starved.** Saturate the surging tenant completely (every slot, reserved included),
/// then prove the quiet tenant's human read is STILL admitted within budget (the per-tenant bound is
/// the blast-radius boundary).
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

    // Saturate the surging tenant COMPLETELY (cap admits, the rest shed) with an agent storm.
    for _ in 0..(cap * 4) {
        let _ = gate.admit_class(&surging, RunClass::Agent);
    }
    assert!(
        gate.shed_count(RunClass::Agent) > 0,
        "the saturated surging tenant's agent lane sheds"
    );

    // The quiet tenant is UNTOUCHED: its human read is admitted within budget.
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

/// **MANDATORY counter-case: the REF-D10 gate is NOT vacuous — an UNBOUNDED lane (no shed) reads
/// RED.** Proves the green is earned — the storm genuinely exceeds the lane budget and the shed is
/// what holds it (EI-01 §3).
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
        "the unbounded lane swallowed the storm (no shed) — the failure mode the gate catches"
    );
    assert!(
        !report.is_ref_d10_green(),
        "an unbounded Refs lane (storm not absorbed by shedding) MUST read RED — never a silent pass"
    );
}
