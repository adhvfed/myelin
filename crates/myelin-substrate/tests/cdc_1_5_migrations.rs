use myelin_events::relay::InProcessBus;
use myelin_events::OutboxStore;
use myelin_substrate::serve::{boot, AppSpec, OutboxSpec};
use myelin_substrate::{
    is_blocking_alter, is_destructive, Config, CriticalDependencies, HotTables, InternalRpc,
    Migration, MigrationPhase, MigrationRunner, Migrations, PublicRoutes,
};

fn spec(name: &'static str, migrations: Migrations, hot_tables: HotTables) -> AppSpec {
    AppSpec {
        name,
        config: Config::default(),
        migrations,
        hot_tables,
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: vec![],
        outbox: OutboxSpec::new(OutboxStore::new(), InProcessBus::new()),
        critical: CriticalDependencies::default(),
        intake_scope: None,
    }
}

#[test]
fn cdc_1_5_runner_applies_forward_only_migrations_at_boot() {
    let migrations = Migrations::new([("0010_acct", "CREATE TABLE IF NOT EXISTS acct (id TEXT)")]);
    let handle =
        boot(spec("acct", migrations, HotTables::none())).expect("boot applies migrations");
    assert_eq!(handle.name(), "acct");
}

#[test]
fn cdc_1_5_destructive_migration_fails_boot() {
    let migrations = Migrations::new([("0010_drop", "DROP TABLE acct")]);
    match boot(spec("acct", migrations, HotTables::none())) {
        Err(e) => assert!(
            e.0.contains("forward-only"),
            "the error names forward-only: {}",
            e.0
        ),
        Ok(_) => panic!("a destructive migration must fail boot"),
    }
}

#[test]
fn cdc_1_5_blocking_alter_on_declared_hot_table_fails_boot() {
    let migrations = Migrations::of([Migration::phased(
        "0010_hot",
        "ALTER TABLE issue ADD COLUMN body TEXT NOT NULL;",
        MigrationPhase::Expand,
        "issue",
    )]);
    match boot(spec("tracker", migrations, HotTables::declare(["issue"]))) {
        Err(e) => {
            assert!(
                e.0.contains("declared-HOT"),
                "the error names the hot-table rule: {}",
                e.0
            );
            assert!(
                e.0.contains("issue"),
                "the error names the offending table: {}",
                e.0
            );
        }
        Ok(_) => panic!("a blocking ALTER on a declared-hot table must fail boot (§9.4)"),
    }
}

#[test]
fn cdc_1_5_expand_backfill_contract_admitted_on_hot_table() {
    let migrations = Migrations::of([
        Migration::phased(
            "0010_expand",
            "ALTER TABLE issue ADD COLUMN priority INT;",
            MigrationPhase::Expand,
            "issue",
        ),
        Migration::phased(
            "0011_backfill",
            "UPDATE issue SET priority = 0 WHERE priority IS NULL;",
            MigrationPhase::Backfill,
            "issue",
        ),
        Migration::phased(
            "0012_contract",
            "ALTER TABLE issue ADD COLUMN status TEXT NOT NULL DEFAULT 'open';",
            MigrationPhase::Contract,
            "issue",
        ),
    ]);
    let handle = boot(spec("tracker", migrations, HotTables::declare(["issue"])))
        .expect("expand→backfill→contract is admitted on a hot table");
    assert_eq!(handle.name(), "tracker");
}

#[test]
fn cdc_1_5_shared_ddl_classifiers() {
    assert!(is_destructive("DROP TABLE issue"));
    assert!(is_blocking_alter(
        "ALTER TABLE issue ADD COLUMN x TEXT NOT NULL"
    ));
    assert!(!is_blocking_alter("ALTER TABLE issue ADD COLUMN x TEXT"));

    assert!(MigrationRunner::new().applied().is_empty());
}
