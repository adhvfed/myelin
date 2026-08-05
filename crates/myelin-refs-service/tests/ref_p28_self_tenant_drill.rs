use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_refs_service::{
    proven_refs_rows, run_refs_over_myelins_own_work, run_refs_truth_up_scorecard, RefsIncident,
    RefsTruthUpPass,
};

const RUN_DATE: &str = "2026-06-26";

#[test]
fn the_reference_graph_runs_on_myelins_own_work() {
    let artifact = run_refs_over_myelins_own_work(RUN_DATE);

    assert!(
        artifact.is_green(),
        "the reference graph must be green on Myelin's own work: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.total_leaks(),
        0,
        "0 leak across the three faces: {}",
        artifact.summary()
    );
    assert_eq!(artifact.pr_pane.scenario, "E2E-1");
    assert_eq!(artifact.spec_to_ship.scenario, "E2E-3");
    assert_eq!(artifact.holder_fanout.scenario, "E2E-4");

    let line = artifact.summary();
    assert!(
        line.contains("P-513 REFS SELF_TENANT 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn the_truth_up_pass_confirms_every_proven_refs_row_is_dated() {
    let rows = proven_refs_rows(RUN_DATE);
    assert!(
        rows.len() >= 13,
        "the PROVEN set covers REF-D1..REF-D10 + the E2E legs (E2E-1/E2E-3/E2E-4)"
    );

    let confirmed = RefsTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band Refs gates - every PROVEN row rests on a dated green artifact");
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let scorecard = run_refs_truth_up_scorecard(RUN_DATE, &repo_root);
    assert!(
        scorecard.is_green(),
        "every PROVEN Refs row's proof source must exist on disk; claimed-not-proven: {:?}",
        scorecard.claimed_not_proven()
    );

    println!(
        "[P-513 TRUTH-UP GREEN {RUN_DATE}] {confirmed} PROVEN Refs rows rest on a dated green artifact \
         (0 red earlier-band gates); scorecard {}/{} dated-green",
        scorecard.rows_dated_green(),
        scorecard.rows_total()
    );
}

#[test]
fn a_refs_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = RefsIncident::new(
        "INC-REFS-SELF_TENANT-1",
        "REF-D1",
        "a resolve chokepoint regression leaked a denied issue title on the Myelin self-tenant",
        "repro_ref_d1_self_tenant_resolve_leak",
    );

    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "REF-D1");
    assert!(draft.title.contains("INC-REFS-SELF_TENANT-1"));
    assert!(
        draft.body.contains("repro_ref_d1_self_tenant_resolve_leak"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );

    let ticket = incident.drill_ticket();
    let drill_name = ticket.drill_name.clone();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let self_tenant = run_refs_over_myelins_own_work(RUN_DATE);
            ctx.signals.set_scalar(
                SignalName::DeadLetterCount,
                if self_tenant.is_green() && self_tenant.total_leaks() == 0 {
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
fn self_tenant_loop_end_to_end_self_hosting() {
    let self_tenant = run_refs_over_myelins_own_work(RUN_DATE);
    assert!(
        self_tenant.is_green() && self_tenant.total_leaks() == 0,
        "the reference graph is green on Myelin's own work: {}",
        self_tenant.summary()
    );

    let rows = proven_refs_rows(RUN_DATE);
    let confirmed = RefsTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band Refs gates");
    assert!(confirmed >= 13);

    let incident = RefsIncident::new(
        "INC-REFS-SELF_TENANT-E2E",
        "E2E-3",
        "a spec-to-ship lineage traverse dropped a reindex-parity node under a doc-edit surge",
        "repro_e2e3_self_tenant_lineage_reindex_parity",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        ticket.drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let whole = run_refs_over_myelins_own_work(RUN_DATE).is_green();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the incident's repro re-runs green");

    println!(
        "[P-513 SELF_TENANT LOOP GREEN {RUN_DATE}] self-hosting: the reference graph runs on Myelin's own \
         work (PR-pane + spec-to-ship + holder-fanout green, 0 leak); truth-up confirms {confirmed} \
         PROVEN Refs rows dated; incident→issue→repro-drill registered + re-runs green"
    );
}
