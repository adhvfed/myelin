//! REF-P29 → global P-514 (M6) — the reference-graph SWITCH TEST driven over the real surface.
//!
//! This is the prompt's required switch-test browser drive: the four-keystroke cross-artifact jump
//! across the five real subsystems (the failing CI check → the line of code → the issue → the
//! conversation, plus the Knowledge doc) driven over Myelin's own work, MEASURED against the latency
//! budgets read from the thresholds file (EI-01 §4: drive the real thing; EI-01 §3: never hardcode or
//! weaken the bar). It chains the deliverables (EI-01 §4 — chain operations, do not exercise handlers in
//! isolation):
//!
//! 1. **The four-keystroke jump works** — driving the real [`RefsSwitchTest`] (the SAME resolve
//!    chokepoint REF-P10 froze) reaches every capability the four-tool anchor (GitHub/Jira/Linear/Notion/
//!    Slack) has (0 walls), with 0 leak (the denied issue tombstones, root-only).
//! 2. **Measured within budget** — every leg (the backlink read / the per-viewer unfurl "within the
//!    keyboard" / the whole four-keystroke jump "no spinner flash") is within its budget, read from
//!    `thresholds.toml`, never hardcoded in the test and never weakened to pass.
//! 3. **The drill joins the permanent suite** — the switch test is registered into the harness
//!    [`DrillRegistry`] (the T-3 `register_drill` hook), which then RE-RUNS forever and stays green (a
//!    regression — a wall, a blown budget, or a leak — would re-red it loudly).
//!
//! ## Browser-driven vs only-automated (recorded HONESTLY — EI-01 §1/§4)
//!
//! The prompt requires we record yes/no/partial which switch-test surfaces were driven IN A BROWSER vs.
//! only automated. This host has no live browser harness wired to a rendered Refs web tier (the
//! production Refs web component / the `ResilientClient` production wire are NAMED FLOORS — the resolve/
//! traverse/backlink/tombstone ENGINE the browser would call IS built + driven + measured here). So
//! every switch-test surface is recorded as `AutomatedEngineNamedFloor` (browser-driven=no): the engine
//! is driven end-to-end and the latency legs are MEASURED, but the pixel-level browser drive over a
//! rendered pane is the honest named floor — never a claimed-but-unearned browser green (EI-01 §1).
//!
//! It is NOT behind the `integration` feature: the switch test's LOGIC runs in-process over the
//! production Refs engine. This drill proves the switch-test WIRING and joins the permanent `cargo test`
//! suite (it re-runs on every Myelin commit — wired as a Myelin CI job via the self-hosting CI graph,
//! the `REF-P29-switch-test` band).

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_refs_service::switch_test::{
    switch_surface_drive_record, BrowserDriveStatus, RefsSwitchTest, RefsSwitchVerdict,
};
use myelin_substrate::thresholds::Thresholds;

/// A dated run stamp (the switch-test CI run's date). Pinned so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// The number of repeats each measured leg is averaged over (damps scheduler noise — the measure is a
/// real wall-clock, not a hand-set literal).
const REPEATS: u32 = 64;

/// Load the canonical thresholds file (the real latency budgets the switch test measures against).
fn thresholds() -> Thresholds {
    Thresholds::load_canonical().expect("load thresholds.toml")
}

/// **(1) THE HEADLINE: the reference-graph switch test PASSES driven over the real surface.** The
/// four-keystroke cross-artifact jump (failing-test → line of code → issue → conversation) works without
/// hitting a wall the four-tool anchor (GitHub/Jira/Linear/Notion/Slack) didn't have (0 walls), 0 leak,
/// and every MEASURED leg is within its latency budget (read from the thresholds file, never weakened).
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
    // 0 walls vs the four-tool anchor — a GitHub/Jira/Linear/Notion user moves without hitting a wall.
    assert!(
        verdict.walls().is_empty(),
        "0 walls vs the four-tool anchor: {:?}",
        verdict.walls()
    );
    // 0 leak — the denied issue tombstoned (the four-tool anchor would leak the title in a 404 preview).
    assert!(!switch.leaked, "0 leak in the four-keystroke jump");

    // Every measured leg within its budget (read from thresholds.toml, never hardcoded).
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

/// **(2) The browser-drive record is HONEST.** Every switch-test surface is recorded
/// automated-engine / web-tier named floor (browser-driven=no), never a claimed-but-unearned browser
/// green — the resolve/unfurl/backlink/tombstone ENGINE is driven + measured, the pixel-level browser
/// drive over a rendered Refs pane is the honest named floor (EI-01 §1).
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
        println!("[REF-P29 SURFACE] {} — {}", s.surface, s.drive.token());
    }
}

/// **(3) The switch test joins the permanent drill suite + RE-RUNS green forever.** The switch test is
/// registered into the harness [`DrillRegistry`] (the T-3 `register_drill` hook) and driven twice green
/// — a regression (a wall, a blown latency budget, or a leak in the four-keystroke jump) would re-red it
/// loudly. This is the dogfood loop's guarantee: the switch test re-runs on every Myelin commit.
#[test]
fn the_switch_test_joins_the_permanent_drill_suite_and_re_runs_green() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "ref_p29_switch_test_four_keystroke_jump".to_string(),
        move |ctx: &mut DrillContext| {
            // The reproducing scenario: re-drive the switch test over the real surface and assert it is
            // whole (0 walls + 0 leak + every leg within budget — a regression re-reds this).
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

    // It RE-RUNS forever — drive it twice, green both times (the dogfood loop's guarantee).
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

/// **The switch-test spine end-to-end (EI-01 §4: chain the operations).** The full REF-P29 spine in one
/// chained run: drive the switch test over the real surface (0 walls + 0 leak + every leg within budget)
/// → record the honest browser-drive note → register the repro that re-runs green. The moat thesis — the
/// four-keystroke cross-artifact jump — is held on Myelin's own work, measured, no claimed browser green.
#[test]
fn switch_test_spine_end_to_end() {
    let t = thresholds();

    // (1) drive the switch test over the real surface → 0 walls + 0 leak + within budget.
    let switch = RefsSwitchTest::drive(&t, REPEATS);
    let verdict = switch.verdict();
    assert!(
        verdict.is_pass(),
        "the switch test is green driven over the real surface: {}",
        switch.summary(RUN_DATE)
    );

    // (2) the honest browser-drive note — every surface automated-engine / web-tier named floor.
    let record = switch_surface_drive_record();
    assert!(record
        .iter()
        .all(|s| s.drive == BrowserDriveStatus::AutomatedEngineNamedFloor));

    // (3) the repro joins the permanent suite + re-runs green.
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
