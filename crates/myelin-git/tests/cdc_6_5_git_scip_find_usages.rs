use myelin_git::scip::{Occurrence, ScipIndex, ScipSymbol, SymbolRole};

fn sym() -> ScipSymbol {
    ScipSymbol::new("rust cargo myelin-git scip/ScipIndex#")
}

fn ci_index() -> ScipIndex {
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

#[test]
fn go_to_definition_returns_the_definition() {
    let idx = ci_index();
    let def = idx.definition(&sym()).expect("definition present");
    assert!(def.is_definition());
    assert_eq!(def.path, "src/scip.rs");
}

#[test]
fn demand_triggered_unindexed_repo_falls_back_to_lexical_floor() {
    let idx = ScipIndex::unavailable();
    assert!(
        !idx.is_available(),
        "no CI SCIP index → find-usages unavailable"
    );
    assert!(idx.find_usages(&sym()).is_empty());
    assert!(ci_index().is_available());
}
