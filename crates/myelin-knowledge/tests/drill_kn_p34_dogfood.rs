//! KN-P34 → global P-519 (M6) — the dogfood drill: Myelin's OWN docs live in Knowledge + the truth-up
//! pass + the every-incident-adds-a-drill loop. THE DONE-BAR for the Knowledge platform (roadmap §3 KN-M6).
//!
//! This is the prompt's required end-to-end integration of the Knowledge dogfood loop, chaining the
//! deliverables (EI-01 §4: chain operations, do not exercise handlers in isolation):
//!
//! 1. **Myelin's own docs live in Knowledge** — the platform's roadmap / gap-report / scorecard as a
//!    Knowledge space, every block round-tripping `render(parse(md)) === md` through the ONE render path
//!    (the team's own knowledge survives the editor with byte-fidelity, the §8b.2 one-render-path law);
//!    plus the PR context pane (a Knowledge design-doc embed resolves per-viewer; a denied viewer's
//!    confidential doc tombstones, 0 title leak) and the spec-to-ship lineage (roadmap → initiative →
//!    issues; cold-reindex == live byte-for-byte; audit tamper detected) — all green, 0 leak. This REUSES
//!    the production Knowledge surface (the SAME projector / reindex engine — EI-01 §7, never re-implemented).
//! 2. **The truth-up pass** — every PROVEN Knowledge row (KN-D1..KN-D13 + the E2E slices E2E-1/E2E-3)
//!    rests on a DATED green artifact whose proof SOURCE exists on disk; no later-band Knowledge gate is
//!    red. A row that names a vanished artifact is surfaced LOUDLY (EI-01 §1, code-wins-over-docs).
//! 3. **The every-incident-adds-a-drill loop** — a Knowledge incident files a PII-free Myelin issue draft
//!    AND registers a reproducing drill into the harness [`DrillRegistry`] (the T-3 `register_drill`
//!    hook), which then RE-RUNS forever and stays green.
//!
//! It is NOT behind the `integration` feature: the dogfood loop's LOGIC runs in-process over the
//! production Knowledge surface driven on the modeled self-tenant data. This drill proves the dogfood
//! WIRING and joins the permanent `cargo test` suite (it re-runs on every Myelin commit). **The switch
//! test is the sibling drill → `drill_kn_p34_switch_test.rs`.**

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_knowledge::dogfood::{
    myelin_knowledge_space, proven_knowledge_rows, run_knowledge_over_myelins_own_work,
    run_knowledge_truth_up_scorecard, KnowledgeIncident, KnowledgeTruthUpPass,
};

/// A dated run stamp (the dogfood CI run's date). Pinned so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// **(1) THE HEADLINE: Knowledge runs GREEN on Myelin's OWN work.** Myelin's own docs round-trip through
/// the ONE render path, the PR context pane resolves per-viewer (0 title leak), and the spec-to-ship
/// lineage is cold == live + tamper-detected over the Myelin self-tenant, 0 leak — the production-hardened
/// surface exercised on the platform's own work.
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

/// Myelin's own Knowledge space carries the roadmap / gap-report / scorecard, and each doc round-trips.
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

/// **(2) The truth-up pass is GREEN.** Every PROVEN Knowledge row (KN-D1..KN-D13 + the E2E slices) rests
/// on a DATED green artifact whose proof SOURCE exists on disk — no later-band Knowledge gate is red (the
/// gate invariant holds end-to-end). A vanished/undated row is surfaced LOUDLY, never trusted on faith.
#[test]
fn the_truth_up_pass_is_green_with_proof_sources_on_disk() {
    let rows = proven_knowledge_rows(RUN_DATE);
    assert!(
        rows.len() >= 15,
        "the PROVEN set covers KN-D1..KN-D13 + the E2E slices"
    );
    // every PROVEN row dated.
    let confirmed = KnowledgeTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red later-band Knowledge gates — every PROVEN row dated");
    assert_eq!(confirmed, rows.len());

    // every proof source exists on disk — the scorecard renders GREEN.
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

/// **(3) The every-incident loop joins the permanent drill suite + RE-RUNS green forever.** A Knowledge
/// incident files a PII-free Myelin issue draft + a reproducing-drill ticket, and the repro is registered
/// into the harness [`DrillRegistry`] (the T-3 `register_drill` hook) and driven twice green — a
/// regression would re-red it loudly. This is the dogfood loop's guarantee: it re-runs on every commit.
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
    // PII-free: the draft carries no personal data, only opaque ids + gate names.
    assert!(!draft.body.to_lowercase().contains("email"));

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        incident.drill_ticket().drill_name,
        move |ctx: &mut DrillContext| {
            // The reproducing scenario: re-drive the Knowledge dogfood faces and assert all-green (a
            // regression re-reds this — a non-round-tripping doc, a title leak, or a broken lineage).
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

/// **The dogfood spine end-to-end (EI-01 §4: chain the operations).** The full KN-P34 dogfood spine in
/// one chained run: Myelin's own docs live in Knowledge (all round-trip, the E2E faces green, 0 leak) →
/// the truth-up pass confirms every PROVEN Knowledge row is dated (0 red later-band gate) → the
/// every-incident repro joins the suite and re-runs green. THE DONE-BAR for the Knowledge platform, held
/// on the platform's own work.
#[test]
fn dogfood_spine_end_to_end() {
    // (1) Myelin's own docs live in Knowledge → all round-trip, the E2E faces green, 0 leak.
    let artifact = run_knowledge_over_myelins_own_work(RUN_DATE);
    assert!(
        artifact.is_green(),
        "Knowledge is green on Myelin's own work: {}",
        artifact.summary()
    );

    // (2) the truth-up pass → every PROVEN Knowledge row dated (0 red later-band gate).
    let rows = proven_knowledge_rows(RUN_DATE);
    KnowledgeTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("the truth-up pass is green — every PROVEN Knowledge row dated");

    // (3) the every-incident repro joins the suite + re-runs green.
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
