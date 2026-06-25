//! # Drill — SRCH-P32 (P-465): the whole-system E2E wedge Search crosses (E2E-1 / E2E-3 / E2E-4)
//!
//! **The completion of S-M5 (the master M5→M6 boundary's Search rows).** This drives the Search side of
//! the three whole-system chained-mutation E2E scenarios over the PUBLIC `myelin_search` surface (the
//! [`run_search_e2e_wedge`] driver + the per-scenario entries) and asserts each emits its dated green
//! artifact. It is the integration-level proof that the engine the M5 prompts hardened composes into the
//! three whole-system rows: the leak-free per-viewer hit (E2E-1), the cold==live reindex (E2E-3), and
//! the 0-recoverable DSAR erase incl. backups (E2E-4).
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §2 — E2E-1 (the PR
//! context pane, SRCH-D1 in-context), E2E-3 (spec-to-ship / reindex-parity, SRCH-D5 at scale), E2E-4
//! (the DSAR fan-out, SRCH-D4 at backup scale). **Architecture:** `search-and-indexing.md` §4.2 (the
//! pre-filter — E2E-1), §4.8 (erase — E2E-4), §4.9 (reindex-from-source — E2E-3).
//!
//! ## Floor named
//! These are the M5 E2E wedge rows — the named single-cell / erase / reindex follow-ons proven
//! end-to-end. Run at the CI variant (a moderate corpus, not the world-scale fleet corpus); the
//! world-scale 30× fleet load is the ONLY remaining floor (SRCH-P25/P29). The SRCH-P09 leak mutation
//! floor + the SRCH-P15 erase mutation floor hold (unchanged) — this drill re-drives those exact paths.

use myelin_search::{
    run_e2e_1_pr_pane, run_e2e_3_spec_to_ship, run_e2e_4_dsar_fanout, run_search_e2e_wedge,
    E2eArtifact, E2E_SCENARIOS,
};

/// **The whole Search-side E2E wedge is GREEN (the master M5 exit gate citation).** All three scenarios
/// drive end-to-end over the production-hardened engine; each emits its dated green artifact at 0 leak /
/// 0 recoverable. A red E2E-1 must NOT let M6 start — the gate is loud.
#[test]
fn srch_p32_whole_search_e2e_wedge_is_green() {
    let arts: Vec<E2eArtifact> = run_search_e2e_wedge();
    assert_eq!(
        arts.len(),
        3,
        "the three rows Search crosses: E2E-1 / E2E-3 / E2E-4"
    );
    let scenarios: Vec<&str> = arts.iter().map(|a| a.scenario).collect();
    assert_eq!(scenarios, E2E_SCENARIOS);
    for a in &arts {
        assert!(
            a.is_green(),
            "{} must be green (the master M5 exit gate cites it): {}",
            a.scenario,
            a.evidence
        );
        // The dated green-artifact line (observability is part of the pass, EI-01 §3).
        println!(
            "[P-465 SRCH-P32 {} GREEN 2026-06-25] {}",
            a.scenario, a.evidence
        );
    }
}

/// **E2E-1 (Search row): a hit on a confidential issue resolves to a tombstone — 0 title/count leak.**
#[test]
fn srch_p32_e2e_1_pr_pane_zero_leak() {
    let a = run_e2e_1_pr_pane();
    assert!(a.is_green(), "E2E-1: {}", a.evidence);
    assert_eq!(
        a.leaks, 0,
        "0 doc/count/IDF/RAG/title leak (the §4.2 pre-filter)"
    );
    assert!(a.evidence.contains("tombstone"));
    assert!(a.evidence.contains("title_absent=true"));
}

/// **E2E-3 (Search row): the wiped index reindexes to byte-match live (SRCH-D5 at scale).**
#[test]
fn srch_p32_e2e_3_reindex_byte_match() {
    let a = run_e2e_3_spec_to_ship();
    assert!(a.is_green(), "E2E-3: {}", a.evidence);
    assert!(
        a.evidence.contains("byte_match=true"),
        "cold-reindex == live: {}",
        a.evidence
    );
    assert!(a.evidence.contains("restore-verify green=true"));
}

/// **E2E-4 (Search row): Search's docs + embeddings return 0 recoverable PII incl. vectors incl.
/// backups; the holder-coverage receipt includes Search (H7).**
#[test]
fn srch_p32_e2e_4_dsar_zero_recoverable_including_backups() {
    let a = run_e2e_4_dsar_fanout();
    assert!(a.is_green(), "E2E-4: {}", a.evidence);
    assert_eq!(
        a.leaks, 0,
        "0 recoverable PII incl. vectors incl. backups (GA-D1 spine)"
    );
    assert!(
        a.evidence.contains("recoverable 3→0"),
        "0 recoverable after the shred: {}",
        a.evidence
    );
    assert!(
        a.evidence.contains("is_h7=true"),
        "the receipt includes Search H7: {}",
        a.evidence
    );
}
