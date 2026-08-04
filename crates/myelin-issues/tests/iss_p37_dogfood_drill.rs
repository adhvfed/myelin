use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_issues::dogfood::{
    proven_issues_rows, run_issues_over_myelins_own_work, run_issues_truth_up_scorecard,
    IssuesIncident, IssuesTruthUpPass,
};

const RUN_DATE: &str = "2026-06-26";

#[test]
fn myelin_tracks_its_own_issues() {
    let artifact = run_issues_over_myelins_own_work(RUN_DATE);

    assert!(
        artifact.is_green(),
        "Issues must be green on Myelin's own work: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.issues_round_tripped,
        artifact.issues_total,
        "every one of Myelin's own issues round-trips through the ONE WASM render path: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.total_leaks(),
        0,
        "0 leak across the three E2E faces: {}",
        artifact.summary()
    );

    let line = artifact.summary();
    assert!(
        line.contains("P-520 ISSUES DOGFOOD 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    assert!(
        line.contains("tenant=myelin") && line.contains("region=fr-par"),
        "self-tenant framing: {line}"
    );
    println!("{line}");
}

#[test]
fn the_truth_up_pass_is_green_with_proof_sources_on_disk() {
    let rows = proven_issues_rows(RUN_DATE);
    assert!(
        rows.len() >= 16,
        "the PROVEN set covers ISS-D1..ISS-D13 + the E2E slices E2E-1/E2E-2/E2E-3"
    );
    let confirmed = IssuesTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red later-band Issues gates - every PROVEN row dated");
    assert_eq!(confirmed, rows.len());

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let scorecard = run_issues_truth_up_scorecard(RUN_DATE, &repo_root);
    assert!(
        scorecard.is_green(),
        "the truth-up scorecard must be green; claimed-not-proven: {:?}",
        scorecard.claimed_not_proven()
    );
    let md = scorecard.render();
    assert!(
        md.contains("verdict=GREEN") && md.contains("ISS-D1") && md.contains("E2E-3"),
        "the rendered scorecard: {md}"
    );
    print!("{md}");
}

#[test]
fn the_every_incident_loop_joins_the_permanent_suite_and_re_runs_green() {
    let incident = IssuesIncident::new(
        "INC-ISS-DOGFOOD-1",
        "ISS-D10",
        "an issue-body corpus fixture silently round-tripped non-canonically on the Myelin self-tenant",
        "repro_iss_d10_dogfood_non_canonical_round_trip",
    );
    let draft = incident.issue_draft();
    assert!(
        draft
            .body
            .contains("repro_iss_d10_dogfood_non_canonical_round_trip"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );
    assert!(!draft.body.to_lowercase().contains("email"));

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        incident.drill_ticket().drill_name,
        move |ctx: &mut DrillContext| {
            let artifact = run_issues_over_myelins_own_work(RUN_DATE);
            ctx.signals.set_scalar(
                SignalName::DeadLetterCount,
                if artifact.is_green() { 0 } else { 1 },
            );
            ctx.signals
                .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        },
    ));
    assert_eq!(registry.len(), 1, "the repro joined the permanent suite");

    let first = registry.run_all();
    let second = registry.run_all();
    assert!(
        first[0].is_pass(),
        "the registered drill must pass: {:?}",
        first[0]
    );
    assert!(second[0].is_pass(), "it re-runs green forever");
    assert!(
        registry.all_green(),
        "the suite is green with the repro registered"
    );
}

#[test]
fn dogfood_spine_end_to_end() {
    let artifact = run_issues_over_myelins_own_work(RUN_DATE);
    assert!(
        artifact.is_green(),
        "Issues is green on Myelin's own work: {}",
        artifact.summary()
    );

    let rows = proven_issues_rows(RUN_DATE);
    IssuesTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("the truth-up pass is green - every PROVEN Issues row dated");

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "iss_p37_dogfood_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let whole = run_issues_over_myelins_own_work(RUN_DATE).is_green();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(
        registry.all_green(),
        "the dogfood spine repro re-runs green"
    );

    println!(
        "[P-520 ISSUES DOGFOOD GREEN {RUN_DATE}] Myelin tracks its own issues: own work as Myelin \
         issues (round-trip) + the PR context pane + the agent-native flagship (exactly-once close) + \
         the spec-to-ship lineage all green, 0 leak; the truth-up pass confirms 0 red later-band Issues \
         gate (ISS-D1..D13 + the E2E slices, every row dated); the every-incident-adds-a-drill loop is \
         self-hosted"
    );
}
