//! # FLOW-D8 — the durable-workflow world-scale 30× agent surge + the protected-human-lane shed order
//! (P-FLOW-27 / P-476, M5)
//!
//! **Drill catalogue:** `01-whole-system-e2e-and-drill-catalogue.md` FLOW-D8 (F6 surge family):
//! *"30× surge of agent-initiated workflows → human-initiated lane holds, agent sheds, others
//! unaffected"*, signal `shed-counts/lane`, cadence `SCHED`. This is the **durable-workflow START
//! surface half** of the master M5 surge family (sibling to SUB-D3 / BUS-D7 / ID-D9 / REF-D10 /
//! SRCH-D6). **Architecture:** durable-workflow.md §7.6 (bounded everything with the principal-aware
//! shed order — an agent-mention storm sheds its lane with `429 + Retry-After` while human-initiated
//! workflows hold the protected lane, the F-8 drill asserts this). **Contract-index:** row **1.11**
//! (the protected-human-lane shed order — CONSUMED, tuned to the workflow-start surface), row **1.8**
//! (the per-lane shed-count telemetry). **Doctrine:** EI-01 §3 (prove-it under 1×/10×/30×; the
//! multiplier is read from the FROZEN thresholds file, never hardcoded; never weaken a threshold to
//! pass — a red is a dated `claimed-not-proven` row), §2 (the protected human lane; per-tenant
//! blast-radius).
//!
//! ## What this drill proves (the three F-8 properties on the workflow-start surface)
//! Under a 30× agent-initiated-workflow storm by ONE tenant the Flow surge gate:
//! 1. **ABSORBS the storm by SHEDDING** the agent lane (`429 + Retry-After`), never by growing
//!    workflow-start latency unboundedly (Little's Law) — `surging_agent_shed_count > 0` and every
//!    shed carries the surface's Retry-After;
//! 2. **HOLDS the protected human-initiated lane** — the surging tenant's OWN human-initiated
//!    workflow is admitted within its reserved slots (shed-last on the noisy tenant too) AND an
//!    unrelated co-tenant's human-initiated workflow is admitted within budget;
//! 3. keeps **cross-tenant impact at 0** — the storm fills only the surging tenant's per-tenant
//!    workflow-start budget; the quiet co-tenant's lanes are untouched.
//!
//! ## The load is REAL (derived from the P-S02 generator), the multiplier is from the FILE (EI-01 §3)
//! The storm-op count is DERIVED from a real `myelin_harness::LoadGenerator` run at the surge
//! multiplier (the agent-mention storm profile, the agent-skewed mix) spread on the surging tenant —
//! never a hand-typed number. The surge multiplier is read from the workspace-root `thresholds.toml`
//! `[surge]` row (the versioned source of truth) and asserted to equal the documented default-to-beat
//! [`myelin_flow::FLOW_SURGE_MULTIPLIER`] — a divergence would be a LOUD failure, never a silent
//! weakening.
//!
//! ## The reserve/settle budget gate still holds under shed
//! Shedding is a PRE-START admission decision: an over-budget agent-initiated start is refused with
//! `429` BEFORE any `workflow_run` row is journaled (let alone before the reserve/settle bookend runs,
//! P-FLOW-16), so a shed start returns no run (let alone a half-created one) and spends no budget. A
//! start that IS admitted still goes through the unchanged BudgetGate. The surge never relaxes the
//! cost gate — it only bounds concurrency at the front door.
//!
//! ## Floors named (the prompt's honesty register — designed-not-built)
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet).
//!   Here the load is the P-S02 generator at 30× across the surging tenant; the per-tenant fairness +
//!   shed-order + cross-tenant-0 PROPERTIES are complete + testable now and do not change shape when
//!   the real cell carries the load.
//! - **Cross-cell workflow spanning** is the durable-workflow.md §7.4 named floor (designed-not-built)
//!   — UNCHANGED here ([`myelin_flow::CROSS_CELL_SPANNING_IS_A_FLOOR`]): the surge proves the
//!   per-tenant shed order holds AT ONE CELL.
//!
//! Permanent-gate posture: re-run on every workflow-start-surface-touching change; contributes to the
//! master M5→M6 boundary (the F6 surge family green on the durable-workflow surface).

use myelin_flow::{run_flow_surge, FlowShedGate, FLOW_SURGE_MULTIPLIER};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_substrate::shed::{RunClass, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// Read the `[surge] multiplier` from the workspace-root `thresholds.toml` through the typed
/// [`Thresholds`] loader (the versioned source of truth) — the SAME loader every other surge drill
/// uses, not a re-parse. A missing/unreadable file is a LOUD failure (EI-01 §3).
fn surge_multiplier_from_thresholds() -> u32 {
    let t = Thresholds::load_canonical().expect("the versioned thresholds file must load");
    let m = t.surge.multiplier;
    assert!(m > 0, "the surge multiplier must be a positive factor");
    m
}

/// Drive the P-S02 load generator at the surge multiplier (agent-mention storm profile, agent-skewed
/// mix) on `surging` and return the number of **agent** workflow-start ops the storm issues — the REAL
/// derived storm-op count (never hand-typed). Agent-initiated workflow starts project onto the agent
/// lane.
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

/// **THE FLOW-D8 SURGE PROOF (the dated green artifact the DoD names).** A 30× agent-initiated-workflow
/// storm by one tenant (the storm-op count derived from a real generator run; the multiplier read from
/// the FILE): the agent lane sheds (absorbed, not unbounded), the human-initiated lane HOLDS (surging
/// tenant's own + the quiet tenant's), cross-tenant impact 0.
#[test]
fn flow_d8_agent_workflow_surge_human_holds_agent_sheds_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, FLOW_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented Flow default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening — EI-01 §3)"
    );

    let surging = TenantId("noisy-flow-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    // The storm-op count is DERIVED from a real generator run at the surge multiplier (base 32 → an
    // agent-op count well past the tuned WorkflowAgentLane non-human ceiling, so the storm MUST shed).
    let storm_ops = derived_agent_storm_ops(&surging, 32, multiplier);

    // Drive the storm at the workflow-start surface, budget read FROM THE THRESHOLDS FILE.
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
        "cross-tenant impact is 0 — the storm is contained to the surging tenant"
    );

    println!(
        "[P-476 FLOW-D8 GREEN 2026-06-25] {} (storm_ops={storm_ops} derived from the P-S02 \
         generator at {multiplier}× agent-mention surge)",
        report.summary()
    );
}

/// **MANDATORY: the cross-tenant-0 property is REAL — the quiet tenant's human is admitted DURING the
/// surge, never starved.** Saturate the surging tenant completely (every slot, reserved included),
/// then prove the quiet tenant's human-initiated workflow is STILL admitted within budget (the
/// per-tenant bound is the blast-radius boundary).
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

    // Saturate the surging tenant COMPLETELY (cap admits, the rest shed) with an agent-workflow storm.
    for _ in 0..(cap * 4) {
        let _ = gate.admit_class(&surging, RunClass::Agent);
    }
    assert!(
        gate.shed_count(RunClass::Agent) > 0,
        "the saturated surging tenant's agent lane sheds"
    );

    // The quiet tenant is UNTOUCHED: its human-initiated workflow is admitted within budget.
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

/// **MANDATORY counter-case: the FLOW-D8 gate is NOT vacuous — an UNBOUNDED lane (no shed) reads RED.**
/// Proves the green is earned — the storm genuinely exceeds the lane budget and the shed is what holds
/// it (EI-01 §3).
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
        "the unbounded lane swallowed the storm (no shed) — the failure mode the gate catches"
    );
    assert!(
        !report.is_flow_d8_green(),
        "an unbounded workflow lane (storm not absorbed by shedding) MUST read RED — never a silent pass"
    );
}
