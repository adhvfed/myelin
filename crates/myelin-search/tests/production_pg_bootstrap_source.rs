#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn production_search_uses_split_role_bootstrap() {
    support::assert_split_role_source(
        include_str!("../src/main.rs"),
        "search_service_migrations()",
        "run_search(Config::default()",
    );
    support::assert_missing_migration_credential_fails_before_serve(
        env!("CARGO_BIN_EXE_search"),
        "search",
        "search service failed",
    );
}
