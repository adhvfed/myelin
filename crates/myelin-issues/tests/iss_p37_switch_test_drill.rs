//! ISS-P37 → global P-520 (M6) — the Issues ISS-D14 SWITCH TEST driven over the real surface. THE DONE-BAR.
//!
//! This is the prompt's required switch-test drive: could a JIRA/LINEAR user complete the core loop
//! create → triage → plan → board → done WITHOUT a manual — MEASURED against the contrast + latency
//! budgets on the primary screens (S1/S3/S5/S6/S9/S10/S13/S17/S19), including the empty/loading/error/
//! permission/erased/agent-pending states (issue-tracker §6 M6-I10; VISION §3; EI-01 §4 — drive the real
//! surface, EI-01 §3 — never hardcode or weaken the bar)? It chains the deliverables (EI-01 §4 — chain
//! operations, do not exercise handlers in isolation):
//!
//! 1. **The switch test passes** — driving the real Issues surface (the SAME canonical [`IssueView`]
//!    `ViewSpec` views + the ONE WASM `render(parse(md)) === md` round-trip + the §2 state-pill /
//!    priority-badge / agent-pending / erased overlays + the primary-screen state matrix) reaches every
//!    capability the Jira/Linear anchor has (0 walls), the issue bodies round-trip at 100% (contract
//!    13.1), every overlay meets the WCAG 4.5:1 contrast floor (the design-manual §2 measured anchor),
//!    and every primary-screen state is reached.
//! 2. **Measured within budget** — the canonical-view render leg is within its latency budget and every
//!    overlay is at or above the contrast floor, both read from `thresholds.toml`, never hardcoded in the
//!    test and never weakened.
//! 3. **The drill joins the permanent suite** — the switch test is registered into the harness
//!    [`DrillRegistry`] (the T-3 `register_drill` hook), which then RE-RUNS forever and stays green (a
//!    regression — a wall, a blown budget, a broken round-trip, a sub-floor overlay, or an unreached
//!    state — would re-red it).
//!
//! ## Browser-driven vs only-automated (recorded HONESTLY — EI-01 §1/§4)
//!
//! The prompt requires we record yes/no/partial which switch-test surfaces were driven IN A BROWSER vs.
//! only automated. The view MODEL (the WASM-clean Rust the browser shell drives behind its `<Board>` /
//! `<Views>` components) is exercised + measured headlessly end-to-end here; a full Playwright drive over
//! the live `<Board>` `j/k`/drag/IME shell is the UI follow-on prompt's NAMED FLOOR. So every switch-test
//! surface is recorded as `AutomatedModelNamedFloor` (browser-driven=partial): the real spec build +
//! round-trip + overlay contrast + state matrix are driven end-to-end and MEASURED, but the pixel-level
//! browser drive over a mounted DOM is the honest named floor — never a claimed-but-unearned browser green
//! (EI-01 §1).
//!
//! It is NOT behind the `integration` feature: the switch test's LOGIC runs in-process over the
//! production Issues render path. This drill proves the switch-test WIRING and joins the permanent `cargo
//! test` suite (it re-runs on every Myelin commit — wired as a Myelin CI job via the self-hosting CI
//! graph, the `ISS-P37-switch-test` band).

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_issues::switch_test::{
    switch_surface_drive_record, BrowserDriveStatus, IssuesSwitchTest, IssuesSwitchVerdict,
    PrimaryScreenState,
};
use myelin_substrate::thresholds::Thresholds;

/// A dated run stamp (the switch-test CI run's date). Pinned so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// The number of repeats the measured render leg is averaged over (damps scheduler noise — the measure is
/// a real wall-clock, not a hand-set literal).
const REPEATS: u32 = 32;

/// Load the canonical thresholds file (the real budgets the switch test measures against).
fn thresholds() -> Thresholds {
    Thresholds::load_canonical().expect("load thresholds.toml")
}

/// **(1) THE HEADLINE: the Issues ISS-D14 switch test PASSES driven over the real surface.** The
/// canonical view renders within budget, every issue body round-trips (`render(parse(md)) === md` at
/// 100%), every overlay meets the WCAG 4.5:1 floor, every primary-screen state is reached, and the
/// Jira/Linear capability matrix (create→triage→plan→board→done) has 0 walls — a Jira/Linear user could
/// move without hitting a wall the old tool didn't have.
#[test]
fn the_switch_test_passes_driven_over_the_real_surface() {
    let t = thresholds();
    let switch = IssuesSwitchTest::drive(&t, REPEATS);
    let verdict = switch.verdict();

    assert!(
        verdict.is_pass(),
        "the Issues switch test must pass driven over the real surface: {} (walls={:?})",
        switch.summary(RUN_DATE),
        verdict.walls(),
    );
    // 0 walls vs the Jira/Linear anchor.
    assert!(
        verdict.walls().is_empty(),
        "0 walls vs the Jira/Linear anchor: {:?}",
        verdict.walls()
    );
    // render(parse(md)) === md at 100% (contract 13.1 / ISS-D10).
    assert_eq!(
        switch.legs.round_trip_ok,
        switch.legs.round_trip_total,
        "render(parse(md)) === md at 100%: {}",
        switch.summary(RUN_DATE)
    );
    // every primary-screen state reached (empty/loading/error/permission/erased/agent-pending).
    assert!(
        switch.legs.states_are_total(),
        "every primary-screen state reached: {}",
        switch.summary(RUN_DATE)
    );

    if let IssuesSwitchVerdict::Pass { legs, budgets, .. } = &verdict {
        assert!(
            legs.view_render_us <= budgets.view_render_budget_us,
            "the canonical-view render is within budget: {}µs <= {}µs",
            legs.view_render_us,
            budgets.view_render_budget_us,
        );
        assert!(
            legs.min_overlay_contrast_bp >= budgets.overlay_contrast_floor_bp,
            "every overlay meets the contrast floor: {}bp >= {}bp",
            legs.min_overlay_contrast_bp,
            budgets.overlay_contrast_floor_bp,
        );
    } else {
        panic!("expected a Pass verdict: {}", switch.summary(RUN_DATE));
    }

    let line = switch.summary(RUN_DATE);
    assert!(
        line.contains("P-520 ISSUES SWITCH-TEST 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

/// **(2) The browser-drive record is HONEST.** Every switch-test surface is recorded automated-model /
/// live-shell named floor (browser-driven=partial), never a claimed-but-unearned browser green — the real
/// spec build, round-trip, overlay contrast, and state matrix are driven + measured, while the pixel-level
/// browser drive over the live `<Board>` / `<Views>` shell is the honest named floor (EI-01 §1).
#[test]
fn the_browser_drive_record_is_honest() {
    let record = switch_surface_drive_record();
    assert!(
        record.len() >= 4,
        "every switch-test surface (render / round-trip / overlay / state-matrix) is recorded"
    );
    for s in &record {
        assert_eq!(
            s.drive,
            BrowserDriveStatus::AutomatedModelNamedFloor,
            "{} is honestly recorded as automated-model / live-shell named floor",
            s.surface
        );
        assert!(
            s.drive.token().contains("browser-driven=partial"),
            "the honest yes/no/partial token: {}",
            s.drive.token()
        );
        println!("[ISS-P37 SURFACE] {} — {}", s.surface, s.drive.token());
    }
}

/// **(3) The switch test joins the permanent drill suite + RE-RUNS green forever.** The switch test is
/// registered into the harness [`DrillRegistry`] (the T-3 `register_drill` hook) and driven twice green —
/// a regression (a wall, a blown render budget, a broken round-trip, a sub-floor overlay, or an unreached
/// state) would re-red it loudly. This is the dogfood loop's guarantee: the switch test re-runs on every
/// Myelin commit.
#[test]
fn the_switch_test_joins_the_permanent_drill_suite_and_re_runs_green() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "iss_p37_switch_test_canonical_view".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let pass = IssuesSwitchTest::drive(&t, 4).verdict().is_pass();
            ctx.signals
                .set_scalar(SignalName::DeadLetterCount, if pass { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        },
    ));
    assert_eq!(
        registry.len(),
        1,
        "the switch test joined the permanent suite"
    );

    let first = registry.run_all();
    let second = registry.run_all();
    assert!(
        first[0].is_pass(),
        "the registered switch-test drill must pass: {:?}",
        first[0]
    );
    assert!(second[0].is_pass(), "it re-runs green forever");
    assert!(
        registry.all_green(),
        "the suite is green with the switch test registered"
    );
}

/// **The switch-test spine end-to-end (EI-01 §4: chain the operations).** The full ISS-P37 switch-test
/// spine in one chained run: drive the switch test over the real surface (0 walls + 100% round-trip +
/// overlay ≥ floor + every state reached + render in budget) → record the honest browser-drive note →
/// register the repro that re-runs green. THE DONE-BAR — could a Jira/Linear user complete
/// create→triage→plan→board→done without a manual — held on Myelin's own work, measured, no claimed
/// browser green.
#[test]
fn switch_test_spine_end_to_end() {
    let t = thresholds();

    // (1) drive the switch test over the real surface → 0 walls + 100% round-trip + overlay ≥ floor +
    //     every state reached.
    let switch = IssuesSwitchTest::drive(&t, REPEATS);
    assert!(
        switch.verdict().is_pass(),
        "the switch test is green driven over the real surface: {}",
        switch.summary(RUN_DATE)
    );
    assert_eq!(
        switch.legs.states_reached,
        PrimaryScreenState::all().len(),
        "every primary-screen state reached: {}",
        switch.summary(RUN_DATE)
    );

    // (2) the honest browser-drive note — every surface automated-model / live-shell named floor.
    let record = switch_surface_drive_record();
    assert!(record
        .iter()
        .all(|s| s.drive == BrowserDriveStatus::AutomatedModelNamedFloor));

    // (3) the repro joins the permanent suite + re-runs green.
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "iss_p37_switch_test_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let whole = IssuesSwitchTest::drive(&t, 4).verdict().is_pass();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the switch-test repro re-runs green");

    println!(
        "[P-520 SWITCH-TEST GREEN {RUN_DATE}] the Issues ISS-D14 switch test driven over the real \
         surface: 0 walls vs the Jira/Linear anchor (create→triage→plan→board→done without a manual), \
         render(parse(md)) === md at 100%, every primary-screen overlay at ≥ 4.5:1 contrast, every \
         primary-screen state reached, the canonical-view render within budget; browser-driven=partial \
         (automated model driven + measured; the live <Board>/<Views> shell + a Playwright drive is the \
         honest named floor)"
    );
}
