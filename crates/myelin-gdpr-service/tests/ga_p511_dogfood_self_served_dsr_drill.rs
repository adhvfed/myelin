use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_gdpr_service::dogfood::{
    proven_gdpr_rows, run_audit_consumer_on_dogfood, run_self_served_dsr_on_dogfood, GdprIncident,
    RopaKnowledgeSpace, TruthUpPass,
};
use myelin_tenancy::Region;

const RUN_DATE: &str = "2026-06-26";

#[test]
fn the_audit_consumer_runs_on_myelins_own_actions() {
    let artifact = run_audit_consumer_on_dogfood(RUN_DATE);

    assert!(
        artifact.audit_graph_is_green(),
        "the audit graph must be green on the platform's own actions: {artifact:?}"
    );
    assert_eq!(
        artifact.actions_logged, 5,
        "all five own-action surfaces logged"
    );
    assert!(
        artifact.chain_verifies,
        "the per-tenant hash-chain verifies"
    );
    assert_eq!(artifact.append_lag, 0, "audit_append_lag reads green");

    let line = artifact.summary();
    assert!(
        line.contains("P-511 DOGFOOD AUDIT GREEN 2026-06-26"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn a_self_served_dsr_over_the_teams_own_data_seals_a_certificate() {
    let artifact = run_self_served_dsr_on_dogfood(RUN_DATE);

    assert!(
        artifact.dsr_is_green(),
        "the self-served DSR must be green on the platform's own data: {artifact:?}"
    );
    assert_eq!(artifact.holders_missed, 0, "GA-D1: 0 holders missed");
    assert_eq!(artifact.cells_missed, 0, "GA-D8: 0 cells missed");
    assert!(
        artifact.certificate_sealed,
        "the completion certificate seals into the per-tenant audit Merkle tree"
    );
    let inclusion = artifact
        .inclusion_proof
        .as_ref()
        .expect("the sealed certificate carries a Merkle inclusion proof");
    assert!(
        inclusion.contains("->blake3:"),
        "inclusion proof: {inclusion}"
    );

    let line = artifact.summary();
    assert!(
        line.contains("P-511 DOGFOOD DSR GREEN 2026-06-26") && line.contains("certificate=SEALED"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn the_ropa_and_data_map_live_as_a_myelin_knowledge_space() {
    let space = RopaKnowledgeSpace::for_myelin_team(Region("fr-par".into()));

    assert!(
        space.is_populated(),
        "the RoPA Knowledge space is populated"
    );
    let pages = space.render_pages();
    assert_eq!(pages.len(), 2, "the data-map page + the RoPA page");
    assert!(
        pages[0].body.contains("blake3:"),
        "the data-map page carries the generated map's fingerprint"
    );
    println!(
        "[P-511 ROPA SPACE {RUN_DATE}] '{}' - {} pages, data-map entries={}",
        space.title(),
        pages.len(),
        space.data_map().entry_count()
    );
}

#[test]
fn a_synthetic_gdpr_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = GdprIncident::new(
        "INC-GDPR-DOGFOOD-1",
        "GA-D1",
        "a self-served DSR fan-out skipped a newly-registered holder on Myelin's own tenant",
        "repro_ga_d1_dogfood_dsr_skips_holder",
    );

    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "GA-D1");
    assert!(draft.title.contains("INC-GDPR-DOGFOOD-1"));
    assert!(
        draft.body.contains("repro_ga_d1_dogfood_dsr_skips_holder"),
        "the issue is traceable to its repro drill: {}",
        draft.body
    );

    let ticket = incident.drill_ticket();
    let drill_name = ticket.drill_name.clone();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let dsr = run_self_served_dsr_on_dogfood(RUN_DATE);
            ctx.signals.set_scalar(
                SignalName::DeadLetterCount,
                if dsr.dsr_is_green() { 0 } else { 1 },
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
        "the suite is green with the incident's repro registered"
    );
    assert_eq!(first[0].name(), drill_name);
}

#[test]
fn the_truth_up_pass_confirms_every_proven_gdpr_row_is_dated() {
    let rows = proven_gdpr_rows(RUN_DATE);
    assert!(
        rows.len() >= 11,
        "the PROVEN set covers the §9.2 GA-D*/GA-10/GA-11 family + the E2E legs"
    );

    let confirmed = TruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band GDPR gates - every PROVEN row rests on a dated green artifact");
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    println!("[P-511 TRUTH-UP GREEN {RUN_DATE}] {confirmed} PROVEN GDPR rows rest on a dated green artifact (0 red earlier-band gates)");
}

#[test]
fn dogfood_loop_end_to_end_self_hosting() {
    let audit = run_audit_consumer_on_dogfood(RUN_DATE);
    assert!(audit.audit_graph_is_green(), "audit green on own actions");

    let dsr = run_self_served_dsr_on_dogfood(RUN_DATE);
    assert!(
        dsr.dsr_is_green(),
        "self-served DSR green + certificate sealed"
    );

    let space = RopaKnowledgeSpace::for_myelin_team(Region("fr-par".into()));
    assert!(space.is_populated(), "the RoPA Knowledge space lives");

    let incident = GdprIncident::new(
        "INC-GDPR-DOGFOOD-E2E",
        "GA-D8",
        "a multi-cell self-served DSR wave dropped a self-host cell under a doc-edit surge",
        "repro_ga_d8_dogfood_cell_drop_under_surge",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        ticket.drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let whole = run_self_served_dsr_on_dogfood(RUN_DATE).dsr_is_green();
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, if whole { 0 } else { 1 });
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the incident's repro re-runs green");

    let rows = proven_gdpr_rows(RUN_DATE);
    let confirmed = TruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band GDPR gates");
    assert!(confirmed >= 11);

    println!("[P-511 DOGFOOD LOOP GREEN {RUN_DATE}] self-hosting: audit consumer green on own actions; self-served DSR green + certificate sealed; RoPA/data-map lives as a Knowledge space; incident→issue→repro-drill registered + re-runs green; truth-up confirms {confirmed} PROVEN GDPR rows dated");
}
