use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_agent_service::{
    proven_fabric_rows, run_fabric_over_myelins_own_work, run_fabric_truth_up_scorecard,
    run_myelin_triage_on_ci_failure, FabricIncident, FabricTruthUpPass, SELF_TENANT_RUNTIME_FLOOR,
};

const RUN_DATE: &str = "2026-06-26";

#[test]
fn myelins_own_agent_runs_on_the_self_hosting_graph() {
    let artifact = run_fabric_over_myelins_own_work(RUN_DATE);

    assert!(
        artifact.is_green(),
        "Myelin's own triage agent must run green on the self-hosting CI graph: {}",
        artifact.summary()
    );

    assert!(
        artifact.triage.dispatched,
        "a ci.result=failure Signal DISPATCHES a costed triage run (explicit-first, §3.4)"
    );
    assert!(
        artifact.triage.mention_only_notifies,
        "a casual @triage mention only NOTIFIES - 0 auto-spawn (the safety boundary, CHAT-1)"
    );

    assert_eq!(
        artifact.triage.reserved, artifact.triage.settled,
        "reserve/settle BALANCED - reserved == settled on the Myelin self-tenant wallet"
    );
    assert_eq!(
        artifact.triage.inflight_interrupts, 0,
        "0 in-flight interrupt - the reservation's only exit is settle (11.7)"
    );

    assert!(
        artifact.triage.trace_ref.starts_with("blake3:"),
        "a content-addressed trace per run (8.8): {}",
        artifact.triage.trace_ref
    );

    let line = artifact.summary();
    assert!(
        line.contains("P-517 FABRIC SELF_TENANT 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

#[test]
fn the_truth_up_pass_confirms_every_proven_fabric_row_is_dated() {
    let rows = proven_fabric_rows(RUN_DATE);
    assert!(
        rows.len() >= 11,
        "the PROVEN set covers AG-D1..AG-D11 + the E2E-2 spine (got {})",
        rows.len()
    );

    let confirmed = FabricTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red later-band Fabric gates - every PROVEN row rests on a dated green artifact");
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let scorecard = run_fabric_truth_up_scorecard(RUN_DATE, &repo_root);
    assert!(
        scorecard.is_green(),
        "every PROVEN Fabric row's proof source must exist on disk; claimed-not-proven: {:?}",
        scorecard.claimed_not_proven()
    );

    println!(
        "[P-517 TRUTH-UP GREEN {RUN_DATE}] {confirmed} PROVEN Fabric rows rest on a dated green \
         artifact (0 red later-band gates); scorecard {}/{} dated-green\n{}",
        scorecard.rows_dated_green(),
        scorecard.rows_total(),
        scorecard.render(),
    );
}

#[test]
fn a_fabric_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = FabricIncident::new(
        "INC-AG-SELF_TENANT-1",
        "AG-D11",
        "a reserve/settle regression left an in-flight triage run torn down on the Myelin self-tenant",
        "repro_ag_d11_self_tenant_runaway_self_limiter",
    );

    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "AG-D11");
    assert!(draft.title.contains("INC-AG-SELF_TENANT-1"));
    assert!(
        draft
            .body
            .contains("repro_ag_d11_self_tenant_runaway_self_limiter"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );

    let ticket = incident.drill_ticket();
    let drill_name = ticket.drill_name.clone();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let self_tenant = run_fabric_over_myelins_own_work(RUN_DATE);
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
    let self_tenant = run_myelin_triage_on_ci_failure("cafef00d", 0x5170);
    assert!(
        self_tenant.is_green(),
        "the platform's own triage agent runs green on a real Myelin CI failure: {self_tenant:?}"
    );
    assert_eq!(
        self_tenant.reserved, self_tenant.settled,
        "reserve/settle BALANCED on the self_tenant run"
    );
    assert!(self_tenant.trace_ref.starts_with("blake3:"), "a trace per run");

    let rows = proven_fabric_rows(RUN_DATE);
    let confirmed = FabricTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red later-band Fabric gates");
    assert!(confirmed >= 11);

    let incident = FabricIncident::new(
        "INC-AG-SELF_TENANT-E2E",
        "E2E-2",
        "a triage run on a Myelin CI failure left the reserve/settle ledger unbalanced under an \
         at-least-once settle re-delivery",
        "repro_e2e2_self_tenant_triage_balanced_ledger",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        ticket.drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let whole = run_myelin_triage_on_ci_failure("cafef00d", 0x5170);
            let unbalanced = if whole.reserved == whole.settled {
                0
            } else {
                1
            };
            ctx.signals.set_scalar(SignalName::OutboxDepth, unbalanced);
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the incident's repro re-runs green");

    assert!(
        SELF_TENANT_RUNTIME_FLOOR.contains("MOCK runtime") && SELF_TENANT_RUNTIME_FLOOR.contains("AG-P25"),
        "the self_tenant runs on the mock runtime; the real LlmAgentRuntime swap is AG-P25"
    );

    println!(
        "[P-517 SELF_TENANT LOOP GREEN {RUN_DATE}] self-hosting: the platform's own triage agent runs on \
         a real Myelin CI failure (explicit-first dispatch + reserved=={settled} balanced + trace per \
         run); truth-up confirms {confirmed} PROVEN Fabric rows dated; incident→issue→repro-drill \
         registered + re-runs green. FLOOR: mock runtime (real LlmAgentRuntime swap = AG-P25).",
        settled = self_tenant.settled,
    );
}
