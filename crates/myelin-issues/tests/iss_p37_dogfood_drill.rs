//! ISS-P37 → global P-520 (M6) — the dogfood drill: Myelin tracks its OWN issues + the truth-up pass +
//! the every-incident-adds-a-drill loop. THE DONE-BAR for the Issue tracker (roadmap §6 M6-I10).
//!
//! This is the prompt's required end-to-end integration of the Issues dogfood loop, chaining the
//! deliverables (EI-01 §4: chain operations, do not exercise handlers in isolation):
//!
//! 1. **Myelin tracks its own issues** — the team plans its own sprints on the platform's own board /
//!    roadmap: Myelin's own roadmap/gap-report/scorecard live as Myelin issues whose bodies round-trip
//!    through the ONE WASM render path (`render(parse(md)) === md`, ISS-D10); the PR context pane (a
//!    confidential issue's title/count never leaks, 0 leak), the agent-native flagship (a governed close
//!    HITL-gated + exactly-once across a crash), and the spec-to-ship lineage (cold-reindex == live
//!    byte-for-byte) — all green, 0 leak. This REUSES the production Issues surface (the SAME ACL
//!    chokepoint / governance FSM / reindex engine — EI-01 §7, never re-implemented).
//! 2. **The truth-up pass** — every PROVEN Issues row (ISS-D1..ISS-D13 + the E2E slices E2E-1/E2E-2/E2E-3)
//!    rests on a DATED green artifact whose proof SOURCE exists on disk; no later-band Issues gate is red.
//!    A row that names a vanished artifact is surfaced LOUDLY (EI-01 §1, code-wins-over-docs).
//! 3. **The every-incident-adds-a-drill loop** — an Issues incident files a PII-free Myelin issue draft
//!    AND registers a reproducing drill into the harness [`DrillRegistry`] (the T-3 `register_drill`
//!    hook), which then RE-RUNS forever and stays green.
//!
//! It is NOT behind the `integration` feature: the dogfood loop's LOGIC runs in-process over the
//! production Issues surface driven on the modeled self-tenant data. This drill proves the dogfood WIRING
//! and joins the permanent `cargo test` suite (it re-runs on every Myelin commit — wired as a Myelin CI
//! job via the self-hosting CI graph, the `ISS-P37-dogfood` band). **The switch test is the sibling band
//! → the `ISS-P37-switch-test` band.**

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_issues::dogfood::{
    proven_issues_rows, run_issues_over_myelins_own_work, run_issues_truth_up_scorecard,
    IssuesIncident, IssuesTruthUpPass,
};

/// A dated run stamp (the dogfood CI run's date). Pinned so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// **(1) THE HEADLINE: Issues runs GREEN on Myelin's OWN work.** Myelin's own issues round-trip through
/// the ONE WASM render path; the PR context pane, the agent-native flagship (exactly-once close), and the
/// spec-to-ship lineage all green over the Myelin self-tenant, 0 leak — the production-hardened surface
/// exercised on the platform's own work.
#[test]
fn myelin_tracks_its_own_issues() {
    let artifact = run_issues_over_myelins_own_work(RUN_DATE);

    assert!(
        artifact.is_green(),
        "Issues must be green on Myelin's own work: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.issues_round_tripped,
        artifact.issues_total,
        "every one of Myelin's own issues round-trips through the ONE WASM render path: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.total_leaks(),
        0,
        "0 leak across the three E2E faces: {}",
        artifact.summary()
    );

    let line = artifact.summary();
    assert!(
        line.contains("P-520 ISSUES DOGFOOD 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    assert!(
        line.contains("tenant=myelin") && line.contains("region=fr-par"),
        "self-tenant framing: {line}"
    );
    println!("{line}");
}

/// **(2) The truth-up pass is GREEN.** Every PROVEN Issues row (ISS-D1..ISS-D13 + the E2E slices) rests on
/// a DATED green artifact whose proof SOURCE exists on disk — no later-band Issues gate is red (the gate
/// invariant holds end-to-end). A vanished/undated row is surfaced LOUDLY, never trusted on faith.
#[test]
fn the_truth_up_pass_is_green_with_proof_sources_on_disk() {
    let rows = proven_issues_rows(RUN_DATE);
    assert!(
        rows.len() >= 16,
        "the PROVEN set covers ISS-D1..ISS-D13 + the E2E slices E2E-1/E2E-2/E2E-3"
    );
    // every PROVEN row dated.
    let confirmed = IssuesTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red later-band Issues gates — every PROVEN row dated");
    assert_eq!(confirmed, rows.len());

    // every proof source exists on disk — the scorecard renders GREEN.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let scorecard = run_issues_truth_up_scorecard(RUN_DATE, &repo_root);
    assert!(
        scorecard.is_green(),
        "the truth-up scorecard must be green; claimed-not-proven: {:?}",
        scorecard.claimed_not_proven()
    );
    let md = scorecard.render();
    assert!(
        md.contains("verdict=GREEN") && md.contains("ISS-D1") && md.contains("E2E-3"),
        "the rendered scorecard: {md}"
    );
    print!("{md}");
}

/// **(3) The every-incident loop joins the permanent drill suite + RE-RUNS green forever.** An Issues
/// incident files a PII-free Myelin issue draft + a reproducing-drill ticket, and the repro is registered
/// into the harness [`DrillRegistry`] (the T-3 `register_drill` hook) and driven twice green — a
/// regression would re-red it loudly. This is the dogfood loop's guarantee: it re-runs on every commit.
#[test]
fn the_every_incident_loop_joins_the_permanent_suite_and_re_runs_green() {
    let incident = IssuesIncident::new(
        "INC-ISS-DOGFOOD-1",
        "ISS-D10",
        "an issue-body corpus fixture silently round-tripped non-canonically on the Myelin self-tenant",
        "repro_iss_d10_dogfood_non_canonical_round_trip",
    );
    let draft = incident.issue_draft();
    assert!(
        draft
            .body
            .contains("repro_iss_d10_dogfood_non_canonical_round_trip"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );
    // PII-free: the draft carries no personal data, only opaque ids + gate names.
    assert!(!draft.body.to_lowercase().contains("email"));

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        incident.drill_ticket().drill_name,
        move |ctx: &mut DrillContext| {
            // The reproducing scenario: re-drive the Issues dogfood faces and assert all-green (a
            // regression re-reds this — a leak, a non-round-tripping body, or a broken lineage).
            let artifact = run_issues_over_myelins_own_work(RUN_DATE);
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

/// **The dogfood spine end-to-end (EI-01 §4: chain the operations).** The full ISS-P37 dogfood spine in
/// one chained run: Myelin tracks its own issues (all green, 0 leak) → the truth-up pass confirms every
/// PROVEN Issues row is dated (0 red later-band gate) → the every-incident repro joins the suite and
/// re-runs green. THE DONE-BAR for the Issue tracker, held on the platform's own work.
#[test]
fn dogfood_spine_end_to_end() {
    // (1) Myelin tracks its own issues → all green, 0 leak.
    let artifact = run_issues_over_myelins_own_work(RUN_DATE);
    assert!(
        artifact.is_green(),
        "Issues is green on Myelin's own work: {}",
        artifact.summary()
    );

    // (2) the truth-up pass → every PROVEN Issues row dated (0 red later-band Issues gate).
    let rows = proven_issues_rows(RUN_DATE);
    IssuesTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("the truth-up pass is green — every PROVEN Issues row dated");

    // (3) the every-incident repro joins the suite + re-runs green.
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "iss_p37_dogfood_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let whole = run_issues_over_myelins_own_work(RUN_DATE).is_green();
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

    println!(
        "[P-520 ISSUES DOGFOOD GREEN {RUN_DATE}] Myelin tracks its own issues: own work as Myelin \
         issues (round-trip) + the PR context pane + the agent-native flagship (exactly-once close) + \
         the spec-to-ship lineage all green, 0 leak; the truth-up pass confirms 0 red later-band Issues \
         gate (ISS-D1..D13 + the E2E slices, every row dated); the every-incident-adds-a-drill loop is \
         self-hosted"
    );
}
