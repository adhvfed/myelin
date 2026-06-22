//! # CDC: the `no-raw-publish` lint over the KNOWLEDGE slice (KN-P06 → P-296, M3)
//!
//! **Contract / doctrine:** `contract-index.md` row 1.6 (the four load-bearing lints) — the Bus's
//! owned `no-raw-publish` slice (EB-07 / P-019; architecture 03 §4: the envelope via the
//! transactional OUTBOX only — no fire-and-forget publish). KN-P06's DEFINITION OF DONE names "the
//! no-raw-publish lint green with fixtures."
//!
//! **Coherence (EI-01 §7 — reconcile, never duplicate).** The lint id, the scanner, the engine, and
//! the central fixtures were shipped CENTRALLY (P-S10 / EB-07). This adds NO new lint id and
//! re-defines NO scanner — it attaches the KNOWLEDGE red+green fixtures + asserts the one central
//! scanner rejects the red and admits the green, exactly mirroring the `refs_lints.rs` precedent.
//! The Knowledge crate's REAL emit seam (`myelin-knowledge/src/emit.rs`) only ever calls
//! `tx.emit(..)`, so the LIVE workspace gate (`myelin-lints/tests/workspace_clean.rs`, which scans
//! `crates/*/src`) is structurally green over Knowledge; these fixtures pin the rule on
//! Knowledge-shaped code.

use myelin_lints::lints::{all_twelve, no_raw_publish};
use myelin_lints::{engine::run, LintId};
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    // The central fixtures live in the lints crate (the one fixture home — no second copy).
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .join("myelin-lints/tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

/// **The KNOWLEDGE red fixture (a fire-and-forget broker publish) is REJECTED by `no-raw-publish`,
/// and by NO OTHER of the twelve lints** — the silent-data-loss / causality-break bug class
/// (KN-D7), caught for the right reason.
#[test]
fn knowledge_red_fixture_is_rejected_only_by_no_raw_publish() {
    let red = read_fixture("no_raw_publish.knowledge.red.rs.txt");
    let violations = no_raw_publish().run(&red);
    assert!(!violations.is_empty(), "the KN red fixture MUST be rejected by no-raw-publish");
    assert!(
        violations.iter().all(|v| v.lint == LintId("no-raw-publish")),
        "every violation carries the no-raw-publish id"
    );

    // Cross-lint isolation: exactly the one lint fires over the whole twelve-set.
    let firing: Vec<LintId> = all_twelve()
        .into_iter()
        .filter(|l| !l.run(&red).is_empty())
        .map(|l| l.id)
        .collect();
    assert_eq!(firing, vec![LintId("no-raw-publish")], "exactly no-raw-publish trips, no other");
}

/// **The KNOWLEDGE green fixture (every `knowledge.*` event emitted via `OutboxTx::emit` in the same
/// transaction as the state change) is ADMITTED by the full twelve-lint set.** No false positive on
/// the sanctioned emit-iff-committed shape.
#[test]
fn knowledge_green_fixture_is_admitted_by_the_full_set() {
    let green = read_fixture("no_raw_publish.knowledge.green.rs.txt");
    assert!(
        no_raw_publish().run(&green).is_empty(),
        "no-raw-publish MUST admit the KN green fixture (the outbox-emit path)"
    );
    assert!(
        run(&all_twelve(), &green).is_ok(),
        "the twelve-lint set must ADMIT the KN green fixture (no lint may false-positive)"
    );
}
