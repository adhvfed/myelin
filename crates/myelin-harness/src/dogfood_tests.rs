//! Tests for the substrate dogfood (P-S38 → P-510): the every-incident-adds-a-drill loop on
//! Myelin's tracker (live) + the substrate truth-up pass (the committed scorecard).

use super::*;
use crate::scorecard::today_iso;

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  1. The every-incident-adds-a-drill loop on Myelin's own tracker (live).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **THE REQUIRED TEST: the incident-loop produces a Myelin issue + a reproducing drill on a
/// simulated incident (the loop is live).** A simulated substrate incident (an outbox relay stall)
/// files a Myelin issue ref on the platform's own tracker AND registers a reproducing drill into the
/// substrate's real [`DrillRegistry`] via the P-S04 `register_drill` hook. The loop is satisfied: the
/// incident is guarded (issue + drill) and the repro re-runs GREEN (the regression stays fixed).
#[test]
fn incident_loop_files_a_myelin_issue_and_a_reproducing_drill_on_a_simulated_incident() {
    let mut loop_ = SubstrateIncidentLoop::new();

    // The simulated substrate incident: an outbox relay stall. It files a Myelin issue on the
    // platform's OWN tracker (the dogfood) AND registers a reproducing drill (the regression).
    loop_.record(
        "INC-outbox-relay-stall",
        "myelin-issues#SUB-INC-1", // the Myelin issue ref on the platform's own tracker
        outbox_relay_stall_repro(),
    );

    // (a) The incident filed a Myelin issue.
    let incidents = loop_.incidents();
    assert_eq!(incidents.len(), 1, "the simulated incident was recorded");
    assert_eq!(
        incidents[0].issue_ref,
        Some("myelin-issues#SUB-INC-1"),
        "the incident filed a Myelin issue on the platform's own tracker"
    );

    // (b) The incident registered a reproducing drill (the regression joins the permanent suite).
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

    // (c) The incident is GUARDED (it filed BOTH legs) and the loop is satisfied.
    assert!(incidents[0].is_guarded(), "the incident is guarded");
    assert!(
        loop_.unguarded_incidents().is_empty(),
        "no unguarded incidents"
    );

    // (d) THE LIVE half: the reproducing drill RE-RUNS GREEN — the regression stays fixed (the loop
    // is not a ref check; the repro actually re-runs forever and reads green).
    assert!(
        loop_.red_repros().is_empty(),
        "the reproducing drill re-runs green — the regression stays fixed"
    );
    assert!(
        loop_.is_satisfied(),
        "the every-incident-adds-a-drill loop is satisfied self-hosted on Myelin's tracker"
    );
}

/// An UNGUARDED incident (filed with no reproducing drill) is a LOUD gap — reported, never silently
/// passed (EI-01 §3/§5: an incident that does not add a drill is a gap, not a green).
#[test]
fn an_incident_without_a_reproducing_drill_is_a_loud_gap() {
    let mut loop_ = SubstrateIncidentLoop::new();
    loop_.record_unguarded(SubstrateIncident {
        id: "INC-no-drill",
        issue_ref: Some("myelin-issues#SUB-INC-2"),
        repro_drill_id: None, // filed an issue but NO reproducing drill — a gap
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

/// A registered repro that reads RED (a regression resurfaced) is reported loudly via `red_repros`
/// and reds the loop — the "re-runs forever" guarantee is live, not vacuous.
#[test]
fn a_regressed_repro_reds_the_loop_loudly() {
    let mut loop_ = SubstrateIncidentLoop::new();
    // Record a guarded incident whose repro asserts a property that is BROKEN at run time (the
    // regression resurfaced): the outbox dead-lettered events (silent data loss).
    loop_.record(
        "INC-outbox-relay-stall",
        "myelin-issues#SUB-INC-1",
        DrillScenario::new("repro-outbox-relay-stall", |ctx| {
            ctx.signals.set_scalar(SignalName::DeadLetterCount, 3); // regression: events lost
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

// ─────────────────────────────────────────────────────────────────────────────────────────────────
//  2. The substrate truth-up pass (the committed scorecard; 0 red earlier rows).
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **THE REQUIRED TEST: the truth-up pass is a committed scorecard — every substrate PROVEN row rests
/// on a dated green artifact; 0 red earlier rows.** Every enumerated PROVEN substrate row carries a
/// dated artifact ⇒ the pass is GREEN ⇒ the gate invariant holds end-to-end (no earlier substrate
/// gate is red).
#[test]
fn truth_up_pass_is_green_every_substrate_proven_row_is_dated() {
    let date = today_iso();
    let rows = proven_substrate_rows(&date);
    // The expected substrate PROVEN gate/drill family is present (no row silently dropped).
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
        "every substrate PROVEN row rests on a dated green artifact — 0 claimed-not-proven: {:?}",
        verdict.undated_rows()
    );

    // The CI-failing form returns the confirmed count (0 red earlier rows).
    let confirmed = pass
        .run_or_fail_ci(&rows, &date)
        .expect("the substrate truth-up pass is GREEN — 0 red earlier substrate gates");
    assert_eq!(
        confirmed,
        rows.len(),
        "every PROVEN substrate row was confirmed dated + green"
    );
}

/// A CLAIMED-NOT-PROVEN row (one whose proof did NOT emit a dated green) reds the pass LOUDLY, naming
/// the undated row — never a silent pass (EI-01 §1: code wins over docs; a claim that outlives its
/// verification misleads the next agent).
#[test]
fn a_claimed_not_proven_row_reds_the_truth_up_pass_loudly() {
    let date = today_iso();
    let mut rows = proven_substrate_rows(&date);
    // Simulate a doc claim that no longer rests on a green artifact (the proof command did not pass).
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

/// The truth-up pass renders a DATED committed scorecard (the prompt's named green artifact): every
/// PROVEN row → its dated green artifact, the GREEN verdict line, and the one named fleet-hardware
/// floor. Print it so a CI run surfaces the artifact (observability is part of the pass, EI-01 §3).
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
    // Surface it (the committed artifact body).
    println!("{md}");
}
