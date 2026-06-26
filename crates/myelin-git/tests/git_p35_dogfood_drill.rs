//! GIT-P35 → global P-518 (M6) — the dogfood drill: git hosts Myelin's OWN repositories + the truth-up
//! pass + the every-incident-adds-a-drill loop. THE DONE-BAR for git hosting (roadmap §6).
//!
//! This is the prompt's required end-to-end integration of the git dogfood loop, chaining the
//! deliverables (EI-01 §4: chain operations, do not exercise handlers in isolation):
//!
//! 1. **git hosts Myelin's own repositories** — the PR context pane on the Myelin monorepo (git the
//!    reference producer; a denied viewer's linked confidential issue tombstones, 0 leak), the
//!    agent-native fix-PR flagship (CI-fail → fix-PR; the `git.merge` HITL + X-1 CheckStatus gate;
//!    exactly-once HITL + merge across the kill; `git.pr.merged` closes the issue), and the spec-to-ship
//!    lineage (commit→PR→merge; cold-reindex == live byte-for-byte) — all green, 0 leak, merge-count == 1.
//!    This REUSES the production git surface (the SAME reference-producer / merge-gate / reindex engine —
//!    EI-01 §7, never re-implemented).
//! 2. **The truth-up pass** — every PROVEN git row (GIT-D1..GIT-D11 + the E2E slices E2E-1/E2E-2/E2E-3)
//!    rests on a DATED green artifact whose proof SOURCE exists on disk; no later-band git gate is red. A
//!    row that names a vanished artifact is surfaced LOUDLY (EI-01 §1, code-wins-over-docs).
//! 3. **The every-incident-adds-a-drill loop** — a git incident files a PII-free Myelin issue draft AND
//!    registers a reproducing drill into the harness [`DrillRegistry`] (the T-3 `register_drill` hook),
//!    which then RE-RUNS forever and stays green.
//!
//! It is NOT behind the `integration` feature: the dogfood loop's LOGIC runs in-process over the
//! production git surface driven on the modeled self-tenant data. This drill proves the dogfood WIRING
//! and joins the permanent `cargo test` suite (it re-runs on every Myelin commit — wired as a Myelin CI
//! job via the self-hosting CI graph, the `GIT-P35-dogfood` band). **The switch test is the sibling band
//! → the `GIT-P35-switch-test` band.**

use myelin_harness::telemetry::{Predicate, SignalName};
use myelin_harness::{DrillContext, DrillRegistry, DrillScenario};

use myelin_git::dogfood::{
    proven_git_rows, run_git_over_myelins_own_repos, run_git_truth_up_scorecard, GitIncident,
    GitTruthUpPass,
};

/// A dated run stamp (the dogfood CI run's date). Pinned so the artifact is reproducible.
const RUN_DATE: &str = "2026-06-26";

/// **(1) THE HEADLINE: git runs GREEN on Myelin's OWN repositories.** The PR context pane, the
/// agent-native fix-PR flagship (exactly-once merge), and the spec-to-ship lineage all green over the
/// Myelin self-tenant, 0 leak — the production-hardened surface exercised on the platform's own work.
#[test]
fn git_hosts_myelins_own_repositories() {
    let artifact = run_git_over_myelins_own_repos(RUN_DATE);

    assert!(
        artifact.is_green(),
        "git must be green on Myelin's own repositories: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.total_leaks(),
        0,
        "0 leak across the three faces: {}",
        artifact.summary()
    );
    assert_eq!(
        artifact.fix_pr_flagship.merge_count,
        1,
        "the agent-native flagship merge is exactly-once: {}",
        artifact.summary()
    );

    let line = artifact.summary();
    assert!(
        line.contains("P-518 GIT DOGFOOD 2026-06-26") && line.contains("verdict=GREEN"),
        "dated artifact: {line}"
    );
    assert!(
        line.contains("tenant=myelin") && line.contains("region=fr-par"),
        "self-tenant framing: {line}"
    );
    println!("{line}");
}

/// **(2) The truth-up pass is GREEN.** Every PROVEN git row (GIT-D1..GIT-D11 + the E2E slices) rests on a
/// DATED green artifact whose proof SOURCE exists on disk — no later-band git gate is red (the gate
/// invariant holds end-to-end). A vanished/undated row is surfaced LOUDLY, never trusted on faith.
#[test]
fn the_truth_up_pass_is_green_with_proof_sources_on_disk() {
    let rows = proven_git_rows(RUN_DATE);
    assert!(
        rows.len() >= 14,
        "the PROVEN set covers GIT-D1..GIT-D11 + the E2E slices"
    );
    // every PROVEN row dated.
    let confirmed = GitTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("0 red later-band git gates — every PROVEN row dated");
    assert_eq!(confirmed, rows.len());

    // every proof source exists on disk — the scorecard renders GREEN.
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let scorecard = run_git_truth_up_scorecard(RUN_DATE, &repo_root);
    assert!(
        scorecard.is_green(),
        "the truth-up scorecard must be green; claimed-not-proven: {:?}",
        scorecard.claimed_not_proven()
    );
    let md = scorecard.render();
    assert!(
        md.contains("verdict=GREEN") && md.contains("GIT-D1") && md.contains("E2E-3"),
        "the rendered scorecard: {md}"
    );
    print!("{md}");
}

/// **(3) The every-incident loop joins the permanent drill suite + RE-RUNS green forever.** A git
/// incident files a PII-free Myelin issue draft + a reproducing-drill ticket, and the repro is registered
/// into the harness [`DrillRegistry`] (the T-3 `register_drill` hook) and driven twice green — a
/// regression would re-red it loudly. This is the dogfood loop's guarantee: it re-runs on every commit.
#[test]
fn the_every_incident_loop_joins_the_permanent_suite_and_re_runs_green() {
    let incident = GitIncident::new(
        "INC-GIT-DOGFOOD-1",
        "GIT-D9",
        "a receive-pack regression left a ghost ref without its outbox event on the Myelin self-tenant",
        "repro_git_d9_dogfood_ghost_ref",
    );
    let draft = incident.issue_draft();
    assert!(
        draft.body.contains("repro_git_d9_dogfood_ghost_ref"),
        "the issue is reference-linked to its repro drill: {}",
        draft.body
    );
    // PII-free: the draft carries no personal data, only opaque ids + gate names.
    assert!(!draft.body.to_lowercase().contains("email"));

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        incident.drill_ticket().drill_name,
        move |ctx: &mut DrillContext| {
            // The reproducing scenario: re-drive the git dogfood faces and assert all-green (a regression
            // re-reds this — a leak, a missed merge, or a broken lineage).
            let artifact = run_git_over_myelins_own_repos(RUN_DATE);
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

/// **The dogfood spine end-to-end (EI-01 §4: chain the operations).** The full GIT-P35 dogfood spine in
/// one chained run: git hosts Myelin's own repos (all green, 0 leak, exactly-once merge) → the truth-up
/// pass confirms every PROVEN git row is dated (0 red later-band gate) → the every-incident repro joins
/// the suite and re-runs green. THE DONE-BAR for git hosting, held on the platform's own work.
#[test]
fn dogfood_spine_end_to_end() {
    // (1) git hosts Myelin's own repositories → all green, 0 leak, merge-count == 1.
    let artifact = run_git_over_myelins_own_repos(RUN_DATE);
    assert!(
        artifact.is_green(),
        "git is green on Myelin's own repositories: {}",
        artifact.summary()
    );

    // (2) the truth-up pass → every PROVEN git row dated (0 red later-band git gate).
    let rows = proven_git_rows(RUN_DATE);
    GitTruthUpPass::new()
        .run_or_fail_ci(&rows, RUN_DATE)
        .expect("the truth-up pass is green — every PROVEN git row dated");

    // (3) the every-incident repro joins the suite + re-runs green.
    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "git_p35_dogfood_spine".to_string(),
        move |ctx: &mut DrillContext| {
            let whole = run_git_over_myelins_own_repos(RUN_DATE).is_green();
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
        "[P-518 GIT DOGFOOD GREEN {RUN_DATE}] the Myelin monorepo hosted on Myelin git: the PR context \
         pane + the agent-native fix-PR flagship (exactly-once merge) + the spec-to-ship lineage all \
         green, 0 leak; the truth-up pass confirms 0 red later-band git gate (GIT-D1..D11 + the E2E \
         slices, every row dated); the every-incident-adds-a-drill loop is self-hosted"
    );
}
