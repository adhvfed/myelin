use myelin_lints::lints::search_requires_acl_filter;

#[test]
fn the_lint_rejects_an_unfiltered_search_path() {
    let lint = search_requires_acl_filter();
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

#[test]
fn the_lint_admits_a_pre_filtered_search_path() {
    let lint = search_requires_acl_filter();
    let green = "let hits = index.search(query.with_acl(acl_filter)).await?;";
    assert!(
        lint.run(green).is_empty(),
        "search-requires-acl-filter MUST admit a path that pre-filters with the composed ACL \
         Filter (0 false reject)"
    );
}
