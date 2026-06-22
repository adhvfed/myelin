//! SRCH-P01 → P-021: the Search-owned red+green confirmation of the `search-requires-acl-filter`
//! lint (contract 1.6, the permanent ratchet gate).
//!
//! **Reconciliation (coherence rule, EI-01 §7).** The lint, its engine, and its red+green
//! fixtures were FIRST shipped by the substrate prompt P-S11 → P-018 (the remaining eight
//! architecture lints; the lint harness is shared substrate). The P-018 source explicitly names
//! this prompt as the OWNER of the same contract-1.6 row ("the Search subsystem also ships its OWN
//! `search-requires-acl-filter` twin (SRCH-P01 / P-021)"). SRCH-P01 therefore CONFIRMS the lint in
//! place — its home stays `myelin_lints::lints::search_requires_acl_filter`, matrix-wired in
//! `myelin-lints/tests/fixture_matrix.rs` and CI-wired via the `lint-gate` binary — rather than
//! duplicating a second scanner. This file is the Search subsystem's own dated proof that the gate
//! REJECTS the bypass path and ADMITS the composed path.
//!
//! **Why this proof lives in `tests/` and not in `src/lib.rs`.** The red sample below is a verbatim
//! bypass fingerprint (`index.search(query)` with no composed ACL filter). The LIVE workspace lint
//! scan (`myelin-lints/tests/workspace_clean.rs` + the `lint-gate` binary) scans `crates/*/src` and
//! EXCLUDES `crates/*/tests/**` (a documented, by-design exclusion — test fixtures legitimately
//! carry red samples). Keeping the bypass sample here means the very gate SRCH-P01 ships stays GREEN
//! over Search's own source, while the red+green verdicts are still proven. (The shared lints crate
//! keeps its own red samples in `tests/fixtures/` for the identical reason.)

use myelin_lints::lints::search_requires_acl_filter;

/// **SRCH-P01 GATE artifact — the red verdict (2026-06-19).** The lint REJECTS an unfiltered
/// `engine.search` path: a search executed WITHOUT conjoining the `list_objects` ACL `Filter`
/// before scoring. Scoring before filtering leaks the existence and rank of forbidden documents
/// (ADR-03 / OQ-E) — the post-filter leak bug class.
#[test]
fn the_lint_rejects_an_unfiltered_search_path() {
    let lint = search_requires_acl_filter();
    // A search with no ACL-filter binder on the statement — the bypass fingerprint.
    let red = "let hits = index.search(query).await?;";
    let violations = lint.run(red);
    assert!(
        !violations.is_empty(),
        "search-requires-acl-filter MUST reject an unfiltered engine.search path (the \
         post-filter existence/rank leak)"
    );
    assert!(
        violations
            .iter()
            .all(|v| v.lint.0 == "search-requires-acl-filter"),
        "every violation must carry the search-requires-acl-filter id (not a false attribution)"
    );
}

/// **SRCH-P01 GATE artifact — the green verdict (2026-06-19).** The lint ADMITS a path that
/// conjoins the ACL `Filter` BEFORE scoring (pre-filter) — the only public query entry shape. No
/// false reject: a correctly composed path passes.
#[test]
fn the_lint_admits_a_pre_filtered_search_path() {
    let lint = search_requires_acl_filter();
    // The ACL Filter is conjoined first (`.with_acl(acl_filter)`) — pre-filter, the composed entry.
    let green = "let hits = index.search(query.with_acl(acl_filter)).await?;";
    assert!(
        lint.run(green).is_empty(),
        "search-requires-acl-filter MUST admit a path that pre-filters with the composed ACL \
         Filter (0 false reject)"
    );
}
