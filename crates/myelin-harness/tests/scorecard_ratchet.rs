use myelin_harness::scorecard::{required_rows, Band, RowResult, Scorecard};

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
    assert!(md.contains("GREEN - M1 may start"));
    assert!(md.contains("re-run-forever"));
    assert!(md.contains("SUB-D1 / SUB-D2 / BUS-D4"));
}

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

#[test]
fn any_claimed_not_proven_row_reds_the_gate() {
    for red_id in required_rows().into_iter().map(|r| r.id) {
        let mut card = Scorecard::new(Band::M0);
        for r in required_rows() {
            if r.id == red_id {
                card.record(RowResult::claimed_not_proven(
                    r.id,
                    "drill read RED - recorded honestly, never edited green",
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
            .contains("RED - M1 is BLOCKED"));
    }
}

#[test]
#[should_panic(expected = "no green without proof")]
fn green_without_proof_is_structurally_impossible() {
    let _ = RowResult::pass("SUB-D1", "", "2026-06-19");
}
