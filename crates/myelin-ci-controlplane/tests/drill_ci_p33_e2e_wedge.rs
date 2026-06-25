//! # CI-P33 / P-493 — CI's slices of the whole-system E2E wedge (E2E-1 + E2E-3)
//!
//! **Prompt:** P-493 (CI-P33, M5 · CI-M5). **Drills (catalogue 01):** E2E-1 (CI's slice — the PR
//! context pane: CI emits `ci.check.updated`, the pane resolves CI's check rows per-viewer, 0 leak to
//! the unauthorized viewer, the `#step-<n>` jump-to-failure anchor resolves) + E2E-3 (CI's slice —
//! spec-to-ship traceability: a CI run attaches `CheckStatus`, a protected-env deploy ships HITL-gated,
//! cold-reindex (replay) == live, audit tamper detected).
//!
//! **Owning architecture doc:**
//! `04-subsystem-architectures/continuous-integration/architecture/03-events-contracts-and-glue.md`
//! §7.2 (the project context-pane / unfurl) + `04-views-cli-and-api.md` §1 (the cross-subsystem
//! surfaces CI feeds). **Contracts exercised (none new):** 5.9 (the check seam end-to-end), 5.6
//! (project — the per-viewer pane/unfurl, the 0-leak crux), 2.6 (replay — cold-reindex == live).
//! **Doctrine:** EI-01 §4 (drive the WHOLE thing end to end — chain mutations mid-flight, not a single
//! handler), §3 (prove-it: 0 leak; lineage live == cold; tombstone content-free; audit tamper
//! detected).
//!
//! ## What this drill proves (both E2E slices, end-to-end, against a full cell with a mock Id resolver)
//! - **E2E-1** — CI emits `ci.check.updated` (build→success, test→failure) carrying the `#step-<n>`
//!   jump-to-failure anchor; the unauthorized viewer's PR-context-pane embed of CI's run row resolves
//!   to a content-free tombstone (the SECRET pipeline name + state structurally ABSENT — 0 leak); a
//!   mid-flight erasure is honoured by the SAME live read path.
//! - **E2E-3** — the spec→issue→PR→run→deploy lineage is traceable; the protected-env deploy ships
//!   HITL-gated (decline withholds 0 mutation; approve ships EXACTLY once across a double-click); a
//!   WIPED derived projection reindexes-from-source to BYTE-MATCH live (cold == live); a hash-chained
//!   lineage seal VERIFIES on the honest chain and DETECTS a tampered/reordered hop (0 silent tamper).
//!
//! No new mutation floor applies — these are E2E integration slices over the UNCHANGED production CI
//! engine (the project/ladder floor is CI-P25's `surfacing.rs`; the reindex floor is CI-P26; the deploy
//! HITL floor is CI-P24's `deployment.rs`). The ONE remaining floor inherited by both slices is the
//! world-scale fleet-hardware 30× load drill (CI-P30). **E2E-2 (the agent-native flagship) is CI-P34;
//! E2E-4 (the DSAR fan-out) is covered for CI by CI-P32's CI-D3 (erasure-reaches-every-holder).** No
//! code fix landed in these slices, so no new unit/CDC floor is owed.

use myelin_ci_controlplane::e2e_wedge::{
    run_ci_e2e_slices, run_e2e1_pr_context_pane, run_e2e3_spec_to_ship_lineage, E2E_SCENARIOS,
};

/// **E2E-1 headline (CI's slice): the PR context pane resolves CI's check rows per-viewer; 0 leak.**
/// Drives the whole flow end-to-end (ci.check.updated emitted with the `#step-<n>` anchor; collaborator
/// run-row renders live with merge blocked; denied viewer → content-free tombstone; mid-flight erase
/// honoured live). The dated green is the artifact's `is_green()` AND 0 leaks.
#[test]
fn ci_p33_e2e1_pr_context_pane_zero_leak() {
    let art = run_e2e1_pr_context_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert_eq!(
        art.leaks, 0,
        "E2E-1: 0 row leak across every projection — {}",
        art.evidence
    );
    assert!(
        art.is_green(),
        "E2E-1 green not earned (the dated artifact): {} [seal={}]",
        art.evidence,
        art.seal
    );
    // The artifact is a citable content-address (the master M5 exit gate cites it by hash).
    assert!(art.seal.starts_with("blake3:"), "the artifact is sealed");
    // The evidence body names the load-bearing facts (the check seam, the anchor, the tombstone).
    assert!(art.evidence.contains("ci.check.updated"));
    assert!(art.evidence.contains("#step-"));
    assert!(art.evidence.contains("tombstone"));
    assert!(art.evidence.contains("merge blocked"));
}

/// **E2E-3 headline (CI's slice): CheckStatus attaches; HITL-gated deploy ships; cold==live; tamper.**
/// Drives the whole flow end-to-end (lineage traceable; decline withholds + approve ships exactly once;
/// wipe→replay byte-matches live; the hash-chained seal verifies honest and catches a tamper). The
/// dated green is `is_green()` AND 0 divergence/tamper.
#[test]
fn ci_p33_e2e3_spec_to_ship_lineage_cold_equals_live_and_tamper_detected() {
    let art = run_e2e3_spec_to_ship_lineage();
    assert_eq!(art.scenario, "E2E-3");
    assert_eq!(
        art.leaks, 0,
        "E2E-3: 0 cold/live divergence + 0 undetected tamper — {}",
        art.evidence
    );
    assert!(
        art.is_green(),
        "E2E-3 green not earned (the dated artifact): {} [seal={}]",
        art.evidence,
        art.seal
    );
    assert!(art.seal.starts_with("blake3:"), "the artifact is sealed");
    assert!(art.evidence.contains("lineage traceable=true"));
    assert!(art.evidence.contains("approve-ships-exactly-once=true"));
    assert!(art.evidence.contains("cold-reindex==live=true"));
    assert!(art.evidence.contains("tamper-detected=true"));
}

/// **Both slices green and distinctly sealed (the master M5 exit gate's CI rows).** The two scenarios
/// are E2E-1 and E2E-3, both `is_green()`, sealing to DISTINCT content-addresses. (E2E-2 is CI-P34;
/// E2E-4 is CI-P32's CI-D3 — not owned here.)
#[test]
fn ci_p33_both_slices_green() {
    let arts = run_ci_e2e_slices();
    assert_eq!(arts.len(), 2, "CI's slice crosses two E2E scenarios");
    assert_eq!(E2E_SCENARIOS, ["E2E-1", "E2E-3"]);
    for art in &arts {
        assert!(
            art.is_green(),
            "{} not green: {} [seal={}]",
            art.scenario,
            art.evidence,
            art.seal
        );
    }
    assert_ne!(
        arts[0].seal, arts[1].seal,
        "the two slices seal to distinct citable addresses"
    );
}
