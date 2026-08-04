use myelin_git::surge::{
    run_e2e_1_pr_pane, run_e2e_2_fix_pr, run_e2e_3_spec_to_ship, run_git_e2e_wedge, E2eArtifact,
    GIT_E2E_SCENARIOS,
};

#[test]
fn git_p34_whole_git_e2e_wedge_is_green() {
    let arts: Vec<E2eArtifact> = run_git_e2e_wedge();
    assert_eq!(
        arts.len(),
        3,
        "the three rows git crosses: E2E-1 / E2E-2 / E2E-3"
    );
    let scenarios: Vec<&str> = arts.iter().map(|a| a.scenario).collect();
    assert_eq!(scenarios, GIT_E2E_SCENARIOS);
    for a in &arts {
        assert!(
            a.is_green(),
            "{} must be green (the master M5 exit gate cites it): {}",
            a.scenario,
            a.evidence
        );
        println!(
            "[P-483 GIT-P34 {} GREEN 2026-06-25] {}",
            a.scenario, a.evidence
        );
    }
}

#[test]
fn git_p34_e2e_1_pr_pane_zero_leak() {
    let a = run_e2e_1_pr_pane();
    assert!(a.is_green(), "E2E-1: {}", a.evidence);
    assert_eq!(a.leaks, 0, "zero leak to the unauthorized viewer");
}

#[test]
fn git_p34_e2e_2_flagship_exactly_once_hitl_and_merge() {
    let a = run_e2e_2_fix_pr();
    assert!(a.is_green(), "E2E-2 flagship: {}", a.evidence);
    assert_eq!(
        a.merge_count, 1,
        "exactly-once merge across the kill the durable workflow rode (FLOW-D1)"
    );
    assert_eq!(a.leaks, 0);
}

#[test]
fn git_p34_e2e_3_spec_to_ship_cold_equals_live() {
    let a = run_e2e_3_spec_to_ship();
    assert!(a.is_green(), "E2E-3: {}", a.evidence);
}
