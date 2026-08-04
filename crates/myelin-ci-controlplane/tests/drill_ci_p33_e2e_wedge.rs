use myelin_ci_controlplane::e2e_wedge::{
    run_ci_e2e_slices, run_e2e1_pr_context_pane, run_e2e3_spec_to_ship_lineage, E2E_SCENARIOS,
};

#[test]
fn ci_p33_e2e1_pr_context_pane_zero_leak() {
    let art = run_e2e1_pr_context_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert_eq!(
        art.leaks, 0,
        "E2E-1: 0 row leak across every projection - {}",
        art.evidence
    );
    assert!(
        art.is_green(),
        "E2E-1 green not earned (the dated artifact): {} [seal={}]",
        art.evidence,
        art.seal
    );
    assert!(art.seal.starts_with("blake3:"), "the artifact is sealed");
    assert!(art.evidence.contains("ci.check.updated"));
    assert!(art.evidence.contains("#step-"));
    assert!(art.evidence.contains("tombstone"));
    assert!(art.evidence.contains("merge blocked"));
}

#[test]
fn ci_p33_e2e3_spec_to_ship_lineage_cold_equals_live_and_tamper_detected() {
    let art = run_e2e3_spec_to_ship_lineage();
    assert_eq!(art.scenario, "E2E-3");
    assert_eq!(
        art.leaks, 0,
        "E2E-3: 0 cold/live divergence + 0 undetected tamper - {}",
        art.evidence
    );
    assert!(
        art.is_green(),
        "E2E-3 green not earned (the dated artifact): {} [seal={}]",
        art.evidence,
        art.seal
    );
    assert!(art.seal.starts_with("blake3:"), "the artifact is sealed");
    assert!(art.evidence.contains("lineage traceable=true"));
    assert!(art.evidence.contains("approve-ships-exactly-once=true"));
    assert!(art.evidence.contains("cold-reindex==live=true"));
    assert!(art.evidence.contains("tamper-detected=true"));
}

#[test]
fn ci_p33_both_slices_green() {
    let arts = run_ci_e2e_slices();
    assert_eq!(arts.len(), 2, "CI's slice crosses two E2E scenarios");
    assert_eq!(E2E_SCENARIOS, ["E2E-1", "E2E-3"]);
    for art in &arts {
        assert!(
            art.is_green(),
            "{} not green: {} [seal={}]",
            art.scenario,
            art.evidence,
            art.seal
        );
    }
    assert_ne!(
        arts[0].seal, arts[1].seal,
        "the two slices seal to distinct citable addresses"
    );
}
