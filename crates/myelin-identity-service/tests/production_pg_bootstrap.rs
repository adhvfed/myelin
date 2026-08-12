#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn without_a_migration_credential_identity_refuses_to_open_its_runtime_surface() {
    support::assert_service_refuses_runtime_without_migration_credential(
        env!("CARGO_BIN_EXE_identity"),
        "identity",
        &["identity service failed"],
    );
}
