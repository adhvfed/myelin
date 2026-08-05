use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_flow::{
    proven_flow_rows, run_flow_over_myelins_own_work, run_flow_truth_up_scorecard, FlowIncident,
    FlowTruthUpPass,
};

const RUN_DATE: &str = "2026-06-26";

#[test]
fn myelins_own_workflows_run_on_the_self_hosting_platform() {
    let artifact = run_flow_over_myelins_own_work(RUN_DATE);

    assert!(
        artifact.is_green(),
        "Myelin's pipelines/merge-queue/SLA-timers must run green as myelin-flow workflows: {}",
        artifact.summary()
    );

    assert!(
        artifact.pipeline.is_green(),
        "Myelin's CI pipeline as a ci.pipeline workflow"
    );
    assert!(artifact.pipeline.completed);
    assert_eq!(
        artifact.pipeline.dispatches, 3,
        "0 re-dispatch - one dispatch per stage"
    );

    assert!(
        artifact.merge_queue.is_green(),
        "Myelin's merge queue merges a real Myelin PR"
    );
    assert_eq!(
        artifact.merge_queue.merges, 1,
        "merge-count == 1 (0 double-merge)"
    );
    assert_eq!(
        artifact.merge_queue.git_pr_merged_emits, 1,
        "one git.pr.merged emit"
    );

    assert!(
        artifact.sla_timer.is_green(),
        "a real Myelin SLA timer fires on a real Myelin issue"
    );
    assert!(artifact.sla_timer.fired, "the breach SLA timer FIRED");

    let line = artifact.summary();
    assert!(
        line.contains("P-516 FLOW SELF_TENANT 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn the_truth_up_pass_confirms_every_proven_flow_row_is_dated() {
    let rows = proven_flow_rows(RUN_DATE);
    assert!(
        rows.len() >= 11,
        "the PROVEN set covers FLOW-D1..FLOW-D10 + the E2E-2 spine"
    );

    let confirmed = FlowTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band FLOW gates - every PROVEN row rests on a dated green artifact");
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let scorecard = run_flow_truth_up_scorecard(RUN_DATE, &repo_root);
    assert!(
        scorecard.is_green(),
        "every PROVEN FLOW row's proof source must exist on disk; claimed-not-proven: {:?}",
        scorecard.claimed_not_proven()
    );

    println!(
        "[P-516 TRUTH-UP GREEN {RUN_DATE}] {confirmed} PROVEN FLOW rows rest on a dated green artifact \
         (0 red earlier-band gates); scorecard {}/{} dated-green\n{}",
        scorecard.rows_dated_green(),
        scorecard.rows_total(),
        scorecard.render(),
    );
}

#[test]
fn a_flow_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = FlowIncident::new(
        "INC-FLOW-SELF_TENANT-1",
        "FLOW-D1",
        "a replay-recovery regression dropped a journaled effect on the Myelin self-tenant",
        "repro_flow_d1_self_tenant_replay_recovery",
    );

    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "FLOW-D1");
    assert!(draft.title.contains("INC-FLOW-SELF_TENANT-1"));
    assert!(
        draft.body.contains("repro_flow_d1_self_tenant_replay_recovery"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );

    let ticket = incident.drill_ticket();
    let drill_name = ticket.drill_name.clone();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let self_tenant = run_flow_over_myelins_own_work(RUN_DATE);
            ctx.signals.set_scalar(
                SignalName::DeadLetterCount,
                if self_tenant.is_green() { 0 } else { 1 },
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
fn self_tenant_loop_end_to_end_self_hosting() {
    let self_tenant = run_flow_over_myelins_own_work(RUN_DATE);
    assert!(
        self_tenant.is_green(),
        "Myelin's pipelines/merge-queue/SLA-timers run green as myelin-flow workflows: {}",
        self_tenant.summary()
    );

    let rows = proven_flow_rows(RUN_DATE);
    let confirmed = FlowTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band FLOW gates");
    assert!(confirmed >= 11);

    let incident = FlowIncident::new(
        "INC-FLOW-SELF_TENANT-E2E",
        "E2E-2",
        "a merge-queue wake double-merged a Myelin PR under an at-least-once ci.result re-delivery",
        "repro_e2e2_self_tenant_merge_queue_exactly_once",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        ticket.drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let whole = run_flow_over_myelins_own_work(RUN_DATE);
            let double_merge = if whole.merge_queue.merges == 1 { 0 } else { 1 };
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, double_merge);
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the incident's repro re-runs green");

    println!(
        "[P-516 SELF_TENANT LOOP GREEN {RUN_DATE}] self-hosting: Myelin's own pipelines/merge-queue/\
         SLA-timers run as myelin-flow workflows (ci-pipeline + merge-queue(merge==1) + sla-timer fired, \
         all green); truth-up confirms {confirmed} PROVEN FLOW rows dated; incident→issue→repro-drill \
         registered + re-runs green"
    );
}
