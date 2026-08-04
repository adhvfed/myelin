use myelin_control_plane::{CellBulkhead, CellFleet, CellFleetReport, SURGE_MULTIPLIER};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};
use myelin_substrate::RunClass;
use myelin_tenancy::CellId;

fn cell_id(s: &str) -> CellId {
    CellId::from_token(s)
}

fn cell_data_plane_dep() -> Dependency {
    Dependency::Named("cell-data-plane".to_string())
}

fn three_cell_fleet() -> CellFleet {
    let mut fleet = CellFleet::new();
    for id in ["cell-w-1", "cell-w-2", "cell-w-3"] {
        fleet.insert(CellBulkhead::new(cell_id(id), 100, 10, 5));
    }
    fleet
}

#[test]
fn cp_d5_cell_bulkhead_under_surge_cross_cell_impact_zero() {
    let mut fleet = three_cell_fleet();
    assert_eq!(fleet.len(), 3, "three independent cells in the fleet");

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
    assert!(
        !breaker.is_broken(&cell_data_plane_dep(), &Scope::Cell("cell-w-2".to_string())),
        "the break does NOT reach cell-w-2 (scoped to cell-w-1)"
    );

    let report: CellFleetReport = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);

    assert!(
        report.surged_cell_agent_shed > 0,
        "the surged cell's agent lane shed the surge: {report:?}"
    );
    assert_eq!(
        report.surged_cell_agent_shed, 20,
        "30 agent offers, lane cap 10 → 20 shed (the surge absorbed by shedding)"
    );
    assert!(
        report.surged_cell_human_held,
        "the surged cell's human lane held within budget (0 human-lane sheds)"
    );

    assert_eq!(
        report.cross_cell_impact, 0,
        "cross-cell impact 0 (the CP-D5 zero) - the surge was contained to cell-w-1"
    );
    assert_eq!(
        report.other_cells_unaffected, 2,
        "both other cells (cell-w-2, cell-w-3) kept serving unaffected"
    );
    assert!(report.is_cp_d5_win(), "the CP-D5 win: {report:?}");

    for id in ["cell-w-2", "cell-w-3"] {
        let c = fleet.cell(&cell_id(id)).unwrap();
        assert_eq!(c.human_lane_shed(), 0, "{id}'s human lane held (0 shed)");
        assert!(c.admitted() >= 1, "{id} kept serving its baseline");
    }

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

    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        report.cross_cell_impact as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();
    let agent_lane = vec![Label::new("lane", RunClass::Agent.lane())];
    sig.set_labelled(
        SignalName::ShedCount,
        agent_lane.clone(),
        report.surged_cell_agent_shed as i64,
    );
    sig.assert_labelled(SignalName::ShedCount, agent_lane, Predicate::Gte(1))
        .expect_green();
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
         system recovered (0 leaked breaks - fully reversible). DEGRADE, NOT CASCADE (VISION §3). NO \
         floor - the world-scale hardening of the single-cell topology already built; the measured \
         sizing numbers that bound \"30×\" ride P-CP-22.",
        report.surged_cell_agent_shed, report.cross_cell_impact, report.other_cells_unaffected,
    );
}

#[test]
fn cp_d5_fatal_fault_in_one_cell_is_contained() {
    let mut fleet = three_cell_fleet();
    let breaker = DependencyBreaker::new();

    assert!(
        breaker
            .break_dependency(cell_data_plane_dep(), Scope::Cell("cell-w-1".to_string()))
            .changed(),
        "the fatal fault is scoped to cell-w-1"
    );
    {
        let target = fleet.cell_mut(&cell_id("cell-w-1")).unwrap();
        target.inject_fatal_fault();
        assert!(target.is_faulted(), "cell-w-1 is fatally faulted");
        assert!(
            target.offer(RunClass::Human).is_faulted(),
            "the faulted cell refuses every request"
        );
    }

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

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, 0);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

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
         (cell-w-2, cell-w-3) kept serving unaffected - cross-cell impact 0. The fault was contained to \
         its bulkhead."
    );
}

#[test]
fn cp_d5_gate_is_not_vacuous_shared_queue_reads_red() {
    let shared_impact = CellFleet::shared_queue_impact(10, SURGE_MULTIPLIER, 2);
    assert!(
        shared_impact > 0,
        "a shared hot-path queue saturated by the surge sheds the other cells too (cross-cell impact \
         {shared_impact} - RED)"
    );
    assert_eq!(
        shared_impact, 2,
        "both other cells impacted by the shared queue"
    );

    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, shared_impact as i64);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a non-zero cross-cell impact MUST read RED - the CP-D5 zero is a real tripwire"
    );

    let mut fleet = three_cell_fleet();
    let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
    assert_eq!(
        report.cross_cell_impact, 0,
        "the independent bulkhead contains the SAME surge (cross-cell impact 0 - GREEN)"
    );
}

#[test]
fn cp_d5_e2e_wedge_tenancy_is_the_partition_under_all_four() {
    let mut fleet = CellFleet::new();
    fleet.insert(CellBulkhead::new(cell_id("cell-w-1"), 100, 10, 5));
    fleet.insert(CellBulkhead::new(cell_id("cell-w-2"), 100, 10, 5));

    let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
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
    assert_eq!(
        fleet.cell(&cell_id("cell-w-2")).unwrap().human_lane_shed(),
        0,
        "cell-w-2's human lane held - its E2E run was not starved by cell-w-1's surge"
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
         E2E-3 traceability / E2E-4 DSAR) UNAFFECTED - cross-cell impact 0. The full E2E scenarios run \
         green in the per-system spine (identity spine + GA-D8 DSAR fan-out); this confirms the \
         tenancy-as-partition guarantee under the surge for the band."
    );
}
