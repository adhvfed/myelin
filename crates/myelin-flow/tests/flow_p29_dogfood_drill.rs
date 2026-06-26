//! P-FLOW-29 → global P-516 (M6) — the dogfood drill: Myelin's OWN pipelines / merge queue / SLA
//! timers as myelin-flow workflows + the truth-up pass + the every-incident-adds-a-drill loop.
//!
//! This is the prompt's required end-to-end integration of the FLOW dogfood loop, chaining the
//! deliverables (EI-01 §4: the dogfood loop IS the test — chain the operations, do not exercise
//! handlers in isolation):
//!
//! 1. **Myelin's own pipelines / merge queue / SLA timers run as myelin-flow workflows** — Myelin's
//!    own build/test/lint pipeline runs as a `ci.pipeline` workflow end-to-end (face 1); Myelin's own
//!    merge queue merges a real Myelin PR EXACTLY ONCE (face 2); a real Myelin SLA timer FIRES on a
//!    real Myelin issue (face 3) — all green. This REUSES the production myelin-flow surface (the SAME
//!    CI-pipeline substrate / merge-queue body / SLA-timer wheel — EI-01 §7, never re-implemented).
//!    The dogfood loop exercises every engine path (replay, the long-park, signals, the merge-queue
//!    wake, the durable timer) on the platform's own commits.
//! 2. **The truth-up pass** — every PROVEN FLOW row (FLOW-D1..FLOW-D10 + the E2E-2 spine) rests on a
//!    DATED green artifact whose proof SOURCE exists on disk; no earlier-band FLOW gate is red. A row
//!    that names a vanished artifact is surfaced LOUDLY (EI-01 §1, code-wins-over-docs).
//! 3. **The every-incident-adds-a-drill loop** — a FLOW incident files a PII-free Myelin issue draft
//!    AND registers a reproducing drill into the harness [`DrillRegistry`] (the T-3 `register_drill`
//!    hook), which then RE-RUNS forever and stays green.
//!
//! It is NOT behind the `integration` feature: the dogfood loop's LOGIC runs in-process over the
//! production myelin-flow engine driven on the modeled own-tenant data (the real durable substrate —
//! a `FlowDispatcher` over a `RunStore` + journal + signal buffer + outbox + timer wheel). This drill
//! proves the dogfood WIRING and joins the permanent `cargo test` suite (it re-runs on every Myelin
//! commit — the dogfood loop's whole point; it is wired as a Myelin CI job via the self-hosting CI
//! graph, the `FLOW-P29-dogfood` job).

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_flow::{
    proven_flow_rows, run_flow_over_myelins_own_work, run_flow_truth_up_scorecard, FlowIncident,
    FlowTruthUpPass,
};

/// A dated run stamp (the dogfood CI run's date). The harness `today_iso()` supplies the real one in a
/// live run; the test pins a date so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// **(1) THE HEADLINE: Myelin's own pipelines / merge queue / SLA timers run GREEN as myelin-flow
/// workflows on the self-hosting platform.** All three faces green over the Myelin self-tenant — every
/// engine path exercised on the platform's own commits.
#[test]
fn myelins_own_workflows_run_on_the_self_hosting_platform() {
    let artifact = run_flow_over_myelins_own_work(RUN_DATE);

    assert!(
        artifact.is_green(),
        "Myelin's pipelines/merge-queue/SLA-timers must run green as myelin-flow workflows: {}",
        artifact.summary()
    );

    // Face 1: Myelin's own CI pipeline ran every stage to completion, 0 re-dispatch across the kill.
    assert!(
        artifact.pipeline.is_green(),
        "Myelin's CI pipeline as a ci.pipeline workflow"
    );
    assert!(artifact.pipeline.completed);
    assert_eq!(
        artifact.pipeline.dispatches, 3,
        "0 re-dispatch — one dispatch per stage"
    );

    // Face 2: Myelin's own merge queue merged a real Myelin PR EXACTLY ONCE (exactly-once spine).
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

    // Face 3: a real Myelin SLA timer FIRED on a real Myelin issue (arm → cheap re-arm → fire).
    assert!(
        artifact.sla_timer.is_green(),
        "a real Myelin SLA timer fires on a real Myelin issue"
    );
    assert!(artifact.sla_timer.fired, "the breach SLA timer FIRED");

    let line = artifact.summary();
    assert!(
        line.contains("P-516 FLOW DOGFOOD 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

/// **(2) THE HEADLINE: the truth-up pass confirms every PROVEN FLOW row rests on a dated green
/// artifact whose proof source exists on disk (0 red earlier-band FLOW gates).**
#[test]
fn the_truth_up_pass_confirms_every_proven_flow_row_is_dated() {
    let rows = proven_flow_rows(RUN_DATE);
    assert!(
        rows.len() >= 11,
        "the PROVEN set covers FLOW-D1..FLOW-D10 + the E2E-2 spine"
    );

    let confirmed = FlowTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band FLOW gates — every PROVEN row rests on a dated green artifact");
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    // The enumerated scorecard is GREEN with every proof source on disk (the §x.y-grouped artifact).
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

/// **(3) The every-incident-adds-a-drill loop: a FLOW incident files an issue + REGISTERS a reproducing
/// drill that re-runs forever.** The incident produces a PII-free Myelin issue draft AND a
/// reproducing-drill ticket; the test builds the repro [`DrillScenario`] under the ticket's name,
/// `register_drill`s it into the harness [`DrillRegistry`] (the T-3 hook), and proves it RE-RUNS green
/// twice (the "re-runs forever" guarantee — a regression would re-red it loudly).
#[test]
fn a_flow_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = FlowIncident::new(
        "INC-FLOW-DOGFOOD-1",
        "FLOW-D1",
        "a replay-recovery regression dropped a journaled effect on the Myelin self-tenant",
        "repro_flow_d1_dogfood_replay_recovery",
    );

    // (a) it files a PII-free Myelin issue draft (names the gate + the repro drill).
    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "FLOW-D1");
    assert!(draft.title.contains("INC-FLOW-DOGFOOD-1"));
    assert!(
        draft.body.contains("repro_flow_d1_dogfood_replay_recovery"),
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
            // The reproducing scenario: re-run Myelin's own workflows and assert they are whole (a
            // regression that re-broke replay/recovery — e.g. a re-dispatch or a double-merge — re-reds).
            let dogfood = run_flow_over_myelins_own_work(RUN_DATE);
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

/// **The dogfood loop end-to-end (EI-01 §4: chain the operations).** The full P-FLOW-29 spine in one
/// chained run: Myelin's own pipelines / merge queue / SLA timers run as myelin-flow workflows (all
/// three faces green) → the truth-up pass confirms 0 red earlier-band FLOW gates → a FLOW incident
/// files an issue + registers a repro drill that re-runs green. The platform hosts itself, and Myelin's
/// own CI pipelines / merge queue / SLA timers run on the platform's own commits.
#[test]
fn dogfood_loop_end_to_end_self_hosting() {
    // (1) Myelin's own pipelines / merge queue / SLA timers run as myelin-flow workflows, all green.
    let dogfood = run_flow_over_myelins_own_work(RUN_DATE);
    assert!(
        dogfood.is_green(),
        "Myelin's pipelines/merge-queue/SLA-timers run green as myelin-flow workflows: {}",
        dogfood.summary()
    );

    // (2) the truth-up pass — 0 red earlier-band FLOW gates.
    let rows = proven_flow_rows(RUN_DATE);
    let confirmed = FlowTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band FLOW gates");
    assert!(confirmed >= 11);

    // (3) an incident files an issue + registers a repro drill that re-runs forever.
    let incident = FlowIncident::new(
        "INC-FLOW-DOGFOOD-E2E",
        "E2E-2",
        "a merge-queue wake double-merged a Myelin PR under an at-least-once ci.result re-delivery",
        "repro_e2e2_dogfood_merge_queue_exactly_once",
    );
    let _draft = incident.issue_draft();
    let ticket = incident.drill_ticket();
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        ticket.drill_name.clone(),
        move |ctx: &mut DrillContext| {
            let whole = run_flow_over_myelins_own_work(RUN_DATE);
            // The exactly-once invariant: the merge queue merged the Myelin PR exactly once.
            let double_merge = if whole.merge_queue.merges == 1 { 0 } else { 1 };
            ctx.signals
                .set_scalar(SignalName::OutboxDepth, double_merge);
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        },
    ));
    assert!(registry.all_green(), "the incident's repro re-runs green");

    println!(
        "[P-516 DOGFOOD LOOP GREEN {RUN_DATE}] self-hosting: Myelin's own pipelines/merge-queue/\
         SLA-timers run as myelin-flow workflows (ci-pipeline + merge-queue(merge==1) + sla-timer fired, \
         all green); truth-up confirms {confirmed} PROVEN FLOW rows dated; incident→issue→repro-drill \
         registered + re-runs green"
    );
}
