use super::*;

#[test]
fn e2e_2_issues_flagship_is_green_end_to_end() {
    let art = run_e2e_2_issues_flagship();
    assert_eq!(art.scenario, "E2E-2");
    assert_eq!(
        art.leaks, 0,
        "0 leak/double-apply across Issues' flagship slice: {}",
        art.evidence
    );
    assert!(
        art.is_green(),
        "E2E-2 (Issues slice) green not earned: {}",
        art.evidence
    );
}

#[test]
fn e2e_2_agent_close_is_withheld_zero_pre_approval_mutation() {
    let art = run_e2e_2_issues_flagship();
    assert!(
        art.evidence.contains("withheld=true"),
        "the agent's permitted governed close must be WITHHELD for approval: {}",
        art.evidence
    );
    assert!(
        art.evidence.contains("pre_approval_mutations=0"),
        "0 mutation before approval: {}",
        art.evidence
    );
}

#[test]
fn e2e_2_zero_effect_outside_the_intersection() {
    let art = run_e2e_2_issues_flagship();
    assert!(
        art.evidence.contains("undeclared edge blocked=true"),
        "an undeclared edge must be blocked (0 effect outside the FSM ∩): {}",
        art.evidence
    );
    assert!(
        art.evidence.contains("ci-red blocks agent=true"),
        "a CI-red governed close must block for the agent (the guard never leaks green): {}",
        art.evidence
    );
}

#[test]
fn e2e_2_governed_transition_applies_exactly_once_across_a_kill() {
    let art = run_e2e_2_issues_flagship();
    assert!(
        art.evidence.contains("across_kill=true"),
        "the governed transition must apply exactly once across the kill: {}",
        art.evidence
    );
    assert!(
        art.evidence.contains("apply_count=1"),
        "exactly ONE governed-transition apply (0 double-apply, 0 vanished work): {}",
        art.evidence
    );
    assert!(
        art.evidence.contains("duplicate_absorbed=true"),
        "the at-least-once duplicate approval must be absorbed by the wf_signal PK: {}",
        art.evidence
    );
}

#[test]
fn e2e_2_reserve_settle_balanced_and_no_balance_no_start() {
    let art = run_e2e_2_issues_flagship();
    assert!(
        art.evidence.contains("no-balance→no-start=true"),
        "an exhausted wallet must refuse the dispatch (no balance → no start): {}",
        art.evidence
    );
    assert!(
        art.evidence
            .contains("reserved 20 == billed 14 + refunded 6"),
        "reserve/settle must balance (reserved == billed + refunded): {}",
        art.evidence
    );
}

#[test]
fn the_hitl_apply_ledger_applies_exactly_once() {
    let key = crate::per_effect_idem_key(CLOSE_CARD_ID, 0, 1);
    let mut ledger = HitlApplyLedger::default();
    assert!(ledger.deliver_approval(&key), "the first delivery buffers");
    assert!(
        !ledger.deliver_approval(&key),
        "the duplicate delivery is absorbed (ON CONFLICT DO NOTHING)"
    );
    assert!(ledger.apply_once(&key), "the first apply mutates");
    assert!(!ledger.apply_once(&key), "a re-drive does NOT re-apply");
    assert!(!ledger.apply_once(&key), "still a no-op (exactly once)");
    assert_eq!(ledger.apply_count, 1, "the transition applied exactly once");
}

#[test]
fn an_unapproved_key_never_applies() {
    let key = crate::per_effect_idem_key(CLOSE_CARD_ID, 0, 1);
    let mut ledger = HitlApplyLedger::default();
    assert!(
        !ledger.apply_once(&key),
        "an un-approved governed transition does NOT apply (the run stays parked)"
    );
    assert_eq!(ledger.apply_count, 0, "0 premature apply");
}
