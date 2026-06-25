//! # Drill — GIT-P34 (P-483): git's slices of the whole-system E2E scenarios (E2E-1 / E2E-2 / E2E-3)
//!
//! **The world-scale-hardening E2E half of GIT-P34.** This drives the Git side of the three whole-system
//! chained-mutation E2E scenarios over the PUBLIC `myelin_git::surge` surface (the [`run_git_e2e_wedge`]
//! driver + the per-scenario entries) and asserts each emits its dated green artifact. It is the
//! integration-level proof that the engine the M3/M5 prompts hardened composes into the three whole-system
//! rows: the leak-free per-viewer PR-context reference producer (E2E-1), the agent-native flagship's
//! exactly-once HITL + merge + the X-1 CheckStatus gate + `git.pr.merged` closing the issue (E2E-2), and
//! the commit→PR→merge lineage with cold-reindex == live (E2E-3).
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §2 — E2E-1 (the PR
//! context pane, git is the PR host + reference producer), E2E-2 (the agent-native flagship — git hosts
//! the fix-PR; the `git.merge` HITL approval + the X-1/GIT-D10 gate + `git.pr.merged` closing the issue),
//! E2E-3 (spec-to-ship / the commit→PR→merge lineage, cold-reindex == live). **Architecture:**
//! `git-hosting/architecture/02-internals-and-algorithms.md` §6.2 (the merge gate — E2E-2) + GIT-P19 (the
//! `Closes`-trailer lifecycle edge — E2E-1/E2E-3). **Contract:** rows 5.9 (the X-1 CheckStatus gate) + 5.5
//! (the lifecycle edge).
//!
//! ## Floor named
//! These are the M5 E2E wedge rows — proven end-to-end over git's production-hardened engine at the CI
//! variant (a deterministic in-memory composition, not the world-scale fleet corpus). The world-scale 30×
//! fleet load is the ONLY remaining floor (the shared §4.1 fleet drill). The git-owned mutation floors
//! (the merge-gate GIT-P21, the `Closes`-trailer GIT-P19) hold unchanged — this drill re-drives those
//! exact paths in their E2E context.

use myelin_git::surge::{
    run_e2e_1_pr_pane, run_e2e_2_fix_pr, run_e2e_3_spec_to_ship, run_git_e2e_wedge, E2eArtifact,
    GIT_E2E_SCENARIOS,
};

/// **The whole git-side E2E wedge is GREEN (the master M5 exit gate citation).** All three scenarios drive
/// end-to-end over the production-hardened engine; each emits its dated green artifact at 0 leak /
/// exactly-once merge. A red E2E-2 (the flagship) must NOT let M6 start — the gate is loud.
#[test]
fn git_p34_whole_git_e2e_wedge_is_green() {
    let arts: Vec<E2eArtifact> = run_git_e2e_wedge();
    assert_eq!(
        arts.len(),
        3,
        "the three rows git crosses: E2E-1 / E2E-2 / E2E-3"
    );
    let scenarios: Vec<&str> = arts.iter().map(|a| a.scenario).collect();
    assert_eq!(scenarios, GIT_E2E_SCENARIOS);
    for a in &arts {
        assert!(
            a.is_green(),
            "{} must be green (the master M5 exit gate cites it): {}",
            a.scenario,
            a.evidence
        );
        println!(
            "[P-483 GIT-P34 {} GREEN 2026-06-25] {}",
            a.scenario, a.evidence
        );
    }
}

/// **E2E-1 (git slice): the PR is the reference producer; an unauthorized viewer leaks 0.**
#[test]
fn git_p34_e2e_1_pr_pane_zero_leak() {
    let a = run_e2e_1_pr_pane();
    assert!(a.is_green(), "E2E-1: {}", a.evidence);
    assert_eq!(a.leaks, 0, "zero leak to the unauthorized viewer");
}

/// **E2E-2 (git slice, the flagship): the `git.merge` HITL gate blocks before green, the X-1 CheckStatus
/// gate holds, the merge applies EXACTLY ONCE across the kill, and `git.pr.merged` closes the issue.**
#[test]
fn git_p34_e2e_2_flagship_exactly_once_hitl_and_merge() {
    let a = run_e2e_2_fix_pr();
    assert!(a.is_green(), "E2E-2 flagship: {}", a.evidence);
    assert_eq!(
        a.merge_count, 1,
        "exactly-once merge across the kill the durable workflow rode (FLOW-D1)"
    );
    assert_eq!(a.leaks, 0);
}

/// **E2E-3 (git slice): the commit→PR→merge lineage reconstructs byte-for-byte from cold (== live).**
#[test]
fn git_p34_e2e_3_spec_to_ship_cold_equals_live() {
    let a = run_e2e_3_spec_to_ship();
    assert!(a.is_green(), "E2E-3: {}", a.evidence);
}
