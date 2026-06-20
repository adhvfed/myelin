//! SRCH-P16 / P-179: the `no-cross-db` GATE for the reindex-from-source path (the SEARCH-1 ratchet).
//!
//! The prompt's GATE: *"The no-cross-db lint green: reindex re-drives the indexer, never reads an owner
//! DB; there is no 'load the index from Postgres' path — CI (permanent ratchet)."*
//!
//! **Reconciliation (EI-01 §7 — reuse, never duplicate).** The `no-cross-db` lint, its scanner, the
//! `lint-gate` CI binary, and the central fixtures were shipped centrally by the substrate prompt
//! P-S10 → P-017. This file adds NO new lint id and NO parallel scanner; it REUSES the in-place
//! `myelin_lints::lints::no_cross_db` scanner and proves it (a) admits the REAL Search reindex source
//! (`crates/myelin-search/src/reindex.rs` — green: the rebuild re-drives the live indexer, no owner-DB
//! reach) and (b) rejects the "load the index from Postgres" backdoor fingerprint the SEARCH-1
//! anti-pattern names (red). The live workspace `lint-gate` scans `crates/*/src` and is the permanent
//! ratchet; this is the Search-owned dated proof for the reindex path specifically.

use myelin_lints::lints::no_cross_db;

/// **GREEN: the REAL reindex-from-source source is admitted by `no-cross-db`.** The
/// `crates/myelin-search/src/reindex.rs` module re-drives the live indexer's `index()` step from the bus
/// re-emit seam — it contains NO `myelin_<owner>::{storage|store|db|schema|repo|pool}` reach (the
/// no-cross-db floor is structural). The lint admits it (0 violations).
#[test]
fn the_real_reindex_source_is_admitted_no_owner_db_reach() {
    let src = include_str!("../src/reindex.rs");
    let violations = no_cross_db().run(src);
    assert!(
        violations.is_empty(),
        "no-cross-db MUST admit the reindex-from-source path (it re-drives the live indexer, never \
         reads an owner DB) — found: {violations:?}"
    );
}

/// **RED: a fabricated 'load the index from Postgres' backdoor is rejected by `no-cross-db`.** The
/// SEARCH-1 anti-pattern is reading a sibling owner's store directly instead of re-driving the live
/// consumer. The lint's fingerprint catches a reach into another subsystem's storage module.
#[test]
fn a_load_from_postgres_backdoor_is_rejected() {
    // The bypass fingerprint: a reindex that "loads the index from Postgres" by reaching into an
    // owner's storage module instead of re-driving the live consumer (the SEARCH-1 anti-pattern).
    let red = "use myelin_knowledge::storage::PageRepo; // load the index from Postgres (backdoor)";
    let violations = no_cross_db().run(red);
    assert!(
        !violations.is_empty(),
        "no-cross-db MUST reject a 'load the index from Postgres' backdoor (a reach into an owner's \
         storage module) — the SEARCH-1 anti-pattern"
    );
    assert!(
        violations.iter().all(|v| v.lint.0 == "no-cross-db"),
        "every violation must carry the no-cross-db id"
    );
}
