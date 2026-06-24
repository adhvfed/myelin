//! # P-S33 (global P-434, M5) — the TUNED per-surface shed budgets + the human-lane-starvation
//! regression.
//!
//! **Prompt:** P-S33 → global **P-434** (M5), `planning/07-prompts/by-system/00-platform-substrate.md`
//! §P-S33. **Architecture:** `00-platform-substrate.md` §7.6 (the per-surface budget table — now TUNED
//! by the drills). **Contract-index:** row **1.11** (the per-surface shed budgets — the v1 floors
//! become measured numbers). **Doctrine:** `external-insights/01 §3` (MEASURED-not-predicted — the v1
//! floors become measured numbers; NEVER edited green without the drill, NEVER weakened to pass).
//!
//! ## What this drill is (the P-S33 deliverable, proven)
//! P-S33 tunes the §7.6 per-surface shed-budget NUMBERS to measured values: the SUB-D3 surge (P-S32)
//! plus the connection-storm drill (P-S31) drove the v1 floor numbers under real 30× surge and
//! connection-storm load; the three F6 properties held, so the numbers are now MEASURED
//! defaults-to-beat written into the FROZEN thresholds file (P-S22) as a dated update. The floor
//! DISCIPLINE (bounded + reserved human lane + shed order) is the UNCHANGED contract; only the
//! NUMBERS tune. This file proves the GATE the DoD names:
//!
//! 1. **The tuned numbers in the file VALIDATE** — every row is bounded, reservation-within-cap, and
//!    (for a human-facing surface) at-or-above the MEASURED human-lane floor
//!    ([`SurfaceBudget::HUMAN_LANE_FLOOR_BPS`] = 20% of cap). [`Thresholds::validate_shed_budgets`]
//!    is the load-time gate; the tuned file passes it.
//!
//! 2. **The human-lane-starvation regression** — a budget tuned BELOW the human-lane floor FAILS the
//!    gate (a LOUD [`ShedBudgetError::HumanLaneStarved`]). You cannot tune the human lane into
//!    starvation; the gate is un-bypassable from the file itself.
//!
//! 3. **Re-running SUB-D3 with the TUNED numbers still holds the three properties** — the human lane
//!    holds 0-shed, the machine lanes shed with `429 + Retry-After`, and a second tenant is
//!    unaffected — now against the budgets READ FROM THE FILE (the tuned numbers), not a hardcoded
//!    floor. This is the "re-running SUB-D3 with the tuned numbers still holds the human lane"
//!    regression the DoD requires.
//!
//! 4. **The thresholds-file update round-trips** — the dated tuned numbers parse → serialize → parse
//!    to the identical structure (no lossy edit).
//!
//! ## Coherence (EI-01 §7)
//! This file does NOT re-implement the shed lane or the load generator — it reuses
//! [`ShedLane`]/[`LoadGenerator`] verbatim (the same primitives SUB-D3 drives), and reads the tuned
//! budgets through the FROZEN [`Thresholds`] file via [`Thresholds::shed_budget_table_validated`].
//! It is the M5 budget-tuning follow-on the SUB-D3 drill (`drill_sub_d3_surge_family.rs`) names; the
//! discipline it proves is identical, against the tuned numbers rather than the v1 floor.
//!
//! ## Floors closed
//! - The §7.6 per-surface shed-budget **v1 floor → measured** follow-on (named in P-S19 / P-S32) is
//!   CLOSED here: the numbers are measured (drill-backed), the file carries them dated, and the
//!   human-lane floor is structurally enforced.

use std::collections::HashMap;

use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedBudgetError, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// Map a load request onto the substrate run-class (the SAME derivation the real gateway makes — no
/// parallel classifier; identical to SUB-D3's `run_class_of`).
fn run_class_of(req: &Request) -> RunClass {
    let header = match req.load_kind {
        LoadPrincipalKind::Ci | LoadPrincipalKind::Service | LoadPrincipalKind::ExternalMcp => {
            Some(RunClassHeader::BatchCi)
        }
        LoadPrincipalKind::Human | LoadPrincipalKind::Agent => None,
    };
    RunClass::derive(&req.principal_kind, header)
}

/// A sink that admits each request against a per-tenant [`ShedLane`] built from the TUNED budget, and
/// records the admit/shed verdict per `(tenant, lane)`. The realistic surge model (§7.4): a machine
/// run HOLDS its permit (the storm), an interactive human admits-then-completes (release) — identical
/// to the SUB-D3 sink so the regression is the SAME property against the tuned numbers.
struct TunedShedSink {
    lane: ShedLane,
    shed: HashMap<(String, &'static str), u64>,
    admit: HashMap<(String, &'static str), u64>,
    last_machine_retry_after: Option<u64>,
}

impl TunedShedSink {
    fn new(surface: Surface, budget: SurfaceBudget) -> TunedShedSink {
        TunedShedSink {
            lane: ShedLane::with_budget(surface, budget),
            shed: HashMap::new(),
            admit: HashMap::new(),
            last_machine_retry_after: None,
        }
    }

    fn shed_of(&self, tenant: &str, lane: &'static str) -> u64 {
        self.shed
            .get(&(tenant.to_string(), lane))
            .copied()
            .unwrap_or(0)
    }

    fn admit_of(&self, tenant: &str, lane: &'static str) -> u64 {
        self.admit
            .get(&(tenant.to_string(), lane))
            .copied()
            .unwrap_or(0)
    }
}

impl Sink for TunedShedSink {
    fn handle(&mut self, request: &Request) {
        let class = run_class_of(request);
        let tenant = request.tenant.as_str().to_string();
        match self.lane.admit(&request.tenant, class) {
            ShedDecision::Admit => {
                *self
                    .admit
                    .entry((tenant.clone(), class.lane()))
                    .or_insert(0) += 1;
                // humans + short batch/CI complete immediately (release); the AGENT lane HOLDS (the
                // sustained storm pressure the human lane must survive) — exactly the SUB-D3 model.
                if class != RunClass::Agent {
                    self.lane.release(&request.tenant, class);
                }
            }
            ShedDecision::Shed { retry_after_secs } => {
                *self.shed.entry((tenant.clone(), class.lane())).or_insert(0) += 1;
                if class != RunClass::Human {
                    self.last_machine_retry_after = Some(retry_after_secs);
                }
            }
        }
    }
}

/// Drive a 30× surge on one tenant + a baseline trickle on a second tenant, against the surface's
/// TUNED budget (read from the file), and assert the three SUB-D3 properties hold against the tuned
/// numbers. Shared by the per-surface cases below.
fn re_run_sub_d3_against_tuned_budget(surface: Surface, budget: SurfaceBudget, multiplier: u32) {
    let mut sink = TunedShedSink::new(surface, budget);

    let surge_tenant = TenantId("acme".into());
    let surge = LoadGenerator::new(
        64,
        Multiplier::custom(multiplier).expect("positive multiplier"),
        PrincipalMix::agent_skewed(),
        StormProfile::ci_surge(),
        vec![surge_tenant.clone()],
    )
    .expect("non-empty tenants");
    surge.drive(&mut sink);

    let other_tenant = TenantId("globex".into());
    let baseline = LoadGenerator::new(
        4,
        Multiplier::BASELINE,
        PrincipalMix::balanced(),
        StormProfile::ci_surge(),
        vec![other_tenant.clone()],
    )
    .expect("non-empty tenants");
    baseline.drive(&mut sink);

    // (1) the human lane held — 0 human sheds, and it carried real human traffic (earned, not vacuous).
    assert_eq!(
        sink.shed_of(surge_tenant.as_str(), "human"),
        0,
        "P-S33 RED: the TUNED budget for {surface:?} shed the protected human lane under the {multiplier}× surge \
         — the tuned numbers must STILL hold the human lane (threshold 0, NOT weakened, EI-01 §3)"
    );
    assert!(
        sink.admit_of(surge_tenant.as_str(), "human") > 0,
        "the surge carried human traffic against {surface:?} — the 0-shed result is earned"
    );

    // (2) the machine lanes shed with 429 + Retry-After (the surface's tuned Retry-After).
    let machine_sheds = sink.shed_of(surge_tenant.as_str(), "agent")
        + sink.shed_of(surge_tenant.as_str(), "batch_ci");
    assert!(
        machine_sheds > 0,
        "P-S33 RED: the TUNED budget for {surface:?} did not shed the machine lanes under a {multiplier}× surge"
    );
    assert_eq!(
        sink.last_machine_retry_after,
        Some(budget.retry_after_secs),
        "every machine-lane shed carries the surface's TUNED Retry-After (429 + Retry-After)"
    );

    // (3) the second tenant is unaffected (per-tenant bulkhead, 0 sheds).
    let other_sheds: u64 = ["human", "agent", "batch_ci", "speculative"]
        .iter()
        .map(|l| sink.shed_of(other_tenant.as_str(), l))
        .sum();
    assert_eq!(
        other_sheds, 0,
        "P-S33 RED: a surge on `acme` against {surface:?}'s tuned budget shed `globex` — per-tenant bulkhead failed"
    );
    assert!(
        sink.admit_of(other_tenant.as_str(), "human") > 0
            || sink.admit_of(other_tenant.as_str(), "agent") > 0
            || sink.admit_of(other_tenant.as_str(), "batch_ci") > 0,
        "the other tenant's baseline was admitted (its budget is its own)"
    );
}

/// **(1) The tuned numbers in the FROZEN file VALIDATE against the §7.6 human-lane floor.** The
/// load-time gate [`Thresholds::validate_shed_budgets`] passes on the canonical file — every tuned
/// row is bounded, reservation-within-cap, and (human-facing) at-or-above the measured 20% floor.
#[test]
fn the_tuned_shed_budgets_in_the_file_validate() {
    let t = Thresholds::load_canonical().expect("thresholds.toml loads");
    t.validate_shed_budgets()
        .expect("the TUNED shed budgets in the file must hold the §7.6 human-lane floor (P-S33)");

    // each human-facing surface's tuned reservation is at-or-above its measured floor (earned).
    for surface in [
        Surface::CollabOpStream,
        Surface::ConnectionTier,
        Surface::AgentMention,
        Surface::GitFrontDoor,
        Surface::RefsBacklinkRead,
        Surface::RefsRefCreate,
        Surface::SearchQuery,
        Surface::HttpIntake,
    ] {
        let b = t.shed_budget(surface).expect("present");
        let floor = SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap);
        assert!(
            b.human_lane_reservation >= floor,
            "{surface:?} tuned reservation {} is at-or-above the measured human-lane floor {} (cap {})",
            b.human_lane_reservation,
            floor,
            b.per_tenant_in_flight_cap,
        );
    }
    // CI dispatch is the documented batch-lane exemption (n/a human reservation) but still bounded.
    let ci = t.shed_budget(Surface::CiDispatch).expect("present");
    assert_eq!(
        ci.human_lane_reservation, 0,
        "CI is the batch lane (§7.6 n/a)"
    );
    assert!(
        ci.per_tenant_in_flight_cap > 0,
        "CI dispatch is still bounded"
    );
}

/// **(2) The human-lane-starvation regression (the P-S33 DoD): a budget tuned BELOW the human-lane
/// floor FAILS the gate.** A file whose ConnectionTier reservation is dropped to 4 (well under the
/// 20% floor) is REJECTED by [`Thresholds::validate_shed_budgets`] — you cannot tune the human lane
/// into starvation. The gate is un-bypassable: a hand-edit dropping a reservation fails at load.
#[test]
fn a_thresholds_file_that_starves_a_human_lane_fails_the_gate() {
    // a thresholds file IDENTICAL to canonical except ConnectionTier is starved (reservation 4 of 256).
    let starved_toml = r#"
        version = 1
        as_of = "2026-06-24"
        [revocation]
        sla_mins = 5
        [surge]
        multiplier = 30
        [fail_static]
        status = "OPEN — LEGAL"
        owner = "DPO / Legal"
        static_max_default_secs = 300
        agent_token_ttl_secs = 60
        constraint = "x"
        [rpo_rto]
        rpo_max_mins = 5
        rto_tenant_max_mins = 60
        rto_cell_max_mins = 240
        [depth_ceilings]
        soft = 12
        hard = 16
        [[shed_budgets]]
        surface = "ConnectionTier"
        per_tenant_in_flight_cap = 256
        human_lane_reservation = 4
        retry_after_secs = 3
    "#;
    let t = Thresholds::from_toml(starved_toml).expect("parses (the shape is valid)");
    let err = t
        .validate_shed_budgets()
        .expect_err("a starved human lane must FAIL the gate");
    match err {
        ShedBudgetError::HumanLaneStarved {
            surface,
            reservation,
            floor,
            ..
        } => {
            assert_eq!(surface, Surface::ConnectionTier);
            assert_eq!(reservation, 4);
            assert!(
                reservation < floor,
                "the gate caught a reservation under the measured floor — you cannot tune the human lane into starvation"
            );
        }
        other => panic!("expected HumanLaneStarved, got {other:?}"),
    }
}

/// **(3) Re-running SUB-D3 with the TUNED numbers (read from the file) still holds the three
/// properties** on every human-facing surface. This is the "re-driven through SUB-D3, still hold the
/// three properties" regression the DoD names — now against the tuned budgets, not the v1 floor.
#[test]
fn re_running_sub_d3_against_the_tuned_numbers_still_holds_the_human_lane() {
    let t = Thresholds::load_canonical().expect("load");
    let multiplier = t.surge.multiplier; // 30× from the file, never hardcoded.

    // the validated tuned table (a starved row would have failed validation above).
    let table = t
        .shed_budget_table_validated()
        .expect("the tuned table validates");

    // re-run the surge against each human-facing surface's TUNED budget.
    for surface in [
        Surface::HttpIntake,
        Surface::ConnectionTier,
        Surface::AgentMention,
        Surface::CollabOpStream,
        Surface::GitFrontDoor,
        Surface::RefsBacklinkRead,
        Surface::RefsRefCreate,
        Surface::SearchQuery,
    ] {
        let budget = table.budget(surface);
        re_run_sub_d3_against_tuned_budget(surface, budget, multiplier);
    }
}

/// **(4) The thresholds-file update round-trips.** The dated tuned numbers parse → serialize → parse
/// to the identical structure, and the shed-budget rows survive the round-trip surface-for-surface.
#[test]
fn the_tuned_thresholds_file_round_trips() {
    let t = Thresholds::load_canonical().expect("load");
    let serialized = t.to_toml().expect("serialize");
    let reparsed = Thresholds::from_toml(&serialized).expect("re-parse");
    assert_eq!(t, reparsed, "the tuned file round-trips (no lossy edit)");

    // the tuned shed-budget rows survive the round-trip surface-for-surface.
    for surface in [
        Surface::CiDispatch,
        Surface::CollabOpStream,
        Surface::ConnectionTier,
        Surface::AgentMention,
        Surface::GitFrontDoor,
        Surface::RefsBacklinkRead,
        Surface::RefsRefCreate,
        Surface::SearchQuery,
        Surface::HttpIntake,
    ] {
        assert_eq!(
            t.shed_budget(surface).expect("present"),
            reparsed.shed_budget(surface).expect("present"),
            "the tuned budget for {surface:?} survives the round-trip"
        );
    }
    // and the round-tripped file still validates (the tuning is not lost on serialize).
    reparsed
        .validate_shed_budgets()
        .expect("the round-tripped tuned file still holds the human-lane floor");
}

/// **The tuned file is in lock-step with the `shed::ShedBudgetTable::v1_floor()` table** (the CDC
/// invariant): the file is the source of truth, and the in-code table mirrors it surface-for-surface,
/// so a drill reading either gets the SAME tuned numbers (no drift between the file and the code).
#[test]
fn the_tuned_file_and_the_in_code_table_agree() {
    let t = Thresholds::load_canonical().expect("load");
    let table = myelin_substrate::shed::ShedBudgetTable::v1_floor();
    // the in-code table itself validates against the human-lane floor (no starved row in code either).
    table
        .validate()
        .expect("the in-code tuned table holds the human-lane floor");
    for surface in [
        Surface::CiDispatch,
        Surface::CollabOpStream,
        Surface::ConnectionTier,
        Surface::AgentMention,
        Surface::GitFrontDoor,
        Surface::RefsBacklinkRead,
        Surface::RefsRefCreate,
        Surface::SearchQuery,
        Surface::HttpIntake,
    ] {
        assert_eq!(
            t.shed_budget(surface).expect("present"),
            table.budget(surface),
            "the tuned file and the in-code table must agree for {surface:?} (one source of truth)"
        );
    }
}
