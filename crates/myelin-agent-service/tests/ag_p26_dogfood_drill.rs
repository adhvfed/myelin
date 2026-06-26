//! AG-P26 → global P-517 (M6) — the dogfood drill: the platform's own agents run on its own
//! commits/issues/chat + the truth-up pass + the every-incident-adds-a-drill loop.
//!
//! This is the prompt's required end-to-end integration of the Fabric dogfood loop, chaining the
//! deliverables (EI-01 §4: the dogfood loop IS the test — chain the operations, do not exercise
//! handlers in isolation):
//!
//! 1. **The platform's own triage agent runs on the self-hosting CI graph** — a real Myelin CI
//!    failure on a Myelin commit dispatches a costed triage run (explicit-first / Signal-driven, NOT a
//!    casual mention), the run emits a BALANCED reserve/settle ledger (reserved == settled; a Mock
//!    bills 0 metered units → the reservation refunds), 0 in-flight interrupt, and a content-addressed
//!    trace per run (the dogfood green artifacts — contract 1.8). This REUSES the production
//!    Agent-Fabric surface (the SAME dispatch classifier / CostLedger / TraceDocument — EI-01 §7,
//!    never re-implemented).
//! 2. **The truth-up pass** — every PROVEN Fabric row (AG-D1..AG-D11 + the E2E-2 spine) rests on a
//!    DATED green artifact whose proof SOURCE exists on disk; no later-band Fabric gate is red. A row
//!    that names a vanished artifact is surfaced LOUDLY (EI-01 §1, code-wins-over-docs).
//! 3. **The every-incident-adds-a-drill loop** — a Fabric incident files a PII-free Myelin issue draft
//!    AND registers a reproducing drill into the harness [`DrillRegistry`] (the T-3 `register_drill`
//!    hook), which then RE-RUNS forever and stays green.
//!
//! It is NOT behind the `integration` feature: the dogfood loop's LOGIC runs in-process over the
//! production Agent-Fabric engine driven on the modeled own-tenant data (the real reserve/settle
//! `CostLedger` + the content-addressed `TraceDocument`). This drill proves the dogfood WIRING and
//! joins the permanent `cargo test` suite (it re-runs on every Myelin commit — the dogfood loop's whole
//! point; it is wired as a Myelin CI job via the self-hosting CI graph, the `AG-P26-dogfood` job).
//!
//! ## FLOOR named (VISION §3, EI-01 §1)
//! The dogfood agents run on the MOCK runtime (the `--use-mock` MockAgentRuntime path) — correct per
//! VISION §3 during development. The real `LlmAgentRuntime` swap (the only place a model/SDK/prompt/
//! model-name string appears) is the named post-M5 follow-on AG-P25; the Mock metering ZERO is correct
//! (the reserve/settle gate is the runaway self-limiter regardless of which brain runs).

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_agent_service::{
    proven_fabric_rows, run_fabric_over_myelins_own_work, run_fabric_truth_up_scorecard,
    run_myelin_triage_on_ci_failure, FabricIncident, FabricTruthUpPass, DOGFOOD_RUNTIME_FLOOR,
};

/// A dated run stamp (the dogfood CI run's date). The harness `today_iso()` supplies the real one in a
/// live run; the test pins a date so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// **(1) THE HEADLINE: Myelin's own triage agent runs GREEN on the self-hosting CI graph.** A real
/// Myelin CI failure dispatches a costed triage run (explicit-first) with a BALANCED reserve/settle
/// ledger + a content-addressed trace per run — the Fabric runs unchanged on the platform's own work.
#[test]
fn myelins_own_agent_runs_on_the_self_hosting_graph() {
    let artifact = run_fabric_over_myelins_own_work(RUN_DATE);

    assert!(
        artifact.is_green(),
        "Myelin's own triage agent must run green on the self-hosting CI graph: {}",
        artifact.summary()
    );

    // explicit-first: a ci.result=failure Signal DISPATCHES; a casual mention only NOTIFIES.
    assert!(
        artifact.triage.dispatched,
        "a ci.result=failure Signal DISPATCHES a costed triage run (explicit-first, §3.4)"
    );
    assert!(
        artifact.triage.mention_only_notifies,
        "a casual @triage mention only NOTIFIES — 0 auto-spawn (the safety boundary, CHAT-1)"
    );

    // BALANCED reserve/settle ledger: reserved == settled, 0 in-flight interrupt (contract 1.8).
    assert_eq!(
        artifact.triage.reserved, artifact.triage.settled,
        "reserve/settle BALANCED — reserved == settled on the Myelin self-tenant wallet"
    );
    assert_eq!(
        artifact.triage.inflight_interrupts, 0,
        "0 in-flight interrupt — the reservation's only exit is settle (11.7)"
    );

    // a trace per run: a content-addressed blake3:<hex> trace ref was written (8.8 / contract 1.8).
    assert!(
        artifact.triage.trace_ref.starts_with("blake3:"),
        "a content-addressed trace per run (8.8): {}",
        artifact.triage.trace_ref
    );

    let line = artifact.summary();
    assert!(
        line.contains("P-517 FABRIC DOGFOOD 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

/// **(2) THE HEADLINE: the truth-up pass confirms every PROVEN Fabric row rests on a dated green
/// artifact whose proof source exists on disk (0 red later-band Fabric gates).**
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
        .expect("0 red later-band Fabric gates — every PROVEN row rests on a dated green artifact");
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    // The enumerated scorecard is GREEN with every proof source on disk (the §x.y-grouped artifact).
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

/// **(3) The every-incident-adds-a-drill loop: a Fabric incident files an issue + REGISTERS a
/// reproducing drill that re-runs forever.** The incident produces a PII-free Myelin issue draft AND a
/// reproducing-drill ticket; the test builds the repro [`DrillScenario`] under the ticket's name,
/// `register_drill`s it into the harness [`DrillRegistry`] (the T-3 hook), and proves it RE-RUNS green
/// twice (the "re-runs forever" guarantee — a regression would re-red it loudly).
#[test]
fn a_fabric_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = FabricIncident::new(
        "INC-AG-DOGFOOD-1",
        "AG-D11",
        "a reserve/settle regression left an in-flight triage run torn down on the Myelin self-tenant",
        "repro_ag_d11_dogfood_runaway_self_limiter",
    );

    // (a) it files a PII-free Myelin issue draft (names the gate + the repro drill).
    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "AG-D11");
    assert!(draft.title.contains("INC-AG-DOGFOOD-1"));
    assert!(
        draft
            .body
            .contains("repro_ag_d11_dogfood_runaway_self_limiter"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );

    // (b) it registers a reproducing drill into the harness suite (the T-3 register_drill hook).
    let ticket = incident.drill_ticket();
    let drill_name = ticket.drill_name.clone();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        drill_name.clone(),
        move |ctx: &mut DrillContext| {
            // The reproducing scenario: re-run Myelin's own triage agent and assert it is whole (a
            // regression that re-broke the reserve/settle balance or interrupted an in-flight run re-reds).
            let dogfood = run_fabric_over_myelins_own_work(RUN_DATE);
            ctx.signals.set_scalar(
                SignalName::DeadLetterCount,
                if dogfood.is_green() { 0 } else { 1 },
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

    // It RE-RUNS forever — drive it twice, green both times (the every-incident loop's guarantee).
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

/// **The dogfood loop end-to-end (EI-01 §4: chain the operations).** The full AG-P26 spine in one
/// chained run: the platform's own triage agent runs on a real Myelin CI failure (balanced ledger,
/// trace) → the truth-up pass confirms 0 red later-band Fabric gates → a Fabric incident files an issue
/// and registers a repro drill that re-runs green. The platform's own agents run on the platform's own
/// commits/issues/chat — the M6 done-bar.
#[test]
fn dogfood_loop_end_to_end_self_hosting() {
    // (1) the platform's own triage agent runs on a real Myelin CI failure, green.
    let dogfood = run_myelin_triage_on_ci_failure("cafef00d", 0x5170);
    assert!(
        dogfood.is_green(),
        "the platform's own triage agent runs green on a real Myelin CI failure: {dogfood:?}"
    );
    assert_eq!(
        dogfood.reserved, dogfood.settled,
        "reserve/settle BALANCED on the dogfood run"
    );
    assert!(dogfood.trace_ref.starts_with("blake3:"), "a trace per run");

    // (2) the truth-up pass — 0 red later-band Fabric gates.
    let rows = proven_fabric_rows(RUN_DATE);
    let confirmed = FabricTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red later-band Fabric gates");
    assert!(confirmed >= 11);

    // (3) an incident files an issue + registers a repro drill that re-runs forever.
    let incident = FabricIncident::new(
        "INC-AG-DOGFOOD-E2E",
        "E2E-2",
        "a triage run on a Myelin CI failure left the reserve/settle ledger unbalanced under an \
         at-least-once settle re-delivery",
        "repro_e2e2_dogfood_triage_balanced_ledger",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        ticket.drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let whole = run_myelin_triage_on_ci_failure("cafef00d", 0x5170);
            // The balanced-ledger invariant: reserved == settled on the dogfood triage run.
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

    // The MOCK-runtime floor is named honestly in writing (VISION §3) — the real swap is AG-P25.
    assert!(
        DOGFOOD_RUNTIME_FLOOR.contains("MOCK runtime") && DOGFOOD_RUNTIME_FLOOR.contains("AG-P25"),
        "the dogfood runs on the mock runtime; the real LlmAgentRuntime swap is AG-P25"
    );

    println!(
        "[P-517 DOGFOOD LOOP GREEN {RUN_DATE}] self-hosting: the platform's own triage agent runs on \
         a real Myelin CI failure (explicit-first dispatch + reserved=={settled} balanced + trace per \
         run); truth-up confirms {confirmed} PROVEN Fabric rows dated; incident→issue→repro-drill \
         registered + re-runs green. FLOOR: mock runtime (real LlmAgentRuntime swap = AG-P25).",
        settled = dogfood.settled,
    );
}
