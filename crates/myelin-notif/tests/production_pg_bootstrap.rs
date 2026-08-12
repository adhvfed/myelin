#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn without_a_migration_credential_notifications_refuse_to_bind_signal_intake() {
    support::assert_service_refuses_runtime_without_migration_credential(
        env!("CARGO_BIN_EXE_notif"),
        "notif",
        &["notif service failed"],
    );
}
