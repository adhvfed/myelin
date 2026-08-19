use myelin_events::relay::InProcessBus;
use myelin_events::OutboxStore;
use myelin_substrate::serve::{boot, AppSpec, OutboxSpec};
use myelin_substrate::{
    assert_all_holders_registered, holder_registered, is_blocking_alter, is_destructive, Config,
    CriticalDependencies, DeclaredStore, HolderRegistration, HolderRegistry, HotTables,
    InternalRpc, Migration, MigrationPhase, MigrationRunner, Migrations, PublicRoutes, StoreKind,
    StoreManifest,
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
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::new(OutboxStore::new(), InProcessBus::new()),
        critical: CriticalDependencies::default(),
        intake_scope: None,
    }
}

#[test]
fn cdc_1_4_opened_store_auto_registers_as_holder() {
    let handle = boot(spec("acct", Migrations::default(), HotTables::none())).expect("boot");
    assert_eq!(
        handle.registered_holders(),
        &[HolderRegistration {
            kind: StoreKind::Oltp,
            name: "acct"
        }],
        "the opened OLTP store auto-registered (§3.4, contract 1.4)"
    );
    assert!(
        handle
            .holder_registry()
            .is_registered(StoreKind::Oltp, "acct"),
        "the registry confirms the store registered - opening IS registering (GD-3)"
    );
    assert!(handle.holder_registry().holder_ids().contains("oltp:acct"));
}

#[test]
fn cdc_1_4_every_store_kind_registers_through_one_door() {
    use myelin_substrate::HolderRegistry;
    let mut reg = HolderRegistry::new();
    reg.open(StoreKind::Oltp, "svc_oltp");
    reg.open(StoreKind::Blob, "svc_blobs");
    reg.open(StoreKind::Cache, "svc_cache");
    reg.open(StoreKind::SearchIndex, "svc_index");
    assert_eq!(reg.len(), 4, "all four §3.4 store kinds registered");
    for id in [
        "oltp:svc_oltp",
        "blob:svc_blobs",
        "cache:svc_cache",
        "search_index:svc_index",
    ] {
        assert!(
            reg.holder_ids().contains(id),
            "store `{id}` escaped registration"
        );
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

#[test]
fn cdc_1_4_arch_test_harness_opened_service_passes() {
    let mut s = spec("svc", Migrations::default(), HotTables::none());
    s.stores = StoreManifest::of([DeclaredStore::new(StoreKind::Blob, "svc_blobs")]);
    let handle = boot(s).expect("boot");

    assert_eq!(
        handle.holder_registered(),
        Ok(()),
        "a harness-opened service passes the holder-registered architecture test"
    );
    assert!(handle
        .holder_registry()
        .is_registered(StoreKind::Oltp, "svc"));
    assert!(handle
        .holder_registry()
        .is_registered(StoreKind::Blob, "svc_blobs"));
    assert!(handle.store_manifest().holder_ids().contains("oltp:svc"));
    assert!(handle
        .store_manifest()
        .holder_ids()
        .contains("blob:svc_blobs"));
}

#[test]
fn cdc_1_4_arch_test_store_opened_outside_the_harness_fails() {
    let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, "rogue_oltp")]);
    let registry = HolderRegistry::new();

    let violations = holder_registered(&manifest, &registry);
    assert_eq!(
        violations.len(),
        1,
        "the store opened outside the harness is the violation"
    );
    assert_eq!(
        violations[0].store,
        DeclaredStore::new(StoreKind::Oltp, "rogue_oltp")
    );

    let err = assert_all_holders_registered(&manifest, &registry)
        .expect_err("a store opened outside the harness MUST fail the architecture test");
    let msg = err[0].message();
    assert!(
        msg.contains("rogue_oltp"),
        "names the offending store: {msg}"
    );
    assert!(
        msg.contains("OUTSIDE the harness"),
        "names WHY it failed: {msg}"
    );
}

#[test]
fn cdc_1_4_arch_test_one_hundred_percent_of_harness_opened_stores_register() {
    let manifest = StoreManifest::of([
        DeclaredStore::new(StoreKind::Oltp, "svc_oltp"),
        DeclaredStore::new(StoreKind::Blob, "svc_blobs"),
        DeclaredStore::new(StoreKind::Cache, "svc_cache"),
        DeclaredStore::new(StoreKind::SearchIndex, "svc_index"),
    ]);
    let mut registry = HolderRegistry::new();
    for s in manifest.stores() {
        registry.open(s.kind, s.name);
    }
    assert_eq!(
        assert_all_holders_registered(&manifest, &registry),
        Ok(()),
        "100% of harness-opened stores auto-register - no store escapes the holder fan-out"
    );
}
