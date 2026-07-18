#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn production_identity_uses_split_role_bootstrap() {
    support::assert_split_role_source(
        include_str!("../src/main.rs"),
        "identity_service_migrations()",
        "serve(identity_app_spec",
    );
    support::assert_missing_migration_credential_fails_before_serve(
        env!("CARGO_BIN_EXE_identity"),
        "identity",
        "identity service failed",
    );
}
