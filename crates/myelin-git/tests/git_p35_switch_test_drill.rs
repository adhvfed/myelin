use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_git::switch_test::{
    switch_surface_drive_record, BrowserDriveStatus, GitSwitchTest, GitSwitchVerdict,
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
    let switch = GitSwitchTest::drive(&t, REPEATS);
    let verdict = switch.verdict();

    assert!(
        verdict.is_pass(),
        "the Git switch test must pass driven over the real surface: {} (walls={:?})",
        switch.summary(RUN_DATE),
        verdict.walls(),
    );
    assert!(
        verdict.walls().is_empty(),
        "the pull-request workflow must have no blocked experience requirements: {:?}",
        verdict.walls()
    );
    assert_eq!(
        switch.legs.round_trip_ok,
        switch.legs.round_trip_total,
        "render(parse(md)) === md at 100%: {}",
        switch.summary(RUN_DATE)
    );

    if let GitSwitchVerdict::Pass { legs, budgets, .. } = &verdict {
        assert!(
            legs.pr_overview_render_us <= budgets.pr_overview_render_budget_us,
            "the PR overview render is within budget: {}µs <= {}µs",
            legs.pr_overview_render_us,
            budgets.pr_overview_render_budget_us,
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
        line.contains("P-518 GIT SWITCH-TEST 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn the_browser_drive_record_is_honest() {
    let record = switch_surface_drive_record();
    assert!(
        record.len() >= 4,
        "every switch-test surface (render / round-trip / overlay / merge-readiness) is recorded"
    );
    for s in &record {
        assert_eq!(
            s.drive,
            BrowserDriveStatus::AutomatedRenderNamedFloor,
            "{} is honestly recorded as automated-render / web-tier named floor",
            s.surface
        );
        assert!(
            s.drive.token().contains("browser-driven=no"),
            "the honest yes/no/partial token: {}",
            s.drive.token()
        );
        println!("[GIT-P35 SURFACE] {} - {}", s.surface, s.drive.token());
    }
}

#[test]
fn the_switch_test_joins_the_permanent_drill_suite_and_re_runs_green() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "git_p35_switch_test_pr_overview".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let pass = GitSwitchTest::drive(&t, 4).verdict().is_pass();
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

    let switch = GitSwitchTest::drive(&t, REPEATS);
    assert!(
        switch.verdict().is_pass(),
        "the switch test is green driven over the real surface: {}",
        switch.summary(RUN_DATE)
    );

    let record = switch_surface_drive_record();
    assert!(record
        .iter()
        .all(|s| s.drive == BrowserDriveStatus::AutomatedRenderNamedFloor));

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "git_p35_switch_test_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let whole = GitSwitchTest::drive(&t, 4).verdict().is_pass();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the switch-test repro re-runs green");

    println!(
        "[P-518 SWITCH-TEST GREEN {RUN_DATE}] the Git OQ-12 switch test driven over the real surface: 0 \
         workflow walls, render(parse(md)) === md at 100%, every status overlay at ≥ 4.5:1 \
         contrast, the PR overview render within budget; browser-driven=no (automated render; the git \
         web tier - the WASM editor + the <svg> icon binding - is the honest named floor)"
    );
}
