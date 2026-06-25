//! # KN-P33 / P-488 — Knowledge's legs of the whole-system E2E wedge (E2E-1 + E2E-3)
//!
//! **Prompt:** P-488 (KN-P33, M5 · KN-M5). **Drills (catalogue 01):** E2E-1 (a Knowledge design-doc
//! embed in the PR context pane resolves per-viewer, 0 leak to the unauthorized viewer — a confidential
//! doc degrades to a tombstone carrying ONLY the root) + E2E-3 (a Knowledge spec doc → initiative →
//! issues lineage over TE-7 typed edges; cold-reindex == live via replay; audit tamper detected).
//!
//! **Owning architecture doc:**
//! `04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md` (the
//! project per-viewer embed + the TE-7 lineage). **Contracts exercised (none new):** 5.6 (the
//! per-viewer `project(ref, viewer)` ladder — the 0-leak crux), 4.3 (the leak-free filter), 2.6
//! (cold-reindex == live), 5.5 (the TE-7 typed-edge lineage). **Doctrine:** EI-01 §4 (drive the WHOLE
//! thing end to end — chain mutations mid-flight, not a single handler), §3 (prove-it: 0 leak; lineage
//! live == cold; the tombstone carries the root; audit tamper detected).
//!
//! ## What this drill proves (both E2E legs, end-to-end, against a full cell with a mock Id resolver)
//! - **E2E-1** — the unauthorized viewer's PR-context-pane embed of a confidential Knowledge design-doc
//!   resolves to a content-free tombstone carrying ONLY the `#sub`-stripped root; the SECRET title is
//!   structurally ABSENT (0 title leak). A mid-flight erasure is honoured by the SAME live read path.
//! - **E2E-3** — the spec→initiative→issues lineage is traceable; a WIPED derived projection
//!   reindexes-from-source to BYTE-MATCH live (cold == live); a hash-chained lineage seal VERIFIES on
//!   the honest chain and DETECTS a tampered/reordered hop (0 silent tamper).
//!
//! No new mutation floor applies — these are E2E integration legs over the UNCHANGED production engine
//! (the project/ladder floor is KN-P19's `refs_glue.rs` ≥90% mutants; the reindex floor is KN-P20). The
//! ONE remaining floor inherited by both legs is the world-scale fleet-hardware 30× load drill
//! (KN-P32). No code fix landed in these legs, so no new unit/CDC floor is owed.

use myelin_knowledge::e2e_wedge::{
    run_e2e1_pr_context_pane, run_e2e3_spec_to_ship_lineage, run_knowledge_e2e_legs, E2E_SCENARIOS,
};

/// **E2E-1 headline (Knowledge leg): the PR context pane resolves per-viewer; 0 title leak.** Drives
/// the whole flow end-to-end (author embed renders live; denied viewer → root-only tombstone; mid-flight
/// erase honoured live). The dated green is the artifact's `is_green()` AND 0 leaks.
#[test]
fn kn_p33_e2e1_pr_context_pane_zero_leak() {
    let art = run_e2e1_pr_context_pane();
    assert_eq!(art.scenario, "E2E-1");
    assert_eq!(
        art.leaks, 0,
        "E2E-1: 0 title leak across every projection — {}",
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
    // The evidence body names the load-bearing facts (per-viewer embed, denied tombstone, mid-flight).
    assert!(art.evidence.contains("denied viewer"));
    assert!(art.evidence.contains("tombstone"));
}

/// **E2E-3 headline (Knowledge leg): spec→initiative→issues lineage; cold==live; tamper detected.**
/// Drives the whole flow end-to-end (lineage traceable; wipe→replay byte-matches live; the hash-chained
/// seal verifies honest and catches a tamper). The dated green is `is_green()` AND 0 divergence/tamper.
#[test]
fn kn_p33_e2e3_spec_to_ship_lineage_cold_equals_live_and_tamper_detected() {
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
    assert!(art.evidence.contains("cold-reindex==live=true"));
    assert!(art.evidence.contains("tamper-detected=true"));
    assert!(art.evidence.contains("lineage traceable=true"));
}

/// **Both legs green and distinctly sealed (the master M5 exit gate's Knowledge rows).** The two
/// scenarios are E2E-1 and E2E-3, both `is_green()`, sealing to DISTINCT content-addresses.
#[test]
fn kn_p33_both_legs_green() {
    let arts = run_knowledge_e2e_legs();
    assert_eq!(arts.len(), 2, "Knowledge crosses two E2E scenarios");
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
        "the two legs seal to distinct citable addresses"
    );
}
