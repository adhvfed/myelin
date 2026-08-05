use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_git::self_tenant::{
    proven_git_rows, run_git_over_myelins_own_repos, run_git_truth_up_scorecard, GitIncident,
    GitTruthUpPass,
};

const RUN_DATE: &str = "2026-06-26";

#[test]
fn git_hosts_myelins_own_repositories() {
    let artifact = run_git_over_myelins_own_repos(RUN_DATE);

    assert!(
        artifact.is_green(),
        "git must be green on Myelin's own repositories: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.total_leaks(),
        0,
        "0 leak across the three faces: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.fix_pr_flagship.merge_count,
        1,
        "the agent-native flagship merge is exactly-once: {}",
        artifact.summary()
    );

    let line = artifact.summary();
    assert!(
        line.contains("P-518 GIT SELF_TENANT 2026-06-26") && line.contains("verdict=GREEN"),
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
    let rows = proven_git_rows(RUN_DATE);
    assert!(
        rows.len() >= 14,
        "the PROVEN set covers GIT-D1..GIT-D11 + the E2E slices"
    );
    let confirmed = GitTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red later-band git gates - every PROVEN row dated");
    assert_eq!(confirmed, rows.len());

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let scorecard = run_git_truth_up_scorecard(RUN_DATE, &repo_root);
    assert!(
        scorecard.is_green(),
        "the truth-up scorecard must be green; claimed-not-proven: {:?}",
        scorecard.claimed_not_proven()
    );
    let md = scorecard.render();
    assert!(
        md.contains("verdict=GREEN") && md.contains("GIT-D1") && md.contains("E2E-3"),
        "the rendered scorecard: {md}"
    );
    print!("{md}");
}

#[test]
fn the_every_incident_loop_joins_the_permanent_suite_and_re_runs_green() {
    let incident = GitIncident::new(
        "INC-GIT-SELF_TENANT-1",
        "GIT-D9",
        "a receive-pack regression left a ghost ref without its outbox event on the Myelin self-tenant",
        "repro_git_d9_self_tenant_ghost_ref",
    );
    let draft = incident.issue_draft();
    assert!(
        draft.body.contains("repro_git_d9_self_tenant_ghost_ref"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );
    assert!(!draft.body.to_lowercase().contains("email"));

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        incident.drill_ticket().drill_name,
        move |ctx: &mut DrillContext| {
            let artifact = run_git_over_myelins_own_repos(RUN_DATE);
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
fn self_tenant_spine_end_to_end() {
    let artifact = run_git_over_myelins_own_repos(RUN_DATE);
    assert!(
        artifact.is_green(),
        "git is green on Myelin's own repositories: {}",
        artifact.summary()
    );

    let rows = proven_git_rows(RUN_DATE);
    GitTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("the truth-up pass is green - every PROVEN git row dated");

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "git_p35_self_tenant_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let whole = run_git_over_myelins_own_repos(RUN_DATE).is_green();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(
        registry.all_green(),
        "the self_tenant spine repro re-runs green"
    );

    println!(
        "[P-518 GIT SELF_TENANT GREEN {RUN_DATE}] the Myelin monorepo hosted on Myelin git: the PR context \
         pane + the agent-native fix-PR flagship (exactly-once merge) + the spec-to-ship lineage all \
         green, 0 leak; the truth-up pass confirms 0 red later-band git gate (GIT-D1..D11 + the E2E \
         slices, every row dated); the every-incident-adds-a-drill loop is self-hosted"
    );
}
