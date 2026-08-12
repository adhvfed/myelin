#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn without_a_migration_credential_dispatch_refuses_to_bind_trigger_intake() {
    support::assert_service_refuses_runtime_without_migration_credential(
        env!("CARGO_BIN_EXE_ci-dispatch"),
        "ci-dispatch",
        &["reading CI config", "ci-dispatch service failed"],
    );
}
