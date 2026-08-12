#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn without_a_migration_credential_flow_refuses_to_start_its_worker_or_service() {
    support::assert_service_refuses_runtime_without_migration_credential(
        env!("CARGO_BIN_EXE_myelin-flow"),
        "myelin-flow",
        &["myelin-flow service boot failed"],
    );
}
