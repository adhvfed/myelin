use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_knowledge::dogfood::{
    myelin_knowledge_space, proven_knowledge_rows, run_knowledge_over_myelins_own_work,
    run_knowledge_truth_up_scorecard, KnowledgeIncident, KnowledgeTruthUpPass,
};

const RUN_DATE: &str = "2026-06-26";

#[test]
fn knowledge_hosts_myelins_own_docs() {
    let artifact = run_knowledge_over_myelins_own_work(RUN_DATE);

    assert!(
        artifact.is_green(),
        "Knowledge must be green on Myelin's own work: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.docs_round_tripped,
        artifact.docs_total,
        "every one of Myelin's own docs round-trips through the ONE render path: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.total_leaks(),
        0,
        "0 leak across the two E2E faces: {}",
        artifact.summary()
    );

    let line = artifact.summary();
    assert!(
        line.contains("P-519 KNOWLEDGE DOGFOOD 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    assert!(
        line.contains("tenant=myelin") && line.contains("region=fr-par"),
        "self-tenant framing: {line}"
    );
    println!("{line}");
}

#[test]
fn myelins_knowledge_space_is_the_teams_own_work() {
    let space = myelin_knowledge_space();
    assert!(space.len() >= 3, "the roadmap/gap-report/scorecard");
    assert!(space.iter().any(|d| d.page_id == "myelin-roadmap"));
    assert!(space.iter().any(|d| d.page_id == "myelin-gap-report"));
    assert!(space.iter().any(|d| d.page_id == "myelin-scorecard"));
    for doc in &space {
        assert!(
            doc.round_trips(),
            "the Myelin doc {} round-trips through the ONE render path",
            doc.page_id
        );
    }
}

#[test]
fn the_truth_up_pass_is_green_with_proof_sources_on_disk() {
    let rows = proven_knowledge_rows(RUN_DATE);
    assert!(
        rows.len() >= 15,
        "the PROVEN set covers KN-D1..KN-D13 + the E2E slices"
    );
    let confirmed = KnowledgeTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red later-band Knowledge gates - every PROVEN row dated");
    assert_eq!(confirmed, rows.len());

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let scorecard = run_knowledge_truth_up_scorecard(RUN_DATE, &repo_root);
    assert!(
        scorecard.is_green(),
        "the truth-up scorecard must be green; claimed-not-proven: {:?}",
        scorecard.claimed_not_proven()
    );
    let md = scorecard.render();
    assert!(
        md.contains("verdict=GREEN") && md.contains("KN-D1") && md.contains("E2E-3"),
        "the rendered scorecard: {md}"
    );
    print!("{md}");
}

#[test]
fn the_every_incident_loop_joins_the_permanent_suite_and_re_runs_green() {
    let incident = KnowledgeIncident::new(
        "INC-KN-DOGFOOD-1",
        "KN-D2",
        "a markdown-subset corpus body silently round-tripped non-canonically on the Myelin self-tenant",
        "repro_kn_d2_dogfood_non_canonical_round_trip",
    );
    let draft = incident.issue_draft();
    assert!(
        draft
            .body
            .contains("repro_kn_d2_dogfood_non_canonical_round_trip"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );
    assert!(!draft.body.to_lowercase().contains("email"));

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        incident.drill_ticket().drill_name,
        move |ctx: &mut DrillContext| {
            let artifact = run_knowledge_over_myelins_own_work(RUN_DATE);
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
    let artifact = run_knowledge_over_myelins_own_work(RUN_DATE);
    assert!(
        artifact.is_green(),
        "Knowledge is green on Myelin's own work: {}",
        artifact.summary()
    );

    let rows = proven_knowledge_rows(RUN_DATE);
    KnowledgeTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("the truth-up pass is green - every PROVEN Knowledge row dated");

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "kn_p34_dogfood_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let whole = run_knowledge_over_myelins_own_work(RUN_DATE).is_green();
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

    println!("{}", artifact.summary());
}
