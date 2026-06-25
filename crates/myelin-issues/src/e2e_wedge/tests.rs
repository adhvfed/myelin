//! Unit tests for the ISS-P34 whole-system E2E-1 wedge (the Issues side — the PR context pane). Each
//! test drives the chained-mutation scenario END-TO-END (the whole flow, not a single handler) and
//! asserts the named green artifact + the F1 leak invariant at E2E scale. The deeper chained coverage
//! lives in `tests/e2e_wedge_iss_p34.rs`.

use super::*;

/// **E2E-1 green: the linked issue resolves per-viewer (insider sees the title), the mid-flight
/// ci.check.updated re-reads within the freshness budget (merge gate blocked), and the second (denied)
/// viewer's confidential issue tombstones with 0 title/count/backlink leak.** The whole flow is driven
/// end-to-end; a regression in any leg flips `is_green()` false.
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

/// **The confidential issue tombstones for the outsider — NEVER a projection (the load-bearing leak
/// invariant at E2E scale).** A regression that leaked the title would flip `leaks > 0`.
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

/// **The live check-update lands within the freshness budget and the merge gate shows blocked.** The
/// mid-flight `ci.check.updated` (test → failure) re-read off the fact is fresh, and a failing posture is
/// NOT an acceptable Done satisfaction (the SAME `is_acceptable` predicate the Done guard applies).
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

/// **The whole-wedge driver returns exactly the Issues-side E2E-1 leg, green.** The master M5 exit gate
/// cites E2E-1; this is the single Issues-side scenario.
#[test]
fn issues_e2e_wedge_runs_e2e_1_green() {
    let arts = run_issues_e2e_wedge();
    assert_eq!(arts.len(), 1, "Issues crosses exactly E2E-1");
    assert_eq!(arts[0].scenario, "E2E-1");
    assert!(arts[0].is_green(), "E2E-1: {}", arts[0].evidence);
}

/// **The freshness budget is a named threshold, not a stray literal (it is asserted, never weakened).**
/// A regression that widened the budget to mask a stale read would be caught by the catalogue value.
#[test]
fn e2e_1_freshness_budget_is_the_named_threshold() {
    assert_eq!(
        FRESHNESS_BUDGET_SECS, 5,
        "the pane-freshness SLA is 5s (the wedge's named threshold)"
    );
}
