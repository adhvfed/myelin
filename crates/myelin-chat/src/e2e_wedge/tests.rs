//! Unit tests for Chat's whole-system E2E wedge participation (CHAT-P27 / P-501, M5).
//!
//! These assert chat's three E2E legs each reach the EARNED green (E2E-1 the unfurl/live-update pane,
//! E2E-2 the flagship terminal surface, E2E-4 the DSAR holder named in the 0-holders-missed certificate)
//! — the master M5 exit gate cites E2E-1/E2E-2/E2E-4 green. A red leg is a dated scorecard row, never a
//! weakened assertion (the gate must be able to go RED — proven by the negative assertions on each leg).

use super::*;

// ───────────────────────── E2E-1 — the unfurl/live-update pane (CHAT-D7) ─────────────────────────

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
    // The freshness budget is a named threshold, not a stray literal — a re-read at age 0 satisfies it.
    assert_eq!(e2e_pane::FRESHNESS_BUDGET_SECS, 5);
}

// ───────────────────────── E2E-2 — the flagship terminal surface ─────────────────────────

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

// ───────────────────────── E2E-4 — the DSAR holder (0 holders missed) ─────────────────────────

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

// ───────────────────────── the whole wedge ─────────────────────────

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
    // The artifact is green ONLY iff the scenario earned it AND 0 leak — a leak forces red even if the
    // scenario predicate held (the gate cannot be greened by a weakened assertion).
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
