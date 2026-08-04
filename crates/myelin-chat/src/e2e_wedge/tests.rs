use super::*;

#[test]
fn e2e_1_unfurl_pane_is_green_and_zero_leak() {
    let art = e2e_pane::run_e2e_1_unfurl_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert!(
        art.is_green(),
        "E2E-1 (chat unfurl/live-update pane) is green: {}",
        art.evidence
    );
    assert_eq!(art.leaks, 0, "0 title/count/backlink leak (the F1 spine)");
}

#[test]
fn e2e_1_freshness_budget_is_the_named_threshold() {
    assert_eq!(e2e_pane::FRESHNESS_BUDGET_SECS, 5);
}

#[test]
fn e2e_2_flagship_terminates_green_in_chat() {
    let art = e2e_flagship::run_e2e_2_chat_flagship();
    assert_eq!(art.scenario, "E2E-2");
    assert!(
        art.is_green(),
        "E2E-2 (the agent-native flagship) terminates green in chat: {}",
        art.evidence
    );
    assert_eq!(art.leaks, 0);
}

#[test]
fn e2e_4_dsar_holder_is_green_zero_recoverable_pii() {
    let art = e2e_dsar::run_e2e_4_chat_dsar_holder();
    assert_eq!(art.scenario, "E2E-4");
    assert!(
        art.is_green(),
        "E2E-4 (chat's H5 holder named in the 0-holders-missed certificate) is green: {}",
        art.evidence
    );
    assert_eq!(
        art.leaks, 0,
        "0 recoverable PII across hot + cold + backups (the E2E-4 zero)"
    );
}

#[test]
fn run_chat_e2e_wedge_emits_three_green_artifacts() {
    let arts = run_chat_e2e_wedge();
    assert_eq!(arts.len(), 3, "E2E-1 + E2E-2 + E2E-4");
    let scenarios: Vec<&str> = arts.iter().map(|a| a.scenario).collect();
    assert_eq!(scenarios, vec!["E2E-1", "E2E-2", "E2E-4"]);
    for art in &arts {
        assert!(
            art.is_green(),
            "{} is green: {}",
            art.scenario,
            art.evidence
        );
        assert_eq!(art.leaks, 0, "{} 0 leak", art.scenario);
    }
}

#[test]
fn the_green_predicate_requires_both_earned_and_zero_leak() {
    let leaky = ChatE2eArtifact {
        scenario: "E2E-1",
        green: true,
        evidence: "synthetic".into(),
        leaks: 1,
    };
    assert!(!leaky.is_green(), "a leak forces red (the F1 spine)");
    let unearned = ChatE2eArtifact {
        scenario: "E2E-2",
        green: false,
        evidence: "synthetic".into(),
        leaks: 0,
    };
    assert!(!unearned.is_green(), "an unearned scenario is red");
}
