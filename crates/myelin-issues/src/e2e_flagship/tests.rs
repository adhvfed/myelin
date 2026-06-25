//! Unit tests for the ISS-P35 whole-system E2E-2 flagship (the Issues side — the agent-native
//! flagship). Each test drives the chained-mutation scenario END-TO-END (the whole Issues slice across
//! the kill, not a single handler) and asserts the named green artifact + the four load-bearing
//! E2E-2 invariants. The deeper chained coverage (the DURABLE `wf_signal` exactly-once + the 8.2/5.9
//! CDC re-asserts) lives in `tests/e2e_flagship_iss_p35.rs`.

use super::*;

/// **E2E-2 green (Issues slice): the governed close is HITL-gated (0 mutation pre-approval), 0 effect
/// outside the `∩`, the governed transition applies exactly once across a kill, and reserve/settle
/// balances.** The whole Issues slice is driven end-to-end; a regression in any leg flips `is_green()`.
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

/// **0 mutation before approval — the agent's governed close is WITHHELD (AG-8).** Even with a trusted
/// green check (the guard PERMITS), an AGENT does not auto-apply; the close is withheld for HITL
/// approval, and `pre_approval_mutations() == 0`.
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

/// **0 effect outside the `∩` — an UNDECLARED edge is BLOCKED (the FSM is Issues' slice of the `∩`).**
/// The agent cannot invent a transition the workflow FSM does not declare, and a CI-red linked PR
/// blocks the close for the agent too (the poisoned-Done defence under the agent path).
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

/// **Exactly-once approval + governed transition across a kill.** The approval is buffered (first
/// delivery), the kill lands before the apply, the at-least-once duplicate is absorbed, and the
/// governed transition applies EXACTLY ONCE on resume (`apply_count=1`).
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

/// **reserve/settle balanced (11.7) — reserved == billed + refunded, no balance → no start.** The
/// spend-bearing triage run conserves the wallet, and an exhausted wallet refuses the dispatch (the
/// run never starts).
#[test]
fn e2e_2_reserve_settle_balanced_and_no_balance_no_start() {
    let art = run_e2e_2_issues_flagship();
    assert!(
        art.evidence.contains("no-balance→no-start=true"),
        "an exhausted wallet must refuse the dispatch (no balance → no start): {}",
        art.evidence
    );
    // reserved 20 == billed 14 + refunded 6 (the metered units total 14 < the 20 reservation).
    assert!(
        art.evidence
            .contains("reserved 20 == billed 14 + refunded 6"),
        "reserve/settle must balance (reserved == billed + refunded): {}",
        art.evidence
    );
}

/// **The HITL apply ledger applies EXACTLY ONCE under a double-delivery + a re-drive (the dedup crux,
/// isolated).** A focused re-assert of the `wf_signal`-modelled ON-CONFLICT-DO-NOTHING dedup: a
/// duplicate delivery is absorbed; the first apply mutates; every subsequent apply is a no-op.
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

/// **An apply over an UN-approved (unbuffered) key is a no-op — the run stays parked (0 premature
/// apply).** Before the human approves, the gated apply applies NOTHING — the governed transition is
/// withheld until the durable approval is buffered.
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
