use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_refs_service::switch_test::{
    switch_surface_drive_record, BrowserDriveStatus, RefsSwitchTest, RefsSwitchVerdict,
};
use myelin_substrate::thresholds::Thresholds;

const RUN_DATE: &str = "2026-06-26";

const REPEATS: u32 = 64;

fn thresholds() -> Thresholds {
    Thresholds::load_canonical().expect("load thresholds.toml")
}

#[test]
fn the_switch_test_passes_driven_over_the_real_surface() {
    let t = thresholds();
    let switch = RefsSwitchTest::drive(&t, REPEATS);
    let verdict = switch.verdict();

    assert!(
        verdict.is_pass(),
        "the reference-graph switch test must pass driven over the real surface: {} (walls={:?})",
        switch.summary(RUN_DATE),
        verdict.walls(),
    );
    assert!(
        verdict.walls().is_empty(),
        "0 walls vs the four-tool anchor: {:?}",
        verdict.walls()
    );
    assert!(!switch.leaked, "0 leak in the four-keystroke jump");

    if let RefsSwitchVerdict::Pass {
        latencies, budgets, ..
    } = &verdict
    {
        assert!(
            latencies.jump_us <= budgets.jump_no_spinner_budget_us,
            "the four-keystroke jump is within the no-spinner-flash budget: {}µs <= {}µs",
            latencies.jump_us,
            budgets.jump_no_spinner_budget_us,
        );
        assert!(
            latencies.unfurl_us <= budgets.unfurl_budget_us,
            "the unfurl is within the keyboard budget: {}µs <= {}µs",
            latencies.unfurl_us,
            budgets.unfurl_budget_us,
        );
        assert!(
            latencies.backlink_read_us <= budgets.backlink_read_budget_us,
            "the backlink read is within budget: {}µs <= {}µs",
            latencies.backlink_read_us,
            budgets.backlink_read_budget_us,
        );
    } else {
        panic!("expected a Pass verdict: {}", switch.summary(RUN_DATE));
    }

    let line = switch.summary(RUN_DATE);
    assert!(
        line.contains("P-514 REFS SWITCH-TEST 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn the_browser_drive_record_is_honest() {
    let record = switch_surface_drive_record();
    assert!(
        record.len() >= 5,
        "every switch-test surface (the jump / the unfurl / the backlink / the tombstone / the live \
         unfurl) is recorded yes/no/partial"
    );
    for s in &record {
        assert_eq!(
            s.drive,
            BrowserDriveStatus::AutomatedEngineNamedFloor,
            "{} is honestly recorded as automated-engine / web-tier named floor (no claimed browser \
             green)",
            s.surface
        );
        assert!(
            s.drive.token().contains("browser-driven=no"),
            "the honest yes/no/partial token: {}",
            s.drive.token()
        );
        println!("[REF-P29 SURFACE] {} - {}", s.surface, s.drive.token());
    }
}

#[test]
fn the_switch_test_joins_the_permanent_drill_suite_and_re_runs_green() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "ref_p29_switch_test_four_keystroke_jump".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let switch = RefsSwitchTest::drive(&t, 8);
            let pass = switch.verdict().is_pass();
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

    let switch = RefsSwitchTest::drive(&t, REPEATS);
    let verdict = switch.verdict();
    assert!(
        verdict.is_pass(),
        "the switch test is green driven over the real surface: {}",
        switch.summary(RUN_DATE)
    );

    let record = switch_surface_drive_record();
    assert!(record
        .iter()
        .all(|s| s.drive == BrowserDriveStatus::AutomatedEngineNamedFloor));

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "ref_p29_switch_test_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let whole = RefsSwitchTest::drive(&t, 4).verdict().is_pass();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the switch-test repro re-runs green");

    println!(
        "[P-514 SWITCH-TEST GREEN {RUN_DATE}] the four-keystroke cross-artifact jump driven over the \
         real surface: 0 walls vs the four-tool anchor, 0 leak, within the latency budgets; \
         browser-driven=no (automated engine; the Refs web tier is the honest named floor)"
    );
}
