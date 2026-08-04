use myelin_harness::scorecard::{id_m1_required_rows, Band, RowResult, Scorecard};

fn fully_green() -> Scorecard {
    let mut card = Scorecard::new(Band::M1Identity);
    for r in id_m1_required_rows() {
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
    assert!(
        card.is_green(),
        "every required Id-M1 row proven ⇒ GREEN (M2 may start)"
    );
    assert!(card.missing_required().is_empty());
    assert!(card.not_proven().is_empty());
    let md = card.render_markdown("2026-06-19");
    assert!(md.contains("GREEN - M2 may start"));
    assert!(md.contains("P-ID-31"));
    assert!(md.contains("ID-D9"));
}

#[test]
fn all_eight_id_drills_plus_coverage_are_required() {
    let ids: Vec<&str> = id_m1_required_rows().iter().map(|r| r.id).collect();
    for must in [
        "ID-D1",
        "ID-D2",
        "ID-D3",
        "ID-D4",
        "ID-D5",
        "ID-D6",
        "ID-D7",
        "ID-D8",
        "contract-coverage",
    ] {
        assert!(
            ids.contains(&must),
            "Id-M1 gate is missing required row {must}"
        );
    }
    assert_eq!(ids.len(), 9);
}

#[test]
fn dropping_any_single_row_reds_the_gate() {
    for dropped in id_m1_required_rows() {
        let mut card = Scorecard::new(Band::M1Identity);
        for r in id_m1_required_rows()
            .into_iter()
            .filter(|r| r.id != dropped.id)
        {
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
            "dropping required Id-M1 row {} must RED the M1→M2 gate",
            dropped.id
        );
    }
}

#[test]
fn any_claimed_not_proven_row_reds_the_gate() {
    for red_id in id_m1_required_rows().into_iter().map(|r| r.id) {
        let mut card = Scorecard::new(Band::M1Identity);
        for r in id_m1_required_rows() {
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
            "a claimed-not-proven {red_id} row must block M2 (the gate invariant)"
        );
        assert!(card
            .render_markdown("2026-06-19")
            .contains("RED - M2 is BLOCKED"));
    }
}

#[test]
#[should_panic(expected = "no green without proof")]
fn green_without_proof_is_structurally_impossible() {
    let _ = RowResult::pass("ID-D3", "", "2026-06-19");
}
