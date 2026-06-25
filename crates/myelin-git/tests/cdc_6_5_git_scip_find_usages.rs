//! Contract 6.5 CDC pair — git's OWNED SCIP/LSIF "find usages" follow-on projection
//! (GIT-P33 / global P-482, M5).
//!
//! Contract 6.5: "Code-search input — Git emits an indexable `git.*` projection per blob/ref/symbol.
//! **Named follow-on:** consume CI-produced SCIP/LSIF for 'find usages'." The lexical floor
//! (`cdc_6_5_git_code_projection_emitter.rs`) shipped the per-blob symbol/literal/trigram projection;
//! GIT-P33 lands the SCIP follow-on on top.
//!
//! - **PROVIDER:** CI (the SCIP index artifact — the language indexers run in the CI sandbox) +
//!   `myelin-git` OWNS what to index (its blobs).
//! - **CONSUMER:** `myelin-git` — [`myelin_git::scip::ScipIndex`] projects the CI-produced occurrences
//!   into find-usages / go-to-definition (Git owns the projection; Search owns the index storage —
//!   the same "Git owns what to index, Search owns the index" boundary the lexical floor honours).
//!
//! The load-bearing contract: find-usages returns ALL references (never the definition), go-to-
//! definition returns THE definition, and the layer is demand-triggered (an un-indexed repo falls back
//! to the lexical floor — no broken find-usages affordance).

use myelin_git::scip::{Occurrence, ScipIndex, ScipSymbol, SymbolRole};

fn sym() -> ScipSymbol {
    ScipSymbol::new("rust cargo myelin-git scip/ScipIndex#")
}

fn ci_index() -> ScipIndex {
    // The CI SCIP artifact: 1 definition + 2 references of the symbol across files.
    ScipIndex::from_ci(vec![
        Occurrence::new(sym(), "src/scip.rs", 120, SymbolRole::Definition),
        Occurrence::new(sym(), "src/lib.rs", 360, SymbolRole::Reference),
        Occurrence::new(
            sym(),
            "tests/cdc_6_5_git_scip_find_usages.rs",
            30,
            SymbolRole::Reference,
        ),
    ])
}

/// **find-usages returns the references, NOT the definition (the AST-precision the lexical floor
/// lacks).** The frozen 6.5 follow-on shape.
#[test]
fn find_usages_returns_references_across_files() {
    let idx = ci_index();
    let usages = idx.find_usages(&sym());
    assert_eq!(usages.len(), 2, "two cross-file references");
    assert!(
        usages.iter().all(|o| !o.is_definition()),
        "references only, never the definition"
    );
}

/// **go-to-definition returns THE definition occurrence.** The defining site, role-distinguished.
#[test]
fn go_to_definition_returns_the_definition() {
    let idx = ci_index();
    let def = idx.definition(&sym()).expect("definition present");
    assert!(def.is_definition());
    assert_eq!(def.path, "src/scip.rs");
}

/// **Demand-triggered: an un-indexed repo falls back to the lexical floor (no broken affordance).** An
/// unavailable index reports `!is_available` and yields no usages — the consumer uses the lexical
/// projection instead.
#[test]
fn demand_triggered_unindexed_repo_falls_back_to_lexical_floor() {
    let idx = ScipIndex::unavailable();
    assert!(
        !idx.is_available(),
        "no CI SCIP index → find-usages unavailable"
    );
    assert!(idx.find_usages(&sym()).is_empty());
    // The available index IS the trigger fact the consumer checks.
    assert!(ci_index().is_available());
}
