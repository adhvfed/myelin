use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_search::switch_test::{
    switch_surface_drive_record, BrowserDriveStatus, SearchSwitchTest, SearchSwitchVerdict,
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
    let switch = SearchSwitchTest::drive(&t, REPEATS);
    let verdict = switch.verdict();

    assert!(
        verdict.is_pass(),
        "the Search switch test must pass driven over the real surface: {} (walls={:?})",
        switch.summary(RUN_DATE),
        verdict.walls(),
    );
    assert!(
        verdict.walls().is_empty(),
        "0 walls vs the three-tool anchor: {:?}",
        verdict.walls()
    );
    assert!(!switch.leaked, "0 leak in the three interactive finds");

    if let SearchSwitchVerdict::Pass {
        latencies, budgets, ..
    } = &verdict
    {
        assert!(
            latencies.code_by_symbol_us <= budgets.code_by_symbol_budget_us,
            "code-by-symbol is within budget: {}µs <= {}µs",
            latencies.code_by_symbol_us,
            budgets.code_by_symbol_budget_us,
        );
        assert!(
            latencies.doc_by_content_us <= budgets.doc_by_content_budget_us,
            "doc-by-content is within budget: {}µs <= {}µs",
            latencies.doc_by_content_us,
            budgets.doc_by_content_budget_us,
        );
        assert!(
            latencies.issue_by_facet_us <= budgets.issue_by_facet_budget_us,
            "issue-by-facet is within budget: {}µs <= {}µs",
            latencies.issue_by_facet_us,
            budgets.issue_by_facet_budget_us,
        );
    } else {
        panic!("expected a Pass verdict: {}", switch.summary(RUN_DATE));
    }

    let line = switch.summary(RUN_DATE);
    assert!(
        line.contains("P-515 SEARCH SWITCH-TEST 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn the_browser_drive_record_is_honest() {
    let record = switch_surface_drive_record();
    assert!(
        record.len() >= 4,
        "every switch-test surface (code-by-symbol / doc-by-content / issue-by-facet / \
         per-viewer-correct) is recorded yes/no/partial"
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
        println!("[SRCH-P33 SURFACE] {} - {}", s.surface, s.drive.token());
    }
}

#[test]
fn the_switch_test_joins_the_permanent_drill_suite_and_re_runs_green() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "srch_p33_switch_test_three_finds".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let switch = SearchSwitchTest::drive(&t, 4);
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

    let switch = SearchSwitchTest::drive(&t, REPEATS);
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
        "srch_p33_switch_test_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let t = thresholds();
            let whole = SearchSwitchTest::drive(&t, 4).verdict().is_pass();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the switch-test repro re-runs green");

    println!(
        "[P-515 SWITCH-TEST GREEN {RUN_DATE}] the three interactive finds (code-by-symbol / \
         doc-by-content / issue-by-facet) driven over the real surface: 0 walls vs the three-tool \
         anchor, 0 leak, within the latency budgets; browser-driven=no (automated engine; the Search \
         results web tier is the honest named floor)"
    );
}
