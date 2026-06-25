//! REF-P28 → global P-513 (M6) — the dogfood drill: the reference graph over Myelin's OWN work +
//! the truth-up pass + the every-incident-adds-a-drill loop.
//!
//! This is the prompt's required end-to-end integration of the Refs dogfood loop, chaining the
//! deliverables (EI-01 §4: chain operations, do not exercise handlers in isolation):
//!
//! 1. **The reference graph runs over Myelin's own work** — the PR context pane on the Myelin
//!    monorepo's PRs (commits ↔ issues ↔ CI checks ↔ KN docs ↔ chat threads), the spec-to-ship lineage
//!    on Myelin's roadmap/scorecard living as Myelin issues + a Myelin Knowledge space, and the
//!    structural-erasure holder fan-out over a Myelin team member's own data — all green, 0 leak. This
//!    REUSES the production Refs surface (the SAME resolve chokepoint / traverse / reindex / holder
//!    engine — EI-01 §7, never re-implemented).
//! 2. **The truth-up pass** — every PROVEN Refs row (REF-D1..REF-D10 + the E2E legs) rests on a DATED
//!    green artifact whose proof SOURCE exists on disk; no earlier-band Refs gate is red. A row that
//!    names a vanished artifact is surfaced LOUDLY (EI-01 §1, code-wins-over-docs).
//! 3. **The every-incident-adds-a-drill loop** — a Refs incident files a PII-free Myelin issue draft
//!    AND registers a reproducing drill into the harness [`DrillRegistry`] (the T-3 `register_drill`
//!    hook), which then RE-RUNS forever and stays green.
//!
//! It is NOT behind the `integration` feature: the dogfood loop's LOGIC runs in-process over the
//! production Refs engine driven on the modeled own-tenant data. This drill proves the dogfood WIRING
//! and joins the permanent `cargo test` suite (it re-runs on every Myelin commit — the dogfood loop's
//! whole point; it is wired as a Myelin CI job via the self-hosting CI graph).
//!
//! **The switch-test browser drive over the Refs surfaces is the named floor → REF-P29.**

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_refs_service::{
    proven_refs_rows, run_refs_over_myelins_own_work, run_refs_truth_up_scorecard, RefsIncident,
    RefsTruthUpPass,
};

/// A dated run stamp (the dogfood CI run's date). The harness `today_iso()` supplies the real one in a
/// live run; the test pins a date so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// **(1) THE HEADLINE: the reference graph runs GREEN on Myelin's OWN work.** The PR context pane, the
/// spec-to-ship lineage, and the structural-erasure holder fan-out all green over the Myelin
/// self-tenant, 0 leak across the three faces — the moat thesis exercised on the platform's own work.
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
        line.contains("P-513 REFS DOGFOOD 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    println!("{line}");
}

/// **(2) THE HEADLINE: the truth-up pass confirms every PROVEN Refs row rests on a dated green
/// artifact whose proof source exists on disk (0 red earlier-band Refs gates).**
#[test]
fn the_truth_up_pass_confirms_every_proven_refs_row_is_dated() {
    let rows = proven_refs_rows(RUN_DATE);
    assert!(
        rows.len() >= 13,
        "the PROVEN set covers REF-D1..REF-D10 + the E2E legs (E2E-1/E2E-3/E2E-4)"
    );

    let confirmed = RefsTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band Refs gates — every PROVEN row rests on a dated green artifact");
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    // The enumerated scorecard is GREEN with every proof source on disk (the §5.x-grouped artifact).
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

/// **(3) The every-incident-adds-a-drill loop: a Refs incident files an issue + REGISTERS a reproducing
/// drill that re-runs forever.** The incident produces a PII-free Myelin issue draft AND a
/// reproducing-drill ticket; the test builds the repro [`DrillScenario`] under the ticket's name,
/// `register_drill`s it into the harness [`DrillRegistry`] (the T-3 hook), and proves it RE-RUNS green
/// twice (the "re-runs forever" guarantee — a regression would re-red it loudly).
#[test]
fn a_refs_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = RefsIncident::new(
        "INC-REFS-DOGFOOD-1",
        "REF-D1",
        "a resolve chokepoint regression leaked a denied issue title on the Myelin self-tenant",
        "repro_ref_d1_dogfood_resolve_leak",
    );

    // (a) it files a PII-free Myelin issue draft (names the gate + the repro drill).
    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "REF-D1");
    assert!(draft.title.contains("INC-REFS-DOGFOOD-1"));
    assert!(
        draft.body.contains("repro_ref_d1_dogfood_resolve_leak"),
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
            // The reproducing scenario: re-run the reference graph over Myelin's own work and assert it
            // is whole (0 leak — a regression that re-broke the resolve chokepoint would re-red this).
            let dogfood = run_refs_over_myelins_own_work(RUN_DATE);
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

/// **The dogfood loop end-to-end (EI-01 §4: chain the operations).** The full REF-P28 spine in one
/// chained run: the reference graph runs over Myelin's own work (all three faces green, 0 leak) → the
/// truth-up pass confirms 0 red earlier-band Refs gates → a Refs incident files an issue + registers a
/// repro drill that re-runs green. The platform hosts itself, and the reference graph runs on the
/// platform's own commits.
#[test]
fn dogfood_loop_end_to_end_self_hosting() {
    // (1) the reference graph runs over Myelin's own work → all three faces green, 0 leak.
    let dogfood = run_refs_over_myelins_own_work(RUN_DATE);
    assert!(
        dogfood.is_green() && dogfood.total_leaks() == 0,
        "the reference graph is green on Myelin's own work: {}",
        dogfood.summary()
    );

    // (2) the truth-up pass — 0 red earlier-band Refs gates.
    let rows = proven_refs_rows(RUN_DATE);
    let confirmed = RefsTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band Refs gates");
    assert!(confirmed >= 13);

    // (3) an incident files an issue + registers a repro drill that re-runs forever.
    let incident = RefsIncident::new(
        "INC-REFS-DOGFOOD-E2E",
        "E2E-3",
        "a spec-to-ship lineage traverse dropped a reindex-parity node under a doc-edit surge",
        "repro_e2e3_dogfood_lineage_reindex_parity",
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
        "[P-513 DOGFOOD LOOP GREEN {RUN_DATE}] self-hosting: the reference graph runs on Myelin's own \
         work (PR-pane + spec-to-ship + holder-fanout green, 0 leak); truth-up confirms {confirmed} \
         PROVEN Refs rows dated; incident→issue→repro-drill registered + re-runs green"
    );
}
