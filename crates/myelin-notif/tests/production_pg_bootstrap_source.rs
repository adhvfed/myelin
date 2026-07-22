#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn production_notif_uses_split_role_bootstrap() {
    let source = include_str!("../src/main.rs");
    support::assert_split_role_source(
        source,
        "notif_migrations()",
        "run_notif_until_shutdown(Config::default()",
    );
    support::assert_missing_migration_credential_fails_before_serve(
        env!("CARGO_BIN_EXE_notif"),
        "notif",
        "notif service failed",
    );
    assert!(source.contains("SignalKind::terminate()"));
}
