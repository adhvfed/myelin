//! KN-P34 → global P-519 (M6) — the Knowledge SWITCH TEST driven over the real surface. THE DONE-BAR.
//!
//! This is the prompt's required switch-test drive: could a NOTION user move to Myelin Knowledge WITHOUT
//! hitting a wall the old tool didn't have, MEASURED against the contrast + latency budgets +
//! `render(parse(md)) === md` against the real anchor (the design sketches / the design-manual
//! `02-components/block-editor.md` §2 one-render-path law; knowledge-platform §3 KN-M6; VISION §3; EI-01
//! §4 — drive the real surface, EI-01 §3 — never hardcode or weaken the bar)? It chains the deliverables
//! (EI-01 §4 — chain operations, do not exercise handlers in isolation):
//!
//! 1. **The switch test passes** — driving the real Knowledge surface (the SAME KN-P09 editor [`Document`]
//!    render + the ONE `render(parse(md)) === md` round-trip + the KN-P19 [`Projector`] tombstone + the
//!    reference-chip / tombstone overlays) reaches every capability the Notion anchor has (0 walls), the
//!    markdown round-trips at 100% (contract 13.1), and every chip overlay meets the WCAG 4.5:1 contrast
//!    floor (the design-manual §2 measured anchor).
//! 2. **Measured within budget** — the page render leg is within its latency budget and every overlay is
//!    at or above the contrast floor, both read from `thresholds.toml`, never hardcoded in the test and
//!    never weakened.
//! 3. **The drill joins the permanent suite** — the switch test is registered into the harness
//!    [`DrillRegistry`] (the T-3 `register_drill` hook), which then RE-RUNS forever and stays green (a
//!    regression — a wall, a blown budget, a broken round-trip, or a sub-floor overlay — would re-red it).
//!
//! ## Browser-driven vs only-automated (recorded HONESTLY — EI-01 §1/§4)
//!
//! The prompt requires we record yes/no/partial which switch-test surfaces were driven IN A BROWSER vs.
//! only automated. The integrated editor's WASM-clean MODEL (render/round-trip/projection a browser would
//! mount) is driven + measured headlessly end-to-end; a full Playwright drive against the live
//! `<BlockEditor>` `contenteditable` shell (real Chromium/Firefox caret variance, a real IME composition
//! event, paste-from-Word) is the UI follow-on prompt's NAMED FLOOR. So every switch-test surface is
//! recorded `AutomatedModelNamedFloor` (browser-driven=partial): the real model render + round-trip +
//! overlay contrast are driven and MEASURED, but the pixel-level browser drive over a mounted
//! contenteditable is the honest named floor — never a claimed-but-unearned browser green (EI-01 §1; the
//! recorded partial at `crates/myelin-knowledge/editor-browser-drive.md`).
//!
//! It is NOT behind the `integration` feature: the switch test's LOGIC runs in-process over the
//! production Knowledge render path. This drill proves the switch-test WIRING and joins the permanent
//! `cargo test` suite (it re-runs on every Myelin commit).

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_knowledge::switch_test::{
    switch_surface_drive_record, BrowserDriveStatus, KnowledgeSwitchTest, KnowledgeSwitchVerdict,
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

/// **(1) THE HEADLINE: the Knowledge switch test PASSES driven over the real surface.** The page renders
/// within budget, every page body round-trips (`render(parse(md)) === md` at 100%), every chip overlay
/// meets the WCAG 4.5:1 floor, and the Notion capability matrix has 0 walls — a Notion user could move
/// without hitting a wall the old tool didn't have.
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
    // 0 walls vs the Notion anchor.
    assert!(
        verdict.walls().is_empty(),
        "0 walls vs the Notion anchor: {:?}",
        verdict.walls()
    );
    // render(parse(md)) === md at 100% (contract 13.1, the §8b.2 one-render-path law).
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

/// **(2) The browser-drive record is HONEST.** Every switch-test surface is recorded automated-model /
/// live-shell named floor (browser-driven=partial), never a claimed-but-unearned full browser green — the
/// real model render, round-trip, and overlay contrast are driven + measured, while the pixel-level
/// browser drive over a mounted contenteditable is the honest named floor (EI-01 §1).
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
        println!("[KN-P34 SURFACE] {} — {}", s.surface, s.drive.token());
    }
}

/// **(3) The switch test joins the permanent drill suite + RE-RUNS green forever.** The switch test is
/// registered into the harness [`DrillRegistry`] (the T-3 `register_drill` hook) and driven twice green —
/// a regression (a wall, a blown render budget, a broken round-trip, or a sub-floor overlay) would re-red
/// it loudly. This is the dogfood loop's guarantee: the switch test re-runs on every Myelin commit.
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

/// **The switch-test spine end-to-end (EI-01 §4: chain the operations).** The full KN-P34 switch-test
/// spine in one chained run: drive the switch test over the real surface (0 walls + 100% round-trip +
/// overlay ≥ floor + render in budget) → record the honest browser-drive note → register the repro that
/// re-runs green. THE DONE-BAR — could a Notion user move to Myelin Knowledge — held on Myelin's own work,
/// measured, no claimed browser green.
#[test]
fn switch_test_spine_end_to_end() {
    let t = thresholds();

    // (1) drive the switch test over the real surface → 0 walls + 100% round-trip + overlay ≥ floor.
    let switch = KnowledgeSwitchTest::drive(&t, REPEATS);
    assert!(
        switch.verdict().is_pass(),
        "the switch test is green driven over the real surface: {}",
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
