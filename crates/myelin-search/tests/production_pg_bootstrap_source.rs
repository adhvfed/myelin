#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn production_search_uses_split_role_bootstrap() {
    let source = include_str!("../src/main.rs");
    support::assert_split_role_source(
        source,
        "search_service_migrations()",
        "run_search_until_shutdown(Config::default()",
    );
    support::assert_missing_migration_credential_fails_before_serve(
        env!("CARGO_BIN_EXE_search"),
        "search",
        "search service failed",
    );
    assert!(source.contains("SignalKind::terminate()"));
}
