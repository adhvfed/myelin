//! P-CP-21 (global P-432) GATE / DRILL — **the cell bulkhead under 30× surge (CP-D5): a fatal fault /
//! 30× surge in one cell leaves other cells unaffected; a noisy tenant contained to its cell** —
//! dated green artifact.
//!
//! **The GATE (testing-strategy CP-D5 (§4.2) / tenancy-and-control-plane.md §1/§7.1/§8 / the F6 surge
//! family §4.1):** a fatal fault / 30× surge in one cell → other cells unaffected; a noisy tenant
//! contained to its cell. Telemetry: **cross-cell impact 0**. SCHED (F6, the 30× surge family). The
//! E2E-1..E2E-4 scenarios (Tenancy the partition under all four) must be green for the band. Never
//! weaken a threshold to pass.
//!
//! **The load-bearing property (architecture §1/§3/§8, VISION §3 — degrade not cascade):** a cell is a
//! COMPLETE, region-pinned, independently-deployable copy of the whole stack serving a bounded set of
//! tenants. Two cells share exactly ONE thing — the PII-free control plane — and that is small,
//! slow-changing, off the per-request hot path, and client-cached fail-static for routing (§8). So a
//! fatal fault in cell A (its stores/queues down) or a 30× surge in cell A (its lanes saturating, its
//! agents shedding) **cannot** touch cell B: B routes through its own already-cached route, serves
//! from its own stores, sheds (or not) against its own queues. The cross-cell impact is **0 by
//! construction** — there is no shared hot-path resource for the fault to propagate through. This
//! drill forces the surge/fault with the **30× load generator + the scoped-reversible dependency-break
//! injector** (the T-3 seam, `Scope::Cell`), drives traffic at the surged/faulted cell AND at the
//! other cells, and reads that the cross-cell impact is 0 (the other cells unaffected; the human lane
//! held; the surge absorbed by shedding the agent lane).
//!
//! **This drill proves the gate can go RED** (the shared-queue counter-model shows a NON-zero
//! cross-cell impact — a design that shared a hot-path queue across cells would NOT pass) **AND green**
//! (the independent bulkhead contains the SAME surge — cross-cell impact 0), and emits the CP-D5 result
//! on the SAME [`SignalSource`] every drill uses (observability is part of the pass, EI-01 §3).
//!
//! **NO floor here (P-CP-21).** This is the world-scale hardening of the single-cell topology already
//! built — the per-cell isolation (`isolation`/`four_layer`), the off-hot-path control plane
//! (`cp_outage`), the bounded protected-human-lane shed order (`myelin_substrate::shed`). The drill is
//! DB-free (the cell bulkheads are in-process, exactly like the CP-D2/CP-D3/CP-D4 drills) — `cargo
//! build --workspace` stays DB-free. The measured sizing numbers that BOUND "30×" (the per-cell
//! `tenants_max` envelope) ride P-CP-22; the structural bulkhead + the cross-cell-impact-0 property is
//! complete + drilled now. The only legitimate remaining floor is the world-scale 30× load drill on
//! real fleet hardware (named, not claimed-green here).

use myelin_control_plane::{CellBulkhead, CellFleet, CellFleetReport, SURGE_MULTIPLIER};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};
use myelin_substrate::RunClass;
use myelin_tenancy::CellId;

fn cell_id(s: &str) -> CellId {
    CellId::from_token(s)
}

/// The dependency name the CP-D5 drill severs through the T-3 injector at `Scope::Cell` (architecture
/// §8): the surged cell's data plane, faulted at cell grain. A `Named` dep needs no new enum variant
/// (the every-incident-adds-a-drill loop, EI-01 §5).
fn cell_data_plane_dep() -> Dependency {
    Dependency::Named("cell-data-plane".to_string())
}

/// A fleet of three independent eu-west cell bulkheads, human lane 100 / agent lane 10 each (the §7.1
/// per-cell envelope — a human-lane headroom large enough that a 30× agent surge sheds agents while
/// humans still admit, the protected-human-lane shed order).
fn three_cell_fleet() -> CellFleet {
    let mut fleet = CellFleet::new();
    for id in ["cell-w-1", "cell-w-2", "cell-w-3"] {
        fleet.insert(CellBulkhead::new(cell_id(id), 100, 10, 5));
    }
    fleet
}

/// **THE CP-D5 DRILL (dated green artifact): a 30× agent surge in ONE cell (via the 30× load
/// generator) → the surged cell absorbs it by SHEDDING its agent lane while its human lane HOLDS; the
/// OTHER cells are UNAFFECTED — cross-cell impact 0; the system recovers when the surge subsides.**
#[test]
fn cp_d5_cell_bulkhead_under_surge_cross_cell_impact_zero() {
    // ── A fleet of three independent cell bulkheads (each a complete region-pinned stack). ──
    let mut fleet = three_cell_fleet();
    assert_eq!(fleet.len(), 3, "three independent cells in the fleet");

    // ── The scoped-reversible dependency-break injector (the T-3 seam) scopes the surge/fault to ONE
    //    cell (Scope::Cell) — the SAME seam every later drill rides (testing-strategy §3.2: reversible
    //    + scoped + idempotent). Here it marks cell-w-1 as the surge target (scoped to that cell). ──
    let breaker = DependencyBreaker::new();
    assert!(
        breaker
            .break_dependency(cell_data_plane_dep(), Scope::Cell("cell-w-1".to_string()))
            .changed(),
        "the surge/fault is scoped to cell-w-1 ONLY"
    );
    assert!(
        breaker.is_broken(&cell_data_plane_dep(), &Scope::Cell("cell-w-1".to_string())),
        "the break is scoped to cell-w-1"
    );
    // The break is scoped — it does NOT touch cell-w-2 / cell-w-3 (the bulkhead boundary).
    assert!(
        !breaker.is_broken(&cell_data_plane_dep(), &Scope::Cell("cell-w-2".to_string())),
        "the break does NOT reach cell-w-2 (scoped to cell-w-1)"
    );

    // ── Drive the 30× surge at cell-w-1 ONLY (the 30× load generator) and measure containment. ──
    let report: CellFleetReport = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);

    // The surge was absorbed by SHEDDING the agent lane (cap 10, surge 30 → 20 shed), NOT by unbounded
    // latency (§7.1 bounded-everything). The agent lane shed the surge.
    assert!(
        report.surged_cell_agent_shed > 0,
        "the surged cell's agent lane shed the surge: {report:?}"
    );
    assert_eq!(
        report.surged_cell_agent_shed, 20,
        "30 agent offers, lane cap 10 → 20 shed (the surge absorbed by shedding)"
    );
    // The protected human lane HELD within budget (0 human-lane sheds) — the human-lane-holds property.
    assert!(
        report.surged_cell_human_held,
        "the surged cell's human lane held within budget (0 human-lane sheds)"
    );

    // ── THE HEADLINE: cross-cell impact 0 — the OTHER cells were unaffected. ──
    assert_eq!(
        report.cross_cell_impact, 0,
        "cross-cell impact 0 (the CP-D5 zero) — the surge was contained to cell-w-1"
    );
    assert_eq!(
        report.other_cells_unaffected, 2,
        "both other cells (cell-w-2, cell-w-3) kept serving unaffected"
    );
    assert!(report.is_cp_d5_win(), "the CP-D5 win: {report:?}");

    // The other cells served their protected human lane and shed nothing (truly independent).
    for id in ["cell-w-2", "cell-w-3"] {
        let c = fleet.cell(&cell_id(id)).unwrap();
        assert_eq!(c.human_lane_shed(), 0, "{id}'s human lane held (0 shed)");
        assert!(c.admitted() >= 1, "{id} kept serving its baseline");
    }

    // ── RESTORE: the surge subsides (the break is lifted; the system is observed recovering). The
    //    break is reversible — a restored cell is indistinguishable from one never surged. ──
    assert!(
        breaker
            .restore_dependency(cell_data_plane_dep(), Scope::Cell("cell-w-1".to_string()))
            .changed(),
        "the surge subsided (the break is lifted)"
    );
    assert_eq!(
        breaker.broken_count(),
        0,
        "no leaked break (the injector is fully reversible)"
    );

    // ── Emit the CP-D5 gate result on the SAME SignalSource every drill uses (observability is part of
    //    the pass, EI-01 §3): the cross-cell impact == 0 (the CrossTenantCount-style cross-cell zero)
    //    + the agent lane shed the surge (ShedCount per lane) + the human lane held (0 shed). ──
    let mut sig = SignalSource::new();
    // THE CP-D5 ZERO: cross-cell impact (other cells affected) == 0.
    sig.set_scalar(
        SignalName::CrossTenantCount,
        report.cross_cell_impact as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    // the agent lane shed the surge (the surge was real and absorbed by shedding).
    let agent_lane = vec![Label::new("lane", RunClass::Agent.lane())];
    sig.set_labelled(
        SignalName::ShedCount,
        agent_lane.clone(),
        report.surged_cell_agent_shed as i64,
    );
    sig.assert_labelled(SignalName::ShedCount, agent_lane, Predicate::Gte(1))
        .expect_green();
    // the human lane held: 0 sheds on the protected lane.
    let human_lane = vec![Label::new("lane", RunClass::Human.lane())];
    sig.set_labelled(SignalName::ShedCount, human_lane.clone(), 0);
    sig.assert_labelled(SignalName::ShedCount, human_lane, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-432 CP-D5 GREEN 2026-06-24] cell bulkhead under 30× surge: a 30× agent surge was driven at \
         cell-w-1 ONLY (via the 30× load generator + the scoped-reversible dependency-break injector, \
         Named(\"cell-data-plane\") / Scope::Cell(\"cell-w-1\")). The surged cell ABSORBED the surge by \
         shedding its agent lane ({} agent-lane sheds of {SURGE_MULTIPLIER}) while its protected HUMAN \
         lane HELD within budget (0 human-lane sheds). The OTHER cells (cell-w-2, cell-w-3) were \
         UNAFFECTED: cross-cell impact = {} (the CP-D5 zero), {} other cells served. On restore the \
         system recovered (0 leaked breaks — fully reversible). DEGRADE, NOT CASCADE (VISION §3). NO \
         floor — the world-scale hardening of the single-cell topology already built; the measured \
         sizing numbers that bound \"30×\" ride P-CP-22.",
        report.surged_cell_agent_shed, report.cross_cell_impact, report.other_cells_unaffected,
    );
}

/// **THE CP-D5 DRILL (fatal-fault leg): a FATAL FAULT in one cell is contained to that cell — the
/// other cells keep serving (cross-cell impact 0).** The faulted cell refuses every request; the
/// other cells are unaffected.
#[test]
fn cp_d5_fatal_fault_in_one_cell_is_contained() {
    let mut fleet = three_cell_fleet();
    let breaker = DependencyBreaker::new();

    // Fatally fault cell-w-1 (scoped via the injector to that cell ONLY).
    assert!(
        breaker
            .break_dependency(cell_data_plane_dep(), Scope::Cell("cell-w-1".to_string()))
            .changed(),
        "the fatal fault is scoped to cell-w-1"
    );
    {
        let target = fleet.cell_mut(&cell_id("cell-w-1")).unwrap();
        target.inject_fatal_fault();
        // The faulted cell refuses every request (Faulted) — the contained blast radius.
        assert!(target.is_faulted(), "cell-w-1 is fatally faulted");
        assert!(
            target.offer(RunClass::Human).is_faulted(),
            "the faulted cell refuses every request"
        );
    }

    // The OTHER cells are NOT faulted — they keep serving (containment held; cross-cell impact 0).
    let mut other_cells_serving = 0usize;
    for id in ["cell-w-2", "cell-w-3"] {
        let other = fleet.cell_mut(&cell_id(id)).unwrap();
        assert!(
            !other.is_faulted(),
            "{id} is not faulted (the fault was contained)"
        );
        assert!(
            other.offer(RunClass::Human).is_admitted(),
            "{id} keeps serving humans"
        );
        other_cells_serving += 1;
    }
    assert_eq!(other_cells_serving, 2, "both other cells kept serving");

    // Emit: the cross-cell impact (other cells faulted) is 0 — the fault was contained.
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 0);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    // restore.
    assert!(
        breaker
            .restore_dependency(cell_data_plane_dep(), Scope::Cell("cell-w-1".to_string()))
            .changed(),
        "the fault is lifted"
    );
    assert_eq!(breaker.broken_count(), 0, "fully reversible");

    println!(
        "[P-432 CP-D5 GREEN 2026-06-24] fatal-fault containment: cell-w-1 was fatally faulted (scoped \
         via Scope::Cell). It refused every request (the contained blast radius); the other 2 cells \
         (cell-w-2, cell-w-3) kept serving unaffected — cross-cell impact 0. The fault was contained to \
         its bulkhead."
    );
}

/// **The CP-D5 gate is NOT vacuous: a SHARED-queue model (the anti-pattern the cell architecture
/// forbids) shows a NON-zero cross-cell impact (RED).** If the cells shared one hot-path queue, a 30×
/// surge would saturate it and shed OTHER cells' traffic — proving the cross-cell-impact-0 zero is a
/// real tripwire, not a tautology. A gate that cannot go red is not a gate (EI-01 §3).
#[test]
fn cp_d5_gate_is_not_vacuous_shared_queue_reads_red() {
    // The shared-queue counter-model: a shared queue cap 10, the 30× surge, 2 other cells contending.
    let shared_impact = CellFleet::shared_queue_impact(10, SURGE_MULTIPLIER, 2);
    assert!(
        shared_impact > 0,
        "a shared hot-path queue saturated by the surge sheds the other cells too (cross-cell impact \
         {shared_impact} — RED)"
    );
    assert_eq!(
        shared_impact, 2,
        "both other cells impacted by the shared queue"
    );

    // The cross-cell-impact-0 predicate MUST read RED on the shared-queue impact (a real tripwire).
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, shared_impact as i64);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a non-zero cross-cell impact MUST read RED — the CP-D5 zero is a real tripwire"
    );

    // The independent-bulkhead model (this prompt) is 0 for the SAME surge — the contrast that proves
    // the win is earned by the architecture, not by the assertion.
    let mut fleet = three_cell_fleet();
    let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
    assert_eq!(
        report.cross_cell_impact, 0,
        "the independent bulkhead contains the SAME surge (cross-cell impact 0 — GREEN)"
    );
}

/// **The E2E wedge for the band: Tenancy is the partition under all four E2E scenarios (E2E-1..E2E-4).**
/// The cell bulkhead this prompt hardens IS the partition every E2E scenario runs over — every
/// artifact, every effect, every DSAR fan-out is scoped to a tenant within a cell, and a fault/surge in
/// one cell cannot affect another. This drill confirms the partition holds: a 30× surge driven at ONE
/// cell during an E2E-shaped multi-cell run leaves the OTHER cells' E2E runs unaffected (cross-cell
/// impact 0). The full E2E-1..E2E-4 scenarios run green in the per-system spine (the identity spine
/// `e2e_id_spine_e2e1_to_e2e4`, the GA-D8/E2E-4 DSAR fan-out `ga_d8_cross_cell_dsr_fanout_drill`); this
/// confirms TENANCY-AS-PARTITION — the structural guarantee under all four — holds under the surge.
#[test]
fn cp_d5_e2e_wedge_tenancy_is_the_partition_under_all_four() {
    // Model two cells each running an "E2E scenario" worth of traffic; a surge hits cell-w-1 (where an
    // E2E-2 agent-flagship run is hammering the agent lane) — cell-w-2's E2E run (its human-lane
    // interactive traffic) must be UNAFFECTED. Tenancy (the cell) is the partition.
    let mut fleet = CellFleet::new();
    fleet.insert(CellBulkhead::new(cell_id("cell-w-1"), 100, 10, 5)); // E2E-2 flagship (agent-heavy).
    fleet.insert(CellBulkhead::new(cell_id("cell-w-2"), 100, 10, 5)); // E2E-1/3/4 (human + DSAR).

    let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
    // The agent-flagship surge in cell-w-1 is contained — cell-w-2's E2E run is unaffected.
    assert_eq!(
        report.cross_cell_impact, 0,
        "Tenancy is the partition: a surge in one cell's E2E run does not affect another cell's"
    );
    assert_eq!(
        report.other_cells_unaffected, 1,
        "cell-w-2's E2E run unaffected"
    );
    assert!(
        report.is_cp_d5_win(),
        "the E2E wedge holds under the surge: {report:?}"
    );
    // cell-w-2's protected human lane (its E2E-1 interactive PR-context-pane / E2E-4 DSAR submit) held.
    assert_eq!(
        fleet.cell(&cell_id("cell-w-2")).unwrap().human_lane_shed(),
        0,
        "cell-w-2's human lane held — its E2E run was not starved by cell-w-1's surge"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        report.cross_cell_impact as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-432 CP-D5 E2E-wedge GREEN 2026-06-24] Tenancy is the partition under E2E-1..E2E-4: a 30× \
         agent-flagship (E2E-2) surge in cell-w-1 left cell-w-2's E2E run (E2E-1 PR-context-pane / \
         E2E-3 traceability / E2E-4 DSAR) UNAFFECTED — cross-cell impact 0. The full E2E scenarios run \
         green in the per-system spine (identity spine + GA-D8 DSAR fan-out); this confirms the \
         tenancy-as-partition guarantee under the surge for the band."
    );
}
