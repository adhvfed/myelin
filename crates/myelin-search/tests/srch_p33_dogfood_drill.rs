use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_search::{
    proven_search_rows, run_search_over_myelins_own_work, run_search_truth_up_scorecard,
    SearchIncident, SearchTruthUpPass, EMBEDDING_ADAPTER_POSTURE,
};

const RUN_DATE: &str = "2026-06-26";

#[test]
fn search_runs_on_myelins_own_work() {
    let artifact = run_search_over_myelins_own_work(RUN_DATE);

    assert!(
        artifact.is_green(),
        "Search must be green on Myelin's own work: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.total_leaks(),
        0,
        "0 leak across the three faces: {}",
        artifact.summary()
    );
    assert_eq!(artifact.code_and_issue.scenario, "E2E-1");
    assert_eq!(artifact.knowledge_space.scenario, "E2E-3");
    assert_eq!(artifact.dsar_fanout.scenario, "E2E-4");

    let line = artifact.summary();
    assert!(
        line.contains("P-515 SEARCH DOGFOOD 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    assert!(
        line.contains("embedding-adapter=mock") && EMBEDDING_ADAPTER_POSTURE.contains("mock"),
        "the embedding-adapter posture is recorded honestly: {line}"
    );
    println!("{line}");
}

#[test]
fn the_truth_up_pass_confirms_every_proven_search_row_is_dated() {
    let rows = proven_search_rows(RUN_DATE);
    assert!(
        rows.len() >= 13,
        "the PROVEN set covers SRCH-D1..SRCH-D10 + the E2E legs (E2E-1/E2E-3/E2E-4)"
    );

    let confirmed = SearchTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect(
            "0 red earlier-band Search gates - every PROVEN row rests on a dated green artifact",
        );
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let scorecard = run_search_truth_up_scorecard(RUN_DATE, &repo_root);
    assert!(
        scorecard.is_green(),
        "every PROVEN Search row's proof source must exist on disk; claimed-not-proven: {:?}",
        scorecard.claimed_not_proven()
    );

    println!(
        "[P-515 TRUTH-UP GREEN {RUN_DATE}] {confirmed} PROVEN Search rows rest on a dated green \
         artifact (0 red earlier-band gates); scorecard {}/{} dated-green",
        scorecard.rows_dated_green(),
        scorecard.rows_total()
    );
}

#[test]
fn a_search_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = SearchIncident::new(
        "INC-SEARCH-DOGFOOD-1",
        "SRCH-D1",
        "a pre-filter regression let a confidential issue enter the candidate set on the Myelin self-tenant",
        "repro_srch_d1_dogfood_candidate_leak",
    );

    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "SRCH-D1");
    assert!(draft.title.contains("INC-SEARCH-DOGFOOD-1"));
    assert!(
        draft.body.contains("repro_srch_d1_dogfood_candidate_leak"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );

    let ticket = incident.drill_ticket();
    let drill_name = ticket.drill_name.clone();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let dogfood = run_search_over_myelins_own_work(RUN_DATE);
            ctx.signals.set_scalar(
                SignalName::DeadLetterCount,
                if dogfood.is_green() && dogfood.total_leaks() == 0 {
                    0
                } else {
                    1
                },
            );
            ctx.signals
                .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        },
    ));
    assert_eq!(
        registry.len(),
        1,
        "the incident's repro joined the permanent suite"
    );

    let first = registry.run_all();
    let second = registry.run_all();
    assert!(
        first[0].is_pass(),
        "the registered repro drill must pass: {:?}",
        first[0]
    );
    assert!(second[0].is_pass(), "it re-runs green forever");
    assert!(
        registry.all_green(),
        "the suite is green with the repro registered"
    );
    assert_eq!(first[0].name(), drill_name);
}

#[test]
fn dogfood_loop_end_to_end_self_hosting() {
    let dogfood = run_search_over_myelins_own_work(RUN_DATE);
    assert!(
        dogfood.is_green() && dogfood.total_leaks() == 0,
        "Search is green on Myelin's own work: {}",
        dogfood.summary()
    );

    let rows = proven_search_rows(RUN_DATE);
    let confirmed = SearchTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band Search gates");
    assert!(confirmed >= 13);

    let incident = SearchIncident::new(
        "INC-SEARCH-DOGFOOD-E2E",
        "E2E-3",
        "a reindex-from-source rebuild dropped a Knowledge-space node so the parity hash diverged",
        "repro_e2e3_dogfood_reindex_parity",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        ticket.drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let whole = run_search_over_myelins_own_work(RUN_DATE).is_green();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the incident's repro re-runs green");

    println!(
        "[P-515 DOGFOOD LOOP GREEN {RUN_DATE}] self-hosting: Search runs on Myelin's own work \
         (code+issue + Knowledge-space reindex-parity + DSAR fan-out green, 0 leak); truth-up confirms \
         {confirmed} PROVEN Search rows dated; incident→issue→repro-drill registered + re-runs green; \
         embedding-adapter={EMBEDDING_ADAPTER_POSTURE}"
    );
}
