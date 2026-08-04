use super::*;

#[test]
fn switch_test_passes_with_no_walls_within_budget() {
    let caps = switch_capability_matrix();
    assert!(!caps.is_empty(), "the capability matrix is non-empty");
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
    assert_eq!(test.seal(), CiSwitchTest::new(caps, 1_200, 50_000).seal());
}

#[test]
fn switch_test_reds_loudly_on_a_wall() {
    let mut caps = switch_capability_matrix();
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

#[test]
fn deferred_named_floor_the_anchor_also_lacks_is_not_a_wall() {
    let mut caps = switch_capability_matrix();
    caps.push(SwitchCapability {
        id: "ci-local",
        anchor_feature: "act (third-party; no first-party local runner)",
        myelin_surface: "myelin ci local (deferred-by-design named floor - arch 04 §2)",
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

#[test]
fn switch_test_reds_when_render_latency_blows_the_budget() {
    let caps = switch_capability_matrix();
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
                "0 capability walls - only the latency wall"
            );
        }
        SwitchVerdict::Pass { .. } => unreachable!(),
    }
}

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

#[test]
fn truth_up_reds_loudly_on_a_claimed_not_proven_ci_row() {
    let mut rows = proven_ci_rows("2026-06-25");
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

#[test]
fn proven_ci_set_covers_the_drill_family_and_e2e_legs() {
    let rows = proven_ci_rows("2026-06-25");
    let ids: Vec<&str> = rows.iter().map(|r| r.id).collect();
    for must in [
        "CI-D1", "CI-D2", "CI-D3", "CI-D9", "CI-R3", "CI-D10", "E2E-1", "E2E-2", "E2E-3",
    ] {
        assert!(ids.contains(&must), "the PROVEN CI set covers {must}");
    }
    assert!(
        rows.iter()
            .all(|r| r.proof_command.starts_with("cargo test") && r.is_dated()),
        "every PROVEN row names a reproducible dated proof command"
    );
}

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

#[test]
fn incident_loop_reds_loudly_on_an_unguarded_incident() {
    let mut loop_ = IncidentDrillLoop::new();
    loop_.record(CiIncident {
        id: "INC-no-drill",
        issue_ref: Some("myelin://myelin/issues/issue/INC-3"),
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
