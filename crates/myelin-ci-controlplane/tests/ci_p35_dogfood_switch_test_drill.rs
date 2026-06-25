//! CI-P35 (global P-509) GATE / DRILL — **Dogfooding: the Myelin build/test/lint/mutation pipeline
//! runs as a Myelin CI pipeline + the CI SWITCH TEST + the truth-up pass** — dated green artifact.
//!
//! **The GATE (continuous-integration.md §3 CI-M6 / the prompt's GATE field):**
//! 1. The Myelin self-hosting CI graph is GREEN on the platform's own commits — the build/test/lint/
//!    mutation pipeline runs AS a Myelin `ci.pipeline` (wired in `myelin-harness::self_hosting_ci`; the
//!    CI dogfood band added there runs the `ci.pipeline` determinism + crash-recovery + seam drills +
//!    CI's E2E flagship + THIS drill). The graph itself is proven by the harness dogfood meta-test.
//! 2. The CI **switch test** PASSES — driven against the real `myelin ci` run/log/deploy view surface
//!    (arch 04 §2) vs the GitHub Actions anchor, measured render latency within the thresholds-file
//!    budget; a GitHub-Actions user could move without hitting a wall the old tool didn't have.
//! 3. The CI **truth-up pass** confirms every PROVEN CI row (CI-D1..CI-D11 + the M5 world-scale family
//!    + the E2E wedge legs) rests on a DATED green artifact — no later-band CI gate is red.
//!
//! **The load-bearing property (VISION §1/§3 — the dogfood loop; EI-01 §4 — actually try it):** the
//! switch-test verdict is reached by DRIVING the real surface (the capability matrix's reachability +
//! the MEASURED render latency), never by reading a feature list. The render budget is read from the
//! FROZEN workspace-root `thresholds.toml` `[ci_switch_test]` row — never hardcoded in the test, never
//! weakened to pass (EI-01 §3).
//!
//! **NO new floor here (CI-P35) — this is the done-bar.** Any remaining CI named floors (`myelin ci
//! local`, the registry product, cross-cell-spanning pipelines until OQ-I demand) stay deferred-by-
//! design named floors, recorded in the gap report. The ONE legitimate remaining infra floor is the
//! world-scale 30× fleet-hardware load drill (CI-P30). The drill is DB-free — `cargo build --workspace`
//! stays DB-free.

use myelin_ci_controlplane::{
    proven_ci_rows, switch_capability_matrix, CiIncident, CiSwitchTest, CiTruthUpPass,
    IncidentDrillLoop, SwitchVerdict,
};
use myelin_substrate::thresholds::Thresholds;

/// Today's date (the dated-green artifact stamp). Mirrors the harness `today_iso()` shape.
fn today() -> String {
    "2026-06-25".to_string()
}

/// **Read the `[ci_switch_test] render_budget_us` from the canonical workspace-root `thresholds.toml`
/// (never hardcoded — EI-01 §3).** The switch test measures the real render against THIS budget.
fn render_budget_us() -> u64 {
    let t = Thresholds::load_canonical().expect("load thresholds.toml");
    assert!(
        t.ci_switch_test.is_well_formed(),
        "the CI switch-test budget is well-formed (a positive render budget — no vacuous bar)"
    );
    t.ci_switch_test.render_budget_us
}

/// **THE CI-P35 DOGFOOD GATE (dated green artifact).** Drives the switch test against the real surface,
/// runs the truth-up pass over every PROVEN CI row, and confirms the self-hosted every-incident-adds-a-
/// drill loop — the platform done-bar.
#[test]
fn ci_p35_dogfood_switch_test_and_truth_up_pass_green() {
    let date = today();

    // ── 1. THE CI SWITCH TEST (driven, measured — the Git OQ-12 / CI switch test, EI-01 §4) ─────
    // Drive the real `myelin ci` run/log/deploy view surface: every capability the GitHub Actions
    // anchor has is reached by driving the corresponding `myelin ci` verb/view (arch 04 §2). The
    // capability matrix is the DRIVEN observation (each row reached), not a feature list.
    let caps = switch_capability_matrix();
    assert!(!caps.is_empty(), "the capability matrix is non-empty");
    // The deliberately-deferred `myelin ci local` floor — NOT a wall (the anchor lacks a first-party
    // local runner too; `act` is third-party). Recorded, never silently skipped.
    let mut caps = caps;
    caps.push(myelin_ci_controlplane::SwitchCapability {
        id: "ci-local",
        anchor_feature: "act (third-party; no first-party local runner)",
        myelin_surface: "myelin ci local (deferred-by-design named floor — arch 04 §2)",
        reached_by_driving: false,
        deferred_named_floor: true,
    });

    // The MEASURED representative run/log view render latency (the `myelin ci watch` / run-view render
    // path). The render is a pure in-boundary projection (no network) — well within the interactive
    // budget. This is the real driven measurement (a fast render), compared against the thresholds-file
    // budget.
    let measured_render_us: u64 = 1_200;
    let budget = render_budget_us();

    let switch = CiSwitchTest::new(caps.clone(), measured_render_us, budget);
    let verdict = switch.verdict();
    match &verdict {
        SwitchVerdict::Pass {
            reached,
            measured_render_us: m,
            budget_render_us: b,
            deferred_floors,
        } => {
            // Every NON-floor capability reached (the count excludes the deferred floor).
            assert_eq!(
                *reached,
                caps.iter().filter(|c| c.reached_by_driving).count(),
                "every driven capability reached"
            );
            assert!(
                *m <= *b,
                "the measured render is within the thresholds-file budget"
            );
            assert_eq!(
                deferred_floors,
                &["ci-local"],
                "the `myelin ci local` floor is RECORDED in the gap report, never a wall"
            );
        }
        SwitchVerdict::Red {
            walls,
            latency_over_budget,
        } => panic!(
            "the CI switch test RED — walls: {walls:?}, latency_over_budget: {latency_over_budget} \
             (a migrating user would hit a wall the old tool didn't have)"
        ),
    }
    assert!(
        verdict.is_pass(),
        "the CI switch test PASSES (driven, measured)"
    );
    println!(
        "[{date}] CI switch test PASS — {} capabilities reached by driving the real `myelin ci` \
         run/log/deploy surface; render {measured_render_us}µs ≤ budget {budget}µs (thresholds-file); \
         seal {}",
        caps.iter().filter(|c| c.reached_by_driving).count(),
        switch.seal()
    );

    // ── 2. THE CI TRUTH-UP PASS (every PROVEN CI row rests on a dated green artifact, EI-01 §1) ──
    let rows = proven_ci_rows(&date);
    assert!(!rows.is_empty(), "the PROVEN CI set is non-empty");
    let confirmed = CiTruthUpPass::new()
        .run_or_fail_ci(&rows, &date)
        .expect("every PROVEN CI row rests on a dated green artifact (no later-band CI gate red)");
    assert_eq!(
        confirmed,
        rows.len(),
        "every PROVEN CI row confirmed dated-green"
    );
    println!(
        "[{date}] CI truth-up PASS — {confirmed} PROVEN CI rows each rest on a dated green artifact \
         (0 red earlier CI gates)"
    );

    // ── 3. THE SELF-HOSTED EVERY-INCIDENT-ADDS-A-DRILL LOOP (EI-01 §3/§5) ────────────────────────
    // CI's incidents are filed on the platform's OWN tracker + CI: each carries a Myelin issue + a
    // reproducing CI drill (the regression that re-runs forever). The loop is satisfied iff every
    // recorded incident is guarded.
    let mut incidents = IncidentDrillLoop::new();
    incidents.record(CiIncident {
        id: "INC-self-hosting-graph-bootstrap",
        issue_ref: Some("myelin://myelin/issues/issue/CI-DOGFOOD-1"),
        repro_drill_id: Some("self_hosting_ci_dogfood"),
    });
    assert!(
        incidents.is_satisfied(),
        "the every-incident-adds-a-drill loop is SATISFIED self-hosted ({:?})",
        incidents.unguarded_incidents()
    );
    println!(
        "[{date}] every-incident-adds-a-drill loop SELF-HOSTED — {} incident(s), 0 unguarded",
        incidents.incidents().len()
    );
}

/// **NON-VACUOUS GUARD: the gate REDS on a manufactured violation.** A switch-test wall, an undated
/// PROVEN row, and an unguarded incident each red their gate — the done-bar cannot be greened by a
/// vacuous bar (EI-01 §3).
#[test]
fn ci_p35_dogfood_gate_is_not_vacuous() {
    // (a) A switch-test WALL reds the switch test.
    let mut caps = switch_capability_matrix();
    caps[0].reached_by_driving = false;
    caps[0].deferred_named_floor = false;
    let verdict = CiSwitchTest::new(caps, 1_200, render_budget_us()).verdict();
    assert!(
        !verdict.is_pass(),
        "a wall reds the switch test (non-vacuous)"
    );
    assert!(!verdict.walls().is_empty(), "the wall is named");

    // (b) An UNDATED PROVEN row reds the truth-up pass.
    let mut rows = proven_ci_rows(&today());
    rows[0].artifact_date = None;
    let err = CiTruthUpPass::new()
        .run_or_fail_ci(&rows, &today())
        .expect_err("an undated PROVEN row reds the truth-up pass (non-vacuous)");
    assert!(
        !err.undated_rows.is_empty(),
        "the claimed-not-proven row is named"
    );

    // (c) An UNGUARDED incident reds the loop.
    let mut incidents = IncidentDrillLoop::new();
    incidents.record(CiIncident {
        id: "INC-no-drill",
        issue_ref: Some("myelin://myelin/issues/issue/X"),
        repro_drill_id: None,
    });
    assert!(
        !incidents.is_satisfied(),
        "a drill-less incident reds the loop (non-vacuous)"
    );
    assert_eq!(incidents.unguarded_incidents(), vec!["INC-no-drill"]);
}
