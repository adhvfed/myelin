#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn production_flow_uses_split_role_bootstrap() {
    let source = include_str!("../src/main.rs");
    support::assert_split_role_source(source, "flow_migrations()", "boot_flow(Config::default()");
    support::assert_missing_migration_credential_fails_before_serve(
        env!("CARGO_BIN_EXE_myelin-flow"),
        "myelin-flow",
        "myelin-flow service boot failed",
    );
    assert!(source.contains("shutdown_signal()"));
    assert!(source.contains("SignalKind::terminate()"));
    assert!(source.contains("shutdown_error = signal.err()"));
    assert!(source.contains("myelin-flow shutdown signal failed"));
}
