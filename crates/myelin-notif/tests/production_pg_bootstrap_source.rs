#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn production_notif_uses_split_role_bootstrap() {
    let source = include_str!("../src/main.rs");
    support::assert_split_role_source(
        source,
        "notif_migrations()",
        "run_notif_ingestion_until_shutdown(",
    );
    support::assert_missing_migration_credential_fails_before_serve(
        env!("CARGO_BIN_EXE_notif"),
        "notif",
        "notif service failed",
    );
    for required in [
        "MYELIN_CELL_ID",
        ".local_tenants(&cell_id)",
        "build_durable_router(",
        "DedupLedger::durable(",
        "NatsJetStreamBus::connect_consumer(",
        "run_notif_ingestion_until_shutdown(",
    ] {
        assert!(
            source.contains(required),
            "missing production intake seam: {required}"
        );
    }
    assert!(source.contains("SignalKind::terminate()"));
}
