use myelin_lints::lints::no_cross_db;

#[test]
fn the_real_reindex_source_is_admitted_no_owner_db_reach() {
    let src = include_str!("../src/reindex.rs");
    let violations = no_cross_db().run(src);
    assert!(
        violations.is_empty(),
        "no-cross-db MUST admit the reindex-from-source path (it re-drives the live indexer, never \
         reads an owner DB) - found: {violations:?}"
    );
}

#[test]
fn a_load_from_postgres_backdoor_is_rejected() {
    let red = "use myelin_knowledge::storage::PageRepo; // load the index from Postgres (backdoor)";
    let violations = no_cross_db().run(red);
    assert!(
        !violations.is_empty(),
        "no-cross-db MUST reject a 'load the index from Postgres' backdoor (a reach into an owner's \
         storage module) - the SEARCH-1 anti-pattern"
    );
    assert!(
        violations.iter().all(|v| v.lint.0 == "no-cross-db"),
        "every violation must carry the no-cross-db id"
    );
}
