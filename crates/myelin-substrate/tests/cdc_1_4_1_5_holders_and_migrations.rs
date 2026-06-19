//! **CDC 1.4 + 1.5 — the holder auto-registration + forward-only migration runner contract pair**
//! (P-S15 → global P-032). Contract-index rows 1.4 (`PersonalDataHolder` auto-registration
//! mechanism — every store the harness opens) and 1.5 (forward-only online migrations + the
//! hot-table declaration the `forward-only-migration` lint reads).
//!
//! Architecture: `00 §3.4` (auto-register every store the harness opens), `00 §9` (forward-only
//! online migrations — expand→backfill→contract), `00 §9.4` (the hot-table declaration mechanism).
//!
//! ## What this CDC pair proves (the cross-crate contract, both sides)
//! - **PROVIDER (1.4):** the harness — when a service boots through `serve`'s lifecycle — opens
//!   every store through the ONE door (`HolderRegistry`), so opening IS registering and "we forgot
//!   a store" is structurally impossible. The CONSUMER (a service's `AppSpec`) gets a typed
//!   `HolderRegistration` receipt per opened store.
//! - **PROVIDER (1.5):** the forward-only migration runner applies the embedded DDL in order at
//!   boot and REFUSES a destructive (`DROP`) migration AND a blocking `ALTER` on a declared-hot
//!   table; the CONSUMER declares its hot tables in `AppSpec::hot_tables`, the SAME declaration the
//!   `forward-only-migration` lint reads at source-scan.

use myelin_events::relay::InProcessBus;
use myelin_events::OutboxStore;
use myelin_substrate::serve::{boot, AppSpec, OutboxSpec};
use myelin_substrate::{
    is_blocking_alter, is_destructive, Config, CriticalDependencies, HolderRegistration, HotTables,
    InternalRpc, Migration, MigrationPhase, MigrationRunner, Migrations, PublicRoutes, StoreKind,
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
        outbox: OutboxSpec::new(OutboxStore::new(), InProcessBus::new()),
        critical: CriticalDependencies::default(),
    }
}

/// **CDC 1.4 (consumer side) — every store the harness opens auto-registers as a holder.** A
/// service boots; the lifecycle auto-registers its OLTP store as a `PersonalDataHolder` — the
/// receipt names it, and the registry reports it registered. No store escapes registration.
#[test]
fn cdc_1_4_opened_store_auto_registers_as_holder() {
    let handle = boot(spec("acct", Migrations::default(), HotTables::none())).expect("boot");
    assert_eq!(
        handle.registered_holders(),
        &[HolderRegistration { kind: StoreKind::Oltp, name: "acct" }],
        "the opened OLTP store auto-registered (§3.4, contract 1.4)"
    );
    assert!(
        handle.holder_registry().is_registered(StoreKind::Oltp, "acct"),
        "the registry confirms the store registered — opening IS registering (GD-3)"
    );
    // the PII-free holder id is the DSR fan-out address.
    assert!(handle.holder_registry().holder_ids().contains("oltp:acct"));
}

/// **CDC 1.4 (mechanism) — the four store kinds all register through the one door.** A service
/// that opens an OLTP schema + a blob prefix + a cache namespace + a search index registers all
/// four; the enum is closed, so no class of store can escape the holder fan-out.
#[test]
fn cdc_1_4_every_store_kind_registers_through_one_door() {
    use myelin_substrate::HolderRegistry;
    let mut reg = HolderRegistry::new();
    reg.open(StoreKind::Oltp, "svc_oltp");
    reg.open(StoreKind::Blob, "svc_blobs");
    reg.open(StoreKind::Cache, "svc_cache");
    reg.open(StoreKind::SearchIndex, "svc_index");
    assert_eq!(reg.len(), 4, "all four §3.4 store kinds registered");
    for id in ["oltp:svc_oltp", "blob:svc_blobs", "cache:svc_cache", "search_index:svc_index"] {
        assert!(reg.holder_ids().contains(id), "store `{id}` escaped registration");
    }
}

/// **CDC 1.5 (consumer side) — the runner applies forward-only migrations at boot.** A service's
/// embedded migration set is applied (after the substrate-co-located outbox/dedup tables); the
/// service boots only once its schema is migrated.
#[test]
fn cdc_1_5_runner_applies_forward_only_migrations_at_boot() {
    let migrations = Migrations::new([("0010_acct", "CREATE TABLE IF NOT EXISTS acct (id TEXT)")]);
    let handle = boot(spec("acct", migrations, HotTables::none())).expect("boot applies migrations");
    assert_eq!(handle.name(), "acct");
}

/// **CDC 1.5 (provider side) — a destructive (DROP) migration fails boot.** Forward-only is
/// structural: a service cannot start having silently destroyed data (§9.1; EI-01 §2).
#[test]
fn cdc_1_5_destructive_migration_fails_boot() {
    let migrations = Migrations::new([("0010_drop", "DROP TABLE acct")]);
    match boot(spec("acct", migrations, HotTables::none())) {
        Err(e) => assert!(e.0.contains("forward-only"), "the error names forward-only: {}", e.0),
        Ok(_) => panic!("a destructive migration must fail boot"),
    }
}

/// **CDC 1.5 (provider side) — a blocking `ALTER` on a DECLARED-HOT table fails boot (§9.4).** The
/// service declares `issue` hot in its `AppSpec`; a blocking `ALTER` on it is refused — a hot-table
/// change must be expand→backfill→contract.
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
            assert!(e.0.contains("declared-HOT"), "the error names the hot-table rule: {}", e.0);
            assert!(e.0.contains("issue"), "the error names the offending table: {}", e.0);
        }
        Ok(_) => panic!("a blocking ALTER on a declared-hot table must fail boot (§9.4)"),
    }
}

/// **CDC 1.5 — expand→backfill→contract is admitted on a hot table.** The three-deploy idiom
/// (nullable add → throttled backfill → constrain via a non-blocking path) boots cleanly even when
/// the table is declared hot — the runner forbids the BLOCKING step, not the discipline.
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

/// **CDC 1.5 — the shared DDL classifiers** the runner (at boot) and the `forward-only-migration`
/// lint (at source-scan) both rest on: `is_destructive` / `is_blocking_alter`.
#[test]
fn cdc_1_5_shared_ddl_classifiers() {
    assert!(is_destructive("DROP TABLE issue"));
    assert!(is_blocking_alter("ALTER TABLE issue ADD COLUMN x TEXT NOT NULL"));
    assert!(!is_blocking_alter("ALTER TABLE issue ADD COLUMN x TEXT")); // nullable add = expand.

    // a fresh runner has applied nothing.
    assert!(MigrationRunner::new().applied().is_empty());
}
