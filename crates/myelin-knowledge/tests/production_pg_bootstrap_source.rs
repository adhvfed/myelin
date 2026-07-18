#[path = "../../../testing/service_pg_bootstrap_test_support.rs"]
mod support;

#[test]
fn production_knowledge_uses_split_role_bootstrap() {
    let source = include_str!("../src/main.rs");
    support::assert_split_role_source(
        source,
        "knowledge_service_migrations()",
        "serve(knowledge_app_spec",
    );
    assert!(source.contains("HotTables::declare(HOT_TABLES)"));
    support::assert_missing_migration_credential_fails_before_serve(
        env!("CARGO_BIN_EXE_knowledge"),
        "knowledge",
        "knowledge service failed",
    );
}
