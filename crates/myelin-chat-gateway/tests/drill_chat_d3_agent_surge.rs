//! # CHAT-D3 (TE-21 build-gate) — the Chat world-scale 30× agent message/connection surge + the
//! protected-human-lane shed order (CHAT-P26 / P-500, M5).
//!
//! **Drill catalogue:** the **F6 surge family** (the master M5 surge family — SUB-D3 / BUS-D7 / ID-D9
//! / REF-D10 / GIT-D6 / CI-D2). This is the **CHAT-surface half**: a 30× **agent message/connection
//! surge** on ONE tenant (testing-strategy row CHAT-D3). **Architecture:**
//! chat/architecture/07-drills-and-open-questions.md row **D-C3** + 02-internals-and-algorithms.md
//! §1.4/§7 (presence at scale — the connection tier where the worst load manifests).
//! **Reconciliation:** 00-reconciliation-decisions.md ADR-16 (the protected-human-lane shed order) +
//! OQ-K (the per-surface shed budgets — TUNED here). **Contract-index:** row **1.11** (the shed
//! order + chat's connection-storm + agent-mention-storm budgets, tuned), row **1.8** (the per-lane
//! shed-count / human-lane survival signals). **Doctrine:** EI-01 §3 (prove-it under 1×/10×/30×; the
//! multiplier is read from the FROZEN thresholds file, never hardcoded; never weaken a threshold to
//! pass — a red is a dated `claimed-not-proven` row), §2 (the protected human lane; per-tenant
//! blast-radius).
//!
//! ## What this drill proves (the three F6 properties on Chat's surfaces)
//! Under a 30× agent message/connection storm by ONE tenant the chat shed governor:
//! 1. **ABSORBS the storm by SHEDDING** the agent streaming-partial + presence/speculative lanes
//!    (`429 + Retry-After`), never by growing human connection/read latency unboundedly (Little's
//!    Law) — `surging_agent_shed_count > 0` AND `surging_presence_shed_count > 0`;
//! 2. **HOLDS the protected human lane** — the surging tenant's OWN live human message is delivered
//!    within its reserved slots (shed-last on the noisy tenant too; every human frame delivered) AND
//!    an unrelated co-tenant's human delivery is admitted within budget;
//! 3. keeps **cross-tenant impact at 0** — the storm fills only the surging tenant's per-tenant
//!    connection-tier budget; the quiet co-tenant's lanes are untouched.
//!
//! ## The load is REAL (derived from the P-S02 generator), the multiplier is from the FILE (EI-01 §3)
//! The storm-frame count is DERIVED from a real `myelin_harness::LoadGenerator` run at the surge
//! multiplier (the connection-storm + agent-mention storm profiles, the agent-skewed mix) spread on
//! the surging tenant — never a hand-typed number. The surge multiplier is read from the
//! workspace-root `thresholds.toml` `[surge]` row (the versioned source of truth) and asserted to
//! equal the documented default-to-beat [`CHAT_SURGE_MULTIPLIER`] — a divergence is a LOUD failure.
//!
//! ## The budgets are TUNED (promoting the CHAT-P10 floor — Q-C5 / OQ-K)
//! The chat surge governor reads its `ConnectionTier` + `AgentMention` budgets FROM the thresholds
//! file — the MEASURED defaults-to-beat (P-S33 / P-434), no longer named floors. This drill confirms
//! the tuned numbers hold the chat surge green. A vacuity counter-case (an unbounded lane reads RED)
//! proves the green is earned.

use myelin_chat_gateway::{run_chat_surge, surge_governor_from_thresholds, CHAT_SURGE_MULTIPLIER};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_substrate::shed::{Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// Read the `[surge] multiplier` from the workspace-root `thresholds.toml` through the typed
/// [`Thresholds`] loader (the versioned source of truth) — the SAME loader every other surge drill
/// uses. A missing/unreadable file is a LOUD failure (EI-01 §3).
fn surge_multiplier_from_thresholds() -> u32 {
    let t = Thresholds::load_canonical().expect("the versioned thresholds file must load");
    let m = t.surge.multiplier;
    assert!(m > 0, "the surge multiplier must be a positive factor");
    m
}

/// Drive the P-S02 load generator at the surge multiplier and return the (agent storm-frame count,
/// human-frame count) — the REAL derived counts (never hand-typed). The agent storm rides the
/// connection-storm + agent-mention profiles; the thin human lane must survive the storm.
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

/// **THE CHAT-D3 SURGE PROOF (the dated green artifact the DoD names).** A 30× agent message/
/// connection storm by one tenant (the storm-frame count derived from a real generator run; the
/// multiplier read from the FILE; the budgets TUNED from the file): the agent + presence lanes shed
/// (absorbed, not unbounded), the human lane HOLDS (surging tenant's own + the quiet tenant's; every
/// human frame delivered), cross-tenant impact 0.
#[test]
fn chat_d3_agent_surge_human_holds_agent_sheds_cross_tenant_zero() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, CHAT_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented Chat default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening — EI-01 §3)"
    );

    let surging = TenantId("noisy-chat-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    // The storm-frame counts are DERIVED from a real generator run at the surge multiplier (base 32 →
    // an agent-frame count well past the tuned ConnectionTier non-human ceiling, so the storm sheds).
    let (storm_frames, human_frames) = derived_storm_frames(&surging, 32, multiplier);

    // Drive the storm through the chat shed governor, budgets read FROM THE THRESHOLDS FILE (tuned).
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
        "cross-tenant impact is 0 — the storm is contained to the surging tenant"
    );

    println!(
        "[P-500 CHAT-D3 GREEN 2026-06-25] {} (storm_frames={storm_frames} human_frames={human_frames} \
         derived from the P-S02 generator at {multiplier}× agent message/connection surge)",
        report.summary()
    );
}

/// **MANDATORY: the cross-tenant-0 property is REAL — the quiet tenant's human is delivered DURING the
/// surge, never starved.** Saturate the surging tenant completely (every slot, reserved included),
/// then prove the quiet tenant's human delivery is STILL admitted within budget.
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

    // Saturate the surging tenant COMPLETELY (no drains → backs up + sheds) with a presence storm.
    for _ in 0..(cap * 4) {
        let _ = gov.admit(&surging, LiveSurface::Speculative);
    }
    assert!(
        gov.shed_count(LiveSurface::Speculative) > 0,
        "the saturated surging tenant's presence lane sheds"
    );

    // The quiet tenant is UNTOUCHED: its human delivery is admitted within budget.
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

/// **MANDATORY counter-case: the CHAT-D3 gate is NOT vacuous — an UNBOUNDED lane (no shed) reads
/// RED.** Proves the green is earned — the storm genuinely exceeds the lane budget and the shed is
/// what holds it (EI-01 §3).
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
        "the unbounded lane swallowed the storm (no shed) — the failure mode the gate catches"
    );
    assert!(
        !report.is_chat_d3_green(),
        "an unbounded chat lane (storm not absorbed by shedding) MUST read RED — never a silent pass"
    );
}
