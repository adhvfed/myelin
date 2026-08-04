use myelin_ci_controlplane::e2e_flagship::{run_e2e2_ci_flagship_slice, E2E_FLAGSHIP_SCENARIO};

#[test]
fn ci_p34_e2e2_ci_flagship_slice_green_end_to_end() {
    let art = run_e2e2_ci_flagship_slice();
    assert_eq!(art.scenario, "E2E-2");
    assert_eq!(E2E_FLAGSHIP_SCENARIO, "E2E-2");
    assert_eq!(
        art.leaks, 0,
        "E2E-2 (CI slice): 0 leak / 0 double-merge across the whole CI side - {}",
        art.evidence
    );
    assert!(
        art.is_green(),
        "E2E-2 (CI slice) green not earned (the dated artifact): {} [seal={}]",
        art.evidence,
        art.seal
    );
    assert!(art.seal.starts_with("blake3:"), "the artifact is sealed");
    assert!(art.evidence.contains("structured ci.run.failed"));
    assert!(art.evidence.contains("AG-D4-gated"));
    assert!(art.evidence.contains("fix-PR CI greens=true"));
    assert!(art.evidence.contains("EXACTLY ONCE"));
    assert!(art.evidence.contains("merge-count=1"));
    assert!(art.evidence.contains("reserve/settle balanced"));
}
