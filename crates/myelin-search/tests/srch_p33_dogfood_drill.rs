//! SRCH-P33 → global P-515 (M6) — the dogfood drill: Search over Myelin's OWN work + the truth-up
//! pass + the every-incident-adds-a-drill loop.
//!
//! This is the prompt's required end-to-end integration of the Search dogfood loop, chaining the
//! deliverables (EI-01 §4: chain operations, do not exercise handlers in isolation):
//!
//! 1. **Search runs over Myelin's own work** — code + issue search on the Myelin monorepo (per-viewer
//!    leak-free hits, the confidential issue tombstones, 0 doc/count/IDF/RAG leak), search over Myelin's
//!    own Knowledge space (the roadmap/scorecard as a Knowledge space, reindex-from-source byte-parity),
//!    and the DSAR fan-out over a team member's own data (Search's docs + EMBEDDINGS return 0 recoverable
//!    PII incl. vectors incl. backups, the holder-coverage receipt includes Search H7) — all green, 0
//!    leak. This REUSES the production Search surface (the SAME permission-aware query pre-filter /
//!    reindex-from-source / structural-erase engine — EI-01 §7, never re-implemented).
//! 2. **The truth-up pass** — every PROVEN Search row (SRCH-D1..SRCH-D10 + the E2E legs E2E-1/E2E-3/
//!    E2E-4) rests on a DATED green artifact whose proof SOURCE exists on disk; no earlier-band Search
//!    gate is red. A row that names a vanished artifact is surfaced LOUDLY (EI-01 §1, code-wins-over-docs).
//! 3. **The every-incident-adds-a-drill loop** — a Search incident files a PII-free Myelin issue draft
//!    AND registers a reproducing drill into the harness [`DrillRegistry`] (the T-3 `register_drill`
//!    hook), which then RE-RUNS forever and stays green.
//!
//! It is NOT behind the `integration` feature: the dogfood loop's LOGIC runs in-process over the
//! production Search engine driven on the modeled own-tenant data. This drill proves the dogfood WIRING
//! and joins the permanent `cargo test` suite (it re-runs on every Myelin commit — the dogfood loop's
//! whole point; it is wired as a Myelin CI job via the self-hosting CI graph, the `SRCH-P33-dogfood`
//! band).
//!
//! **Embedding-adapter posture (recorded honestly):** the doc-by-content faces run on the
//! `MockEmbeddingAdapter` — the real EU-hostable embedding adapter is the named post-M5/runtime config
//! swap (`EMBEDDING_ADAPTER_POSTURE`), never a rewrite. **The switch test is the sibling band → the
//! `SRCH-P33-switch-test` band.**

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_search::{
    proven_search_rows, run_search_over_myelins_own_work, run_search_truth_up_scorecard,
    SearchIncident, SearchTruthUpPass, EMBEDDING_ADAPTER_POSTURE,
};

/// A dated run stamp (the dogfood CI run's date). Pinned so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// **(1) THE HEADLINE: Search runs GREEN on Myelin's OWN work.** Code + issue search, the
/// Knowledge-space reindex-parity, and the DSAR fan-out all green over the Myelin self-tenant, 0 leak
/// across the three faces — the production-hardened engine exercised on the platform's own work.
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
    // The embedding-adapter posture is recorded honestly (mock; real adapter is the named follow-on).
    assert!(
        line.contains("embedding-adapter=mock") && EMBEDDING_ADAPTER_POSTURE.contains("mock"),
        "the embedding-adapter posture is recorded honestly: {line}"
    );
    println!("{line}");
}

/// **(2) THE HEADLINE: the truth-up pass confirms every PROVEN Search row rests on a dated green
/// artifact whose proof source exists on disk (0 red earlier-band Search gates).**
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
            "0 red earlier-band Search gates — every PROVEN row rests on a dated green artifact",
        );
    assert_eq!(confirmed, rows.len(), "every PROVEN row confirmed dated");

    // The enumerated scorecard is GREEN with every proof source on disk (the section-grouped artifact).
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

/// **(3) The every-incident-adds-a-drill loop: a Search incident files an issue + REGISTERS a
/// reproducing drill that re-runs forever.** The incident produces a PII-free Myelin issue draft AND a
/// reproducing-drill ticket; the test builds the repro [`DrillScenario`] under the ticket's name,
/// `register_drill`s it into the harness [`DrillRegistry`] (the T-3 hook), and proves it RE-RUNS green
/// twice (the "re-runs forever" guarantee — a regression would re-red it loudly).
#[test]
fn a_search_incident_files_an_issue_and_joins_the_permanent_drill_suite() {
    let incident = SearchIncident::new(
        "INC-SEARCH-DOGFOOD-1",
        "SRCH-D1",
        "a pre-filter regression let a confidential issue enter the candidate set on the Myelin self-tenant",
        "repro_srch_d1_dogfood_candidate_leak",
    );

    // (a) it files a PII-free Myelin issue draft (names the gate + the repro drill).
    let draft = incident.issue_draft();
    assert_eq!(draft.gate_id, "SRCH-D1");
    assert!(draft.title.contains("INC-SEARCH-DOGFOOD-1"));
    assert!(
        draft.body.contains("repro_srch_d1_dogfood_candidate_leak"),
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
            // The reproducing scenario: re-run Search over Myelin's own work and assert it is whole
            // (0 leak — a regression that re-broke the §4.2 pre-filter would re-red this).
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

/// **The dogfood loop end-to-end (EI-01 §4: chain the operations).** The full SRCH-P33 spine in one
/// chained run: Search runs over Myelin's own work (all three faces green, 0 leak) → the truth-up pass
/// confirms 0 red earlier-band Search gates → a Search incident files an issue + registers a repro drill
/// that re-runs green. The platform hosts itself, and Search runs on the platform's own commits.
#[test]
fn dogfood_loop_end_to_end_self_hosting() {
    // (1) Search runs over Myelin's own work → all three faces green, 0 leak.
    let dogfood = run_search_over_myelins_own_work(RUN_DATE);
    assert!(
        dogfood.is_green() && dogfood.total_leaks() == 0,
        "Search is green on Myelin's own work: {}",
        dogfood.summary()
    );

    // (2) the truth-up pass — 0 red earlier-band Search gates.
    let rows = proven_search_rows(RUN_DATE);
    let confirmed = SearchTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red earlier-band Search gates");
    assert!(confirmed >= 13);

    // (3) an incident files an issue + registers a repro drill that re-runs forever.
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
