//! The SUB-M0 scorecard ratchet meta-test (P-S24 → P-039, the prompt's required meta-test).
//!
//! "Removing any drill from the scorecard or flipping any threshold green-without-proof fails
//! the gate (the ratchet cannot be gamed)." This exercises the public scorecard API the
//! `sub-m0-scorecard` binary uses, proving both halves of the ratchet at the integration
//! boundary (EI-01 §3: a property does not exist until a test forces the failure).

use myelin_harness::scorecard::{required_rows, Band, RowResult, Scorecard};

/// A scorecard with every required row proven (the all-green baseline the binary writes on a
/// clean M0 tree).
fn fully_green() -> Scorecard {
    let mut card = Scorecard::new(Band::M0);
    for r in required_rows() {
        card.record(RowResult::pass(
            r.id,
            format!("[2026-06-19] PASS `cargo {}`", r.proof_command.join(" ")),
            "2026-06-19",
        ));
    }
    card
}

#[test]
fn baseline_all_proven_is_green() {
    let card = fully_green();
    assert!(card.is_green(), "every required SUB-M0 row proven ⇒ GREEN");
    assert!(card.missing_required().is_empty());
    assert!(card.not_proven().is_empty());
    let md = card.render_markdown("2026-06-19");
    assert!(md.contains("GREEN — M1 may start"));
    // The permanent gates are marked re-run-forever in the rendered artifact.
    assert!(md.contains("re-run-forever"));
    assert!(md.contains("SUB-D1 / SUB-D2 / BUS-D4"));
}

/// RATCHET HALF 1: drop ANY single required row → the gate goes RED. Proven for every row id
/// (you cannot game the gate by omitting any one of them).
#[test]
fn dropping_any_single_row_reds_the_gate() {
    for dropped in required_rows() {
        let mut card = Scorecard::new(Band::M0);
        for r in required_rows().into_iter().filter(|r| r.id != dropped.id) {
            card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
        }
        assert_eq!(
            card.missing_required(),
            vec![dropped.id],
            "dropping {} must surface as the one missing row",
            dropped.id
        );
        assert!(
            !card.is_green(),
            "dropping required row {} must RED the gate (the ratchet)",
            dropped.id
        );
    }
}

/// RATCHET HALF 2: any claimed-not-proven row → RED. Proven for every row id (no single row can
/// be softened into a green it did not earn).
#[test]
fn any_claimed_not_proven_row_reds_the_gate() {
    for red_id in required_rows().into_iter().map(|r| r.id) {
        let mut card = Scorecard::new(Band::M0);
        for r in required_rows() {
            if r.id == red_id {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "drill read RED — recorded honestly, never edited green",
                    "2026-06-19",
                ));
            } else {
                card.record(RowResult::pass(r.id, "[2026-06-19] PASS", "2026-06-19"));
            }
        }
        assert!(
            !card.is_green(),
            "a claimed-not-proven {red_id} row must block M1 (the gate invariant)"
        );
        assert!(card
            .render_markdown("2026-06-19")
            .contains("RED — M1 is BLOCKED"));
    }
}

/// RATCHET HALF 2 (structural): a Pass cannot be recorded without its dated green-artifact proof
/// line — `RowResult::pass` panics on empty proof. This forecloses "flip a threshold green
/// without proof" mechanically (there is no Pass-from-nothing constructor).
#[test]
#[should_panic(expected = "no green without proof")]
fn green_without_proof_is_structurally_impossible() {
    let _ = RowResult::pass("SUB-D1", "", "2026-06-19");
}
