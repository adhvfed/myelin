use myelin_knowledge::e2e_wedge::{
    run_e2e1_pr_context_pane, run_e2e3_spec_to_ship_lineage, run_knowledge_e2e_legs, E2E_SCENARIOS,
};

#[test]
fn kn_p33_e2e1_pr_context_pane_zero_leak() {
    let art = run_e2e1_pr_context_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert_eq!(
        art.leaks, 0,
        "E2E-1: 0 title leak across every projection - {}",
        art.evidence
    );
    assert!(
        art.is_green(),
        "E2E-1 green not earned (the dated artifact): {} [seal={}]",
        art.evidence,
        art.seal
    );
    assert!(art.seal.starts_with("blake3:"), "the artifact is sealed");
    assert!(art.evidence.contains("denied viewer"));
    assert!(art.evidence.contains("tombstone"));
}

#[test]
fn kn_p33_e2e3_spec_to_ship_lineage_cold_equals_live_and_tamper_detected() {
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
    assert!(art.evidence.contains("cold-reindex==live=true"));
    assert!(art.evidence.contains("tamper-detected=true"));
    assert!(art.evidence.contains("lineage traceable=true"));
}

#[test]
fn kn_p33_both_legs_green() {
    let arts = run_knowledge_e2e_legs();
    assert_eq!(arts.len(), 2, "Knowledge crosses two E2E scenarios");
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
        "the two legs seal to distinct citable addresses"
    );
}
