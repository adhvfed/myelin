use super::*;
use crate::scorecard::today_iso;

#[test]
fn incident_loop_files_a_myelin_issue_and_a_reproducing_drill_on_a_simulated_incident() {
    let mut loop_ = SubstrateIncidentLoop::new();

    loop_.record(
        "INC-outbox-relay-stall",
        "myelin-issues#SUB-INC-1",
        outbox_relay_stall_repro(),
    );

    let incidents = loop_.incidents();
    assert_eq!(incidents.len(), 1, "the simulated incident was recorded");
    assert_eq!(
        incidents[0].issue_ref,
        Some("myelin-issues#SUB-INC-1"),
        "the incident filed a Myelin issue on the platform's own tracker"
    );

    assert_eq!(
        incidents[0].repro_drill_id,
        Some("repro-outbox-relay-stall"),
        "the incident registered a reproducing drill via the register_drill hook"
    );
    assert_eq!(
        loop_.registered_drill_count(),
        1,
        "the reproducing drill joined the substrate's real DrillRegistry"
    );

    assert!(incidents[0].is_guarded(), "the incident is guarded");
    assert!(
        loop_.unguarded_incidents().is_empty(),
        "no unguarded incidents"
    );

    assert!(
        loop_.red_repros().is_empty(),
        "the reproducing drill re-runs green - the regression stays fixed"
    );
    assert!(
        loop_.is_satisfied(),
        "the every-incident-adds-a-drill loop is satisfied self-hosted on Myelin's tracker"
    );
}

#[test]
fn an_incident_without_a_reproducing_drill_is_a_loud_gap() {
    let mut loop_ = SubstrateIncidentLoop::new();
    loop_.record_unguarded(SubstrateIncident {
        id: "INC-no-drill",
        issue_ref: Some("myelin-issues#SUB-INC-2"),
        repro_drill_id: None,
    });
    assert_eq!(
        loop_.unguarded_incidents(),
        vec!["INC-no-drill"],
        "an incident without a reproducing drill is named as an unguarded gap"
    );
    assert!(
        !loop_.is_satisfied(),
        "the loop is NOT satisfied while an incident lacks a reproducing drill"
    );
}

#[test]
fn a_regressed_repro_reds_the_loop_loudly() {
    let mut loop_ = SubstrateIncidentLoop::new();
    loop_.record(
        "INC-outbox-relay-stall",
        "myelin-issues#SUB-INC-1",
        DrillScenario::new("repro-outbox-relay-stall", |ctx| {
            ctx.signals.set_scalar(SignalName::DeadLetterCount, 3);
            ctx.signals
                .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        }),
    );
    assert_eq!(
        loop_.red_repros(),
        vec!["repro-outbox-relay-stall".to_string()],
        "a regressed repro is named loudly"
    );
    assert!(
        !loop_.is_satisfied(),
        "a red repro reds the loop (the regression resurfaced)"
    );
}

#[test]
fn truth_up_pass_is_green_every_substrate_proven_row_is_dated() {
    let date = today_iso();
    let rows = proven_substrate_rows(&date);
    assert!(
        rows.len() >= 20,
        "the substrate PROVEN row set spans SUB-D1..D11 + BUS-D4/D7 + lints + scanner + self-test \
         + the M5 world-scale/tuning legs (got {})",
        rows.len()
    );

    let pass = SubstrateTruthUpPass::new();
    let verdict = pass.run(&rows, &date);
    assert!(
        verdict.is_green(),
        "every substrate PROVEN row rests on a dated green artifact - 0 claimed-not-proven: {:?}",
        verdict.undated_rows()
    );

    let confirmed = pass
        .run_or_fail_ci(&rows, &date)
        .expect("the substrate truth-up pass is GREEN - 0 red earlier substrate gates");
    assert_eq!(
        confirmed,
        rows.len(),
        "every PROVEN substrate row was confirmed dated + green"
    );
}

#[test]
fn a_claimed_not_proven_row_reds_the_truth_up_pass_loudly() {
    let date = today_iso();
    let mut rows = proven_substrate_rows(&date);
    rows[0].artifact_date = None;
    let undated_id = rows[0].id;

    let pass = SubstrateTruthUpPass::new();
    let verdict = pass.run(&rows, &date);
    assert!(!verdict.is_green(), "an undated PROVEN row reds the pass");
    assert_eq!(
        verdict.undated_rows(),
        &[undated_id],
        "the claimed-not-proven row is named loudly"
    );

    let err = pass
        .run_or_fail_ci(&rows, &date)
        .expect_err("a claimed-not-proven row must FAIL CI");
    assert!(
        err.to_string().contains(undated_id),
        "the loud error names the claimed-not-proven row: {err}"
    );
}

#[test]
fn truth_up_pass_renders_a_dated_committed_scorecard() {
    let date = today_iso();
    let rows = proven_substrate_rows(&date);
    let pass = SubstrateTruthUpPass::new();
    let md = pass.render_markdown(&rows, &date);

    assert!(md.contains("Substrate truth-up pass"), "titled");
    assert!(md.contains(&format!("Run date: {date}")), "dated");
    assert!(md.contains("`SUB-D1`"), "names a PROVEN row");
    assert!(
        md.contains("`SUB-D6/STOR-D2-cell`"),
        "names the cell-scale restore row"
    );
    assert!(
        md.contains("TRUTH-UP: GREEN"),
        "the scorecard reads GREEN (0 claimed-not-proven)"
    );
    assert!(
        md.contains("FLEET-hardware load drill"),
        "the one legitimate remaining floor is named (never silently claimed closed, EI-01 §1)"
    );
    println!("{md}");
}
