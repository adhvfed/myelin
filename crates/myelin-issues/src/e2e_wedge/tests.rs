use super::*;

#[test]
fn e2e_1_pr_pane_is_green_zero_leak() {
    let art = run_e2e_1_pr_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert!(art.is_green(), "E2E-1 must be green: {}", art.evidence);
    assert_eq!(
        art.leaks, 0,
        "0 title/count/backlink leak: {}",
        art.evidence
    );
}

#[test]
fn e2e_1_outsider_never_sees_confidential_title() {
    let art = run_e2e_1_pr_pane();
    assert!(
        art.evidence.contains("tombstone(denied)=true"),
        "the outsider's confidential issue must tombstone (denied): {}",
        art.evidence
    );
    assert!(
        !art.evidence.contains("SECRET") && !art.evidence.contains("acquisition"),
        "the secret title must NEVER appear in the artifact body: {}",
        art.evidence
    );
    assert_eq!(art.leaks, 0);
}

#[test]
fn e2e_1_check_update_is_fresh_and_blocks_the_merge_gate() {
    let art = run_e2e_1_pr_pane();
    assert!(
        art.evidence.contains("merge_gate_blocked=true"),
        "a failing CheckStatus must block the merge gate: {}",
        art.evidence
    );
    assert!(
        art.evidence
            .contains(&format!("≤ {FRESHNESS_BUDGET_SECS}s)=true")),
        "the re-read must land within the freshness budget: {}",
        art.evidence
    );
}

#[test]
fn issues_e2e_wedge_runs_e2e_1_green() {
    let arts = run_issues_e2e_wedge();
    assert_eq!(arts.len(), 1, "Issues crosses exactly E2E-1");
    assert_eq!(arts[0].scenario, "E2E-1");
    assert!(arts[0].is_green(), "E2E-1: {}", arts[0].evidence);
}

#[test]
fn e2e_1_freshness_budget_is_the_named_threshold() {
    assert_eq!(
        FRESHNESS_BUDGET_SECS, 5,
        "the pane-freshness SLA is 5s (the wedge's named threshold)"
    );
}
