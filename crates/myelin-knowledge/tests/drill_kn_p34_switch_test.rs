use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_knowledge::switch_test::{
    switch_surface_drive_record, BrowserDriveStatus, KnowledgeSwitchTest, KnowledgeSwitchVerdict,
};
use myelin_substrate::thresholds::Thresholds;

const RUN_DATE: &str = "2026-06-26";

const REPEATS: u32 = 32;

fn thresholds() -> Thresholds {
    Thresholds::load_canonical().expect("load thresholds.toml")
}

#[test]
fn the_switch_test_passes_driven_over_the_real_surface() {
    let t = thresholds();
    let switch = KnowledgeSwitchTest::drive(&t, REPEATS);
    let verdict = switch.verdict();

    assert!(
        verdict.is_pass(),
        "the Knowledge switch test must pass driven over the real surface: {} (walls={:?})",
        switch.summary(RUN_DATE),
        verdict.walls(),
    );
    assert!(
        verdict.walls().is_empty(),
        "0 walls vs the Notion anchor: {:?}",
        verdict.walls()
    );
    assert_eq!(
        switch.legs.round_trip_ok,
        switch.legs.round_trip_total,
        "render(parse(md)) === md at 100%: {}",
        switch.summary(RUN_DATE)
    );

    if let KnowledgeSwitchVerdict::Pass { legs, budgets, .. } = &verdict {
        assert!(
            legs.page_render_us <= budgets.page_render_budget_us,
            "the page render is within budget: {}µs <= {}µs",
            legs.page_render_us,
            budgets.page_render_budget_us,
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
        line.contains("P-519 KNOWLEDGE SWITCH-TEST 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn the_browser_drive_record_is_honest() {
    let record = switch_surface_drive_record();
    assert!(
        record.len() >= 4,
        "every switch-test surface (render / round-trip / overlay / per-viewer) is recorded"
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
        println!("[KN-P34 SURFACE] {} - {}", s.surface, s.drive.token());
    }
}

#[test]
fn the_switch_test_joins_the_permanent_drill_suite_and_re_runs_green() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "kn_p34_switch_test_page_render".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let pass = KnowledgeSwitchTest::drive(&t, 4).verdict().is_pass();
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

#[test]
fn switch_test_spine_end_to_end() {
    let t = thresholds();

    let switch = KnowledgeSwitchTest::drive(&t, REPEATS);
    assert!(
        switch.verdict().is_pass(),
        "the switch test is green driven over the real surface: {}",
        switch.summary(RUN_DATE)
    );

    let record = switch_surface_drive_record();
    assert!(record
        .iter()
        .all(|s| s.drive == BrowserDriveStatus::AutomatedModelNamedFloor));

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "kn_p34_switch_test_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let pass = KnowledgeSwitchTest::drive(&t, 2).verdict().is_pass();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if pass { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(
        registry.all_green(),
        "the switch-test spine repro re-runs green"
    );

    println!("{}", switch.summary(RUN_DATE));
}
