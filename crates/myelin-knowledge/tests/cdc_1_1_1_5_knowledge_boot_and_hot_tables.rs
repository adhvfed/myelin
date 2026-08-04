use myelin_events::OutboxStore;
use myelin_knowledge::{boot_knowledge, knowledge_app_spec, HOT_TABLES, SERVICE_NAME};
use myelin_substrate::{
    is_destructive, serve, Config, HotTables, Migration, MigrationPhase, MigrationRunner,
    Migrations, Readiness, Startup, StoreKind, Surface,
};

#[test]
fn consumer_knowledge_appspec_boots_three_ports_over_harness_provider() {
    let handle =
        boot_knowledge(Config::default(), OutboxStore::new()).expect("the knowledge shell boots");
    assert_eq!(handle.name(), SERVICE_NAME);
    assert_eq!(
        handle.surfaces(),
        &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
        "the harness PROVIDER opened the three ports around the Knowledge CONSUMER spec"
    );
    assert_eq!(handle.metrics_health().startup(), Startup::Complete);
    assert_eq!(
        handle.metrics_health().readiness().verdict,
        Readiness::Ready,
        "a booted knowledge instance is ready once the provider's migrate-gate lifts"
    );
    assert!(
        handle
            .holder_registry()
            .is_registered(StoreKind::Oltp, SERVICE_NAME),
        "the knowledge OLTP store auto-registered as a PersonalDataHolder"
    );
}

#[test]
fn consumer_knowledge_service_serves_and_drains_cleanly() {
    assert_eq!(
        serve(knowledge_app_spec(Config::default(), OutboxStore::new())),
        Ok(()),
        "the knowledge service boots → … → drains cleanly over the harness PROVIDER"
    );
}

#[test]
fn consumer_knowledge_appspec_declares_the_three_hot_tables() {
    let spec = knowledge_app_spec(Config::default(), OutboxStore::new());
    for table in HOT_TABLES {
        assert!(
            spec.hot_tables.is_hot(table),
            "the CONSUMER declares `{table}` hot (contract 1.5)"
        );
    }
    let mut declared: Vec<&str> = spec.hot_tables.tables().collect();
    declared.sort_unstable();
    assert_eq!(
        declared,
        ["block", "db_row", "doc_op"],
        "exactly the three high-write tables are declared hot, nothing else"
    );
}

#[test]
fn provider_migration_runner_refuses_blocking_alter_on_hot_table() {
    let mut runner = MigrationRunner::new();
    let migrations = Migrations::of([Migration::phased(
        "0210_block_blocking_alter",
        "ALTER TABLE block ADD COLUMN extra TEXT NOT NULL",
        MigrationPhase::Plain,
        "block",
    )]);
    let r = runner.run(&migrations, &HotTables::declare(HOT_TABLES));
    assert!(
        r.is_err(),
        "the PROVIDER refuses a blocking ALTER on the CONSUMER's declared-hot `block` table"
    );
}

#[test]
fn provider_migration_runner_refuses_destructive_migration() {
    for m in &knowledge_app_spec(Config::default(), OutboxStore::new())
        .migrations
        .0
    {
        assert!(
            !is_destructive(m.ddl),
            "the Knowledge migration `{}` is forward-only",
            m.id
        );
    }
    let mut runner = MigrationRunner::new();
    let bad = Migrations::of([Migration::plain("0210_bad", "DROP TABLE doc_op")]);
    assert!(
        runner.run(&bad, &HotTables::declare(HOT_TABLES)).is_err(),
        "the PROVIDER refuses a destructive migration (forward-only)"
    );
}
