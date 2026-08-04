use myelin_search::{
    run_e2e_1_pr_pane, run_e2e_3_spec_to_ship, run_e2e_4_dsar_fanout, run_search_e2e_wedge,
    E2eArtifact, E2E_SCENARIOS,
};

#[test]
fn srch_p32_whole_search_e2e_wedge_is_green() {
    let arts: Vec<E2eArtifact> = run_search_e2e_wedge();
    assert_eq!(
        arts.len(),
        3,
        "the three rows Search crosses: E2E-1 / E2E-3 / E2E-4"
    );
    let scenarios: Vec<&str> = arts.iter().map(|a| a.scenario).collect();
    assert_eq!(scenarios, E2E_SCENARIOS);
    for a in &arts {
        assert!(
            a.is_green(),
            "{} must be green (the master M5 exit gate cites it): {}",
            a.scenario,
            a.evidence
        );
        println!(
            "[P-465 SRCH-P32 {} GREEN 2026-06-25] {}",
            a.scenario, a.evidence
        );
    }
}

#[test]
fn srch_p32_e2e_1_pr_pane_zero_leak() {
    let a = run_e2e_1_pr_pane();
    assert!(a.is_green(), "E2E-1: {}", a.evidence);
    assert_eq!(
        a.leaks, 0,
        "0 doc/count/IDF/RAG/title leak (the §4.2 pre-filter)"
    );
    assert!(a.evidence.contains("tombstone"));
    assert!(a.evidence.contains("title_absent=true"));
}

#[test]
fn srch_p32_e2e_3_reindex_byte_match() {
    let a = run_e2e_3_spec_to_ship();
    assert!(a.is_green(), "E2E-3: {}", a.evidence);
    assert!(
        a.evidence.contains("byte_match=true"),
        "cold-reindex == live: {}",
        a.evidence
    );
    assert!(a.evidence.contains("restore-verify green=true"));
}

#[test]
fn srch_p32_e2e_4_dsar_zero_recoverable_including_backups() {
    let a = run_e2e_4_dsar_fanout();
    assert!(a.is_green(), "E2E-4: {}", a.evidence);
    assert_eq!(
        a.leaks, 0,
        "0 recoverable PII incl. vectors incl. backups (GA-D1 spine)"
    );
    assert!(
        a.evidence.contains("recoverable 3→0"),
        "0 recoverable after the shred: {}",
        a.evidence
    );
    assert!(
        a.evidence.contains("is_h7=true"),
        "the receipt includes Search H7: {}",
        a.evidence
    );
}
