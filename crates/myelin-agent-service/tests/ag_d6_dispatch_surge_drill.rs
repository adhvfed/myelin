//! # AG-D6 — the 30× agent-dispatch surge family: the human lane holds, the agent lane sheds, the shed
//! budget tuned (AG-P22 / P-478, M5)
//!
//! **Drill catalogue:** `01-whole-system-e2e-and-drill-catalogue.md` row **AG-D6** (F6 surge family):
//! *"30× agent dispatch surge → human lane holds, agent sheds, reserve/settle refuses over-budget runs,
//! others unaffected"*, signal `shed-counts/lane` + `reserve-refusals`, cadence `SCHED`. This is the
//! **agent-fabric's slice** of the master M5 surge family (sibling to SUB-D3 / BUS-D7 / ID-D9 / FLOW-D8 /
//! REF-D10 / SRCH-D6). **Architecture:** agent-fabric.md §8 (the C10 floor — the agent-mention-storm
//! shed budget: a per-tenant agent-run in-flight cap, humans NEVER queue behind agent runs (the
//! protected human lane), the agent lane sheds with `429 + Retry-After` honoured by the runtime; the
//! concrete number TUNED by the 30× agent-surge drill), §7.3 (the shed order
//! `speculative → batch/CI → agent → human-last`). **Contract-index:** row **1.11** (the
//! protected-human-lane shed order + the agent-lane budget — CONSUMED), row **1.9** (`ResilientClient`
//! honours `Retry-After`), row **11.7** (reserve/settle refuses over-budget runs). **Doctrine:** EI-01
//! §3 (prove-it under 1×/10×/30×; the multiplier is read from the FROZEN thresholds file, never
//! hardcoded; never weaken a threshold to pass — a red is a dated `claimed-not-proven` row), §2 (the
//! protected human lane; per-tenant blast-radius); EI-03 §5 (the agent lane is the shed-before-human
//! lane; the runtime MUST honour `Retry-After` or shedding becomes a retry storm).
//!
//! ## What this drill proves (the four AG-D6 properties on the agent-dispatch surface)
//! Under a 30× agent-dispatch storm by ONE tenant the agent-dispatch surge gate:
//! 1. **ABSORBS the storm by SHEDDING** the agent lane (`429 + Retry-After`), never by growing dispatch
//!    latency unboundedly (Little's Law) — `surging_agent_shed_count > 0`, every shed carries the
//!    surface's Retry-After, and the runtime HONOURS it (no retry storm);
//! 2. **HOLDS the protected human lane** — the surging tenant's OWN human dispatch is admitted within its
//!    reserved slots (humans never queue behind agent runs) AND an unrelated co-tenant's human dispatch
//!    is admitted within budget;
//! 3. **the reserve/settle gate REFUSES the over-budget runs** (`reserve_refusals > 0`) while NEVER
//!    interrupting in-flight (`inflight_interrupt_count == 0` — the runaway self-limiter, AG-D11);
//! 4. keeps **cross-tenant impact at 0** — the storm fills only the surging tenant's per-tenant
//!    dispatch budget; the quiet co-tenant's lanes are untouched.
//!
//! ## The load is REAL (derived from the P-S02 generator), the multiplier is from the FILE (EI-01 §3)
//! The storm-op count is DERIVED from a real `myelin_harness::LoadGenerator` run at the surge multiplier
//! (the agent-mention storm profile, the agent-skewed mix) spread on the surging tenant — never a
//! hand-typed number. The surge multiplier is read from the workspace-root `thresholds.toml` `[surge]`
//! row (the versioned source of truth) and asserted to equal the documented default-to-beat
//! [`AGENT_DISPATCH_SURGE_MULTIPLIER`] — a divergence is a LOUD failure, never a silent weakening.
//!
//! ## The agent-lane shed budget: the M2 placeholder → the MEASURED cap (the AG-P22 DoD)
//! The agent-mention-storm shed budget was the M2 v1 floor (the bound existed in `Surface::AgentMention`;
//! the NUMBER was a placeholder). This drill asserts the MEASURED cap green: the thresholds-file
//! AgentMention cap (96 / 24) — the SAME number the Bus's BUS-D7 slice measured, now confirmed on the
//! agent-dispatch path — held the protected human lane (0 human shed) while the agent lane shed and the
//! reserve gate refused the over-budget runs. The row moves from "named floor" to "measured
//! default-to-beat" (dated 2026-06-25), never a number chosen to make the drill pass.
//!
//! ## Floors named (the prompt's honesty register)
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet).
//!   Here the load is the P-S02 generator at 30× across the surging tenant; the per-tenant fairness +
//!   shed-order + reserve-refusal + cross-tenant-0 PROPERTIES are complete + testable now and do not
//!   change shape when the real cell carries the load.
//! - **The real `AgentRuntime` brain** (`LlmAgentRuntime`) is designed-not-built (AG-P25) — UNCHANGED
//!   here; the cost gate + the shed lane are brain-agnostic (the whole point — the surge bounds the
//!   load regardless of which brain runs).
//!
//! Permanent-gate posture: re-run on every agent-dispatch-surface-touching change; contributes to the
//! master M5→M6 boundary (the F6 surge family green on the agent-dispatch surface).

use myelin_agent_service::{
    run_agent_dispatch_surge, AgentDispatchSurgeGate, RetryAfterHonouringRuntime,
    AGENT_DISPATCH_SURGE_MULTIPLIER,
};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_storage::{AgentRunGate, CostLedger, MinorUnits};
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
/// mix) on `surging` and return the number of **agent** dispatch ops the storm issues — the REAL derived
/// storm-op count (never hand-typed). Agent dispatches project onto the agent lane.
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

/// **THE AG-D6 SURGE PROOF (the dated green artifact the DoD names).** A 30× agent-dispatch storm by one
/// tenant (the storm-op count derived from a real generator run; the multiplier read from the FILE): the
/// agent lane sheds (absorbed, not unbounded; the runtime honours the Retry-After), the human lane HOLDS
/// (surging tenant's own + the quiet tenant's), the reserve gate refuses the over-budget runs without
/// interrupting in-flight, cross-tenant impact 0.
#[test]
fn ag_d6_agent_dispatch_surge_human_holds_agent_sheds_reserve_refuses_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, AGENT_DISPATCH_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented agent default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening — EI-01 §3)"
    );

    let surging = TenantId("noisy-agent-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    // The storm-op count is DERIVED from a real generator run at the surge multiplier (base 32 → an
    // agent-op count well past the tuned AgentMention non-human ceiling, so the storm MUST shed).
    let storm_ops = derived_agent_storm_ops(&surging, 32, multiplier);

    // Open BOTH fronts. The shed lane budget is read FROM THE THRESHOLDS FILE (the measured AgentMention
    // cap). The reserve gate fronts a wallet that affords only a funded prefix (so the wallet front ALSO
    // sheds — the AG-D6 reserve-refusal signal). Per-run cost 100; wallet affords 8 funded runs.
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
        MinorUnits(100),
        MinorUnits(800), // affords 8 funded runs; the rest are refused at reserve
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
        "the protected human lane HELD on the surging tenant (shed-last — humans never queue behind agents)"
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
        "cross-tenant impact is 0 — the storm is contained to the surging tenant"
    );
    assert!(
        report.reserve_refusals > 0,
        "the reserve/settle gate REFUSED the over-budget runs (the wallet front shed)"
    );
    assert_eq!(
        report.inflight_interrupt_count, 0,
        "NEVER interrupt in-flight — the headline zero (11.7 / AG-D11)"
    );

    // The runtime HONOURED every shed's Retry-After (no retry storm — 1.9 / EI-03 §5).
    assert_eq!(
        runtime.immediate_retries(),
        0,
        "the runtime honoured Retry-After — ZERO immediate retries (no retry storm)"
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

/// **MANDATORY: the cross-tenant-0 property is REAL — the quiet tenant's human is admitted DURING the
/// surge, never starved.** Saturate the surging tenant completely (every slot, reserved included), then
/// prove the quiet tenant's human dispatch is STILL admitted within budget (the per-tenant bound is the
/// blast-radius boundary).
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

    // Saturate the surging tenant COMPLETELY (cap admits, the rest shed) with an agent-dispatch storm.
    for _ in 0..(cap * 4) {
        let _ = lane.admit_dispatch(&surging, RunClass::Agent);
    }
    assert!(
        lane.shed_count(RunClass::Agent) > 0,
        "the saturated surging tenant's agent lane sheds"
    );

    // The quiet tenant is UNTOUCHED: its human dispatch is admitted within budget.
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

/// **MANDATORY counter-case: the AG-D6 gate is NOT vacuous — an UNBOUNDED lane + an UNLIMITED wallet
/// (no shed, no refusal) reads RED.** Proves the green is earned — the storm genuinely exceeds the lane
/// budget AND the wallet, and the two fronts are what hold it (EI-01 §3).
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
        MinorUnits(100),
        MinorUnits(1_000_000), // a huge wallet too — neither front sheds
        AGENT_DISPATCH_SURGE_MULTIPLIER,
    );
    assert_eq!(
        report.surging_agent_shed_count, 0,
        "the unbounded lane swallowed the storm (no shed) — the failure mode the gate catches"
    );
    assert_eq!(
        report.reserve_refusals, 0,
        "the unlimited wallet refused nothing — the failure mode the reserve gate catches"
    );
    assert!(
        !report.is_ag_d6_green(),
        "an unbounded agent lane + unlimited wallet (storm not absorbed) MUST read RED — never a silent pass"
    );
}
