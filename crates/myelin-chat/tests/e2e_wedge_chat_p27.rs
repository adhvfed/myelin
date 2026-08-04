use myelin_chat::{run_chat_e2e_wedge, ChatE2eArtifact};

#[test]
fn chat_e2e_wedge_three_legs_green() {
    let arts: Vec<ChatE2eArtifact> = run_chat_e2e_wedge();
    assert_eq!(arts.len(), 3, "E2E-1 + E2E-2 + E2E-4 - chat's three legs");

    let scenarios: Vec<&str> = arts.iter().map(|a| a.scenario).collect();
    assert_eq!(
        scenarios,
        vec!["E2E-1", "E2E-2", "E2E-4"],
        "chat crosses E2E-1/E2E-2/E2E-4"
    );

    for art in &arts {
        assert!(
            art.is_green(),
            "{} is green for chat's surface: {}",
            art.scenario,
            art.evidence
        );
        assert_eq!(
            art.leaks, 0,
            "{} 0 title/count/backlink leak (incl. 0 recoverable PII for E2E-4)",
            art.scenario
        );
    }
}

#[test]
fn chat_e2e_2_flagship_terminates_green_in_chat() {
    let arts = run_chat_e2e_wedge();
    let flagship = arts
        .iter()
        .find(|a| a.scenario == "E2E-2")
        .expect("the flagship leg exists");
    assert!(
        flagship.is_green(),
        "the flagship terminates green in chat (exactly-once HITL + merge, 0 leak): {}",
        flagship.evidence
    );
    assert!(flagship.evidence.contains("merge_applied_once=true"));
    assert!(flagship
        .evidence
        .contains("dispatched_through_one_wallet=true"));
    assert!(flagship.evidence.contains("double_click_deduped=true"));
}

#[test]
fn chat_e2e_4_dsar_holder_zero_holders_missed_zero_recoverable() {
    let arts = run_chat_e2e_wedge();
    let dsar = arts
        .iter()
        .find(|a| a.scenario == "E2E-4")
        .expect("the DSAR holder leg exists");
    assert!(
        dsar.is_green(),
        "chat's H5 holder is green: {}",
        dsar.evidence
    );
    assert_eq!(dsar.leaks, 0, "0 recoverable PII (hot + cold + backups)");
    assert!(dsar.evidence.contains("0 holders missed"));
}
