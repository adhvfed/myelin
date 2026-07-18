#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn production_flow_uses_split_role_bootstrap() {
    support::assert_split_role_source(
        include_str!("../src/main.rs"),
        "flow_migrations()",
        "run_flow(Config::default()",
    );
    support::assert_missing_migration_credential_fails_before_serve(
        env!("CARGO_BIN_EXE_myelin-flow"),
        "myelin-flow",
        "myelin-flow service failed",
    );
}
