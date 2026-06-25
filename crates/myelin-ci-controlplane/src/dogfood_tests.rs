//! Unit tests for the CI dogfood done-bar (CI-P35 / P-509, M6): the switch test, the truth-up pass,
//! and the self-hosted every-incident-adds-a-drill loop. The drills assert against the typed
//! verdicts + the NAMED rows (EI-01 §3 — never a hidden literal), and prove each gate REDS LOUDLY on a
//! manufactured violation (a non-vacuous guard: a wall / an undated row / an unguarded incident).

use super::*;

// ── 1. The switch test (driven, measured — the Git OQ-12 / CI switch test). ─────────────────────

/// **The switch test PASSES when the real surface reaches every anchor capability within budget.** The
/// driven capability matrix has every row reached (the real `myelin ci` run/log/deploy surface), and
/// the measured render latency sits within the budget → a GitHub-Actions user could move without
/// hitting a wall the old tool didn't have.
#[test]
fn switch_test_passes_with_no_walls_within_budget() {
    let caps = switch_capability_matrix();
    assert!(!caps.is_empty(), "the capability matrix is non-empty");
    // A measured render comfortably within a 50 ms budget (the real run/log view render is fast).
    let test = CiSwitchTest::new(caps.clone(), 1_200, 50_000);
    let verdict = test.verdict();
    assert!(
        verdict.is_pass(),
        "0 walls + within budget → the migrating user hits no wall"
    );
    match verdict {
        SwitchVerdict::Pass {
            reached,
            measured_render_us,
            budget_render_us,
            ..
        } => {
            assert_eq!(reached, caps.len(), "every capability reached by driving");
            assert!(measured_render_us <= budget_render_us);
        }
        SwitchVerdict::Red { .. } => unreachable!("the matrix is all-reached within budget"),
    }
    // The seal is reproducible (a content-addressed artifact the done-bar cites by hash).
    assert_eq!(test.seal(), CiSwitchTest::new(caps, 1_200, 50_000).seal());
}

/// **A WALL reds the switch test LOUDLY (a capability the anchor has that Myelin does not reach).** A
/// row whose `reached_by_driving` is false AND is not a deferred named floor is a wall — the migrating
/// user WOULD hit a wall the old tool didn't have.
#[test]
fn switch_test_reds_loudly_on_a_wall() {
    let mut caps = switch_capability_matrix();
    // Manufacture a wall: the live-log-tail capability is NOT reached and is NOT a deferred floor.
    let tail = caps
        .iter_mut()
        .find(|c| c.id == "live-log-tail")
        .expect("live-log-tail is in the matrix");
    tail.reached_by_driving = false;
    tail.deferred_named_floor = false;
    assert!(
        tail.is_wall(),
        "an unreached, non-deferred capability IS a wall"
    );

    let verdict = CiSwitchTest::new(caps, 1_200, 50_000).verdict();
    assert!(!verdict.is_pass(), "a wall reds the switch test");
    assert_eq!(
        verdict.walls(),
        &["live-log-tail"],
        "the wall is NAMED (loud, never swallowed)"
    );
}

/// **A deferred-by-design named floor the anchor ALSO lacks is NOT a wall.** `myelin ci local` (laptop
/// execution) is a deliberately-deferred named floor (arch 04 §2) — GitHub Actions has no local runner
/// by default either, so an unreached floor here is NOT a wall the old tool didn't have.
#[test]
fn deferred_named_floor_the_anchor_also_lacks_is_not_a_wall() {
    let mut caps = switch_capability_matrix();
    // Add the `myelin ci local` deferred floor: unreached, but a named floor the anchor also lacks.
    caps.push(SwitchCapability {
        id: "ci-local",
        anchor_feature: "act (third-party; no first-party local runner)",
        myelin_surface: "myelin ci local (deferred-by-design named floor — arch 04 §2)",
        reached_by_driving: false,
        deferred_named_floor: true,
    });
    let local = caps.last().unwrap();
    assert!(
        !local.is_wall(),
        "a deferred named floor the anchor also lacks is NOT a wall"
    );
    let verdict = CiSwitchTest::new(caps, 1_200, 50_000).verdict();
    assert!(
        verdict.is_pass(),
        "the deferred floor does not red the switch"
    );
    match verdict {
        SwitchVerdict::Pass {
            deferred_floors, ..
        } => assert_eq!(
            deferred_floors,
            vec!["ci-local"],
            "the deferred floor is RECORDED (named, never silently skipped)"
        ),
        SwitchVerdict::Red { .. } => unreachable!(),
    }
}

/// **A render slower than the budget is a UX WALL (the latency gate, never weakened).** A measured
/// render over the budget reds the switch test — the migrating user hits a slower-than-the-anchor
/// run/log view (a UX wall the old tool didn't have).
#[test]
fn switch_test_reds_when_render_latency_blows_the_budget() {
    let caps = switch_capability_matrix();
    // A render OVER the 50 ms budget (a 51 ms render — a UX wall).
    let verdict = CiSwitchTest::new(caps, 51_000, 50_000).verdict();
    assert!(
        !verdict.is_pass(),
        "an over-budget render reds the switch test"
    );
    match verdict {
        SwitchVerdict::Red {
            latency_over_budget,
            walls,
        } => {
            assert!(latency_over_budget, "the latency breach is named");
            assert!(
                walls.is_empty(),
                "0 capability walls — only the latency wall"
            );
        }
        SwitchVerdict::Pass { .. } => unreachable!(),
    }
}

// ── 2. The CI truth-up pass (every PROVEN CI row rests on a dated green artifact). ───────────────

/// **The truth-up pass GREENs when every PROVEN CI row is dated.** The done-bar's honesty gate: no
/// earlier-band CI gate is red (every PROVEN row rests on a dated green artifact).
#[test]
fn truth_up_greens_when_every_ci_row_is_dated() {
    let rows = proven_ci_rows("2026-06-25");
    assert!(!rows.is_empty(), "the PROVEN CI set is non-empty");
    let verdict = CiTruthUpPass::new().run(&rows, "2026-06-25");
    assert!(
        verdict.is_green(),
        "every PROVEN row rests on a dated artifact"
    );
    match verdict {
        CiTruthUpVerdict::Green { rows_confirmed, .. } => assert_eq!(rows_confirmed, rows.len()),
        CiTruthUpVerdict::Red { .. } => unreachable!(),
    }
    let confirmed = CiTruthUpPass::new()
        .run_or_fail_ci(&rows, "2026-06-25")
        .expect("green → Ok");
    assert_eq!(confirmed, rows.len());
}

/// **The truth-up pass REDs LOUDLY + names a claimed-not-proven row (EI-01 §1).** A PROVEN CI row
/// without a dated green artifact is a claim that outlived its verification → a loud red, never a
/// silent pass.
#[test]
fn truth_up_reds_loudly_on_a_claimed_not_proven_ci_row() {
    let mut rows = proven_ci_rows("2026-06-25");
    // Strip the date off CI-D9 — a PROVEN claim with no artifact (the docs drifted from the code).
    let ci_d9 = rows
        .iter_mut()
        .find(|r| r.id == "CI-D9")
        .expect("CI-D9 is in the PROVEN set");
    ci_d9.artifact_date = None;
    assert!(!ci_d9.is_dated());

    let verdict = CiTruthUpPass::new().run(&rows, "2026-06-25");
    assert!(!verdict.is_green(), "an undated PROVEN row REDs the pass");
    assert_eq!(
        verdict.undated_rows(),
        &["CI-D9"],
        "the claimed-not-proven row is NAMED (loud, never swallowed)"
    );
    let err = CiTruthUpPass::new()
        .run_or_fail_ci(&rows, "2026-06-25")
        .expect_err("a red → Err naming the row");
    assert_eq!(err.undated_rows, vec!["CI-D9".to_string()]);
    assert!(
        format!("{err}").contains("CI-D9"),
        "the error names the row"
    );
}

/// **The PROVEN CI set covers the CI-D* family + the M5 world-scale + the E2E wedge legs.** The done-bar
/// asserts the truth-up pass enumerates the right rows (EI-01 §3 — the set is the proof, not a number).
#[test]
fn proven_ci_set_covers_the_drill_family_and_e2e_legs() {
    let rows = proven_ci_rows("2026-06-25");
    let ids: Vec<&str> = rows.iter().map(|r| r.id).collect();
    for must in [
        "CI-D1", "CI-D2", "CI-D3", "CI-D9", "CI-R3", "CI-D10", "E2E-1", "E2E-2", "E2E-3",
    ] {
        assert!(ids.contains(&must), "the PROVEN CI set covers {must}");
    }
    // Every row names a reproducible proof command (a `cargo test` target) + is dated.
    assert!(
        rows.iter()
            .all(|r| r.proof_command.starts_with("cargo test") && r.is_dated()),
        "every PROVEN row names a reproducible dated proof command"
    );
}

// ── 3. The self-hosted every-incident-adds-a-drill loop. ────────────────────────────────────────

/// **The loop is SATISFIED when every incident files a Myelin issue + a reproducing CI drill.** The
/// every-incident-adds-a-drill property holds self-hosted (CI's incidents are filed on Myelin Issues +
/// Myelin CI).
#[test]
fn incident_loop_satisfied_when_every_incident_adds_a_drill() {
    let mut loop_ = IncidentDrillLoop::new();
    loop_.record(CiIncident {
        id: "INC-ci-flaky-runner",
        issue_ref: Some("myelin://myelin/issues/issue/INC-1"),
        repro_drill_id: Some("drill_ci_flaky_runner_regression"),
    });
    loop_.record(CiIncident {
        id: "INC-deploy-gate-double-apply",
        issue_ref: Some("myelin://myelin/issues/issue/INC-2"),
        repro_drill_id: Some("drill_ci_deploy_gate_idempotent"),
    });
    assert!(
        loop_.is_satisfied(),
        "every incident filed both an issue + a drill"
    );
    assert!(loop_.unguarded_incidents().is_empty());
    assert_eq!(loop_.incidents().len(), 2);
}

/// **An incident with NO reproducing drill is a LOUD gap (EI-01 §3/§5).** An incident that files an
/// issue but adds no reproducing CI drill is unguarded — the every-incident-adds-a-drill property is
/// violated and named, never silently skipped.
#[test]
fn incident_loop_reds_loudly_on_an_unguarded_incident() {
    let mut loop_ = IncidentDrillLoop::new();
    loop_.record(CiIncident {
        id: "INC-no-drill",
        issue_ref: Some("myelin://myelin/issues/issue/INC-3"),
        // No reproducing CI drill — the regression is not guarded forever (the gap).
        repro_drill_id: None,
    });
    assert!(
        !loop_.is_satisfied(),
        "a drill-less incident violates the loop"
    );
    assert_eq!(
        loop_.unguarded_incidents(),
        vec!["INC-no-drill"],
        "the unguarded incident is NAMED (loud, never swallowed)"
    );
}
