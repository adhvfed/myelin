//! **CDC 1.1 + 1.5 — the Knowledge service-shell boot + the hot-table-flag declaration pair**
//! (KN-P04 → global P-294, M3). Contract-index rows 1.1 (`serve(AppSpec)` — the boot → migrate →
//! relay → consumers → three ports → graceful drain lifecycle) and 1.5 (forward-only online
//! migrations + the hot-table declaration the `forward-only-migration` lint reads).
//!
//! Architecture: `00 §3.1` (the one call — `serve(AppSpec)`), `00 §9` (forward-only online
//! migrations), `00 §9.4` (the hot-table declaration mechanism), and the Knowledge architecture
//! `01 §1` (the `block`/`db_row`/`doc_op` high-write tables) + `03 §4` (the service is a thin shell
//! over the harness).
//!
//! ## What this CDC pair proves (the cross-crate contract, both sides)
//! - **PROVIDER (1.1):** the harness ([`myelin_substrate::serve`]) owns the boot → migrate → relay
//!   → three-ports → graceful-drain lifecycle. The CONSUMER (the Knowledge `AppSpec`) supplies the
//!   eight-field spec and gets a booted instance with three ports + liveness ≠ readiness + a clean
//!   drain — it does NOT re-implement the lifecycle.
//! - **PROVIDER (1.5):** the forward-only migration runner refuses a destructive (`DROP`) migration
//!   AND a blocking `ALTER` on a declared-hot table. The CONSUMER (the Knowledge `AppSpec`) declares
//!   its hot tables (`block`/`db_row`/`doc_op`) in `AppSpec::hot_tables`, the SAME declaration the
//!   `forward-only-migration` lint reads at source-scan — so the high-write tables KN-P05 creates
//!   are protected from the first migration.
//!
//! ## The boot/ready/drain dated GREEN artifact (2026-06-22)
//! `consumer_knowledge_appspec_boots_three_ports_over_harness_provider` +
//! `consumer_knowledge_service_serves_and_drains_cleanly` are the dated green: the Knowledge shell
//! boots → migrates (readiness gates on migrate-complete; liveness stays Up while Booting) → opens
//! the three ports → graceful-drains to `outbox_depth == 0`. 0 ports missing, readiness=Ready only
//! after migrate-complete, drain clean. The `forward-only-migration` lint is green over the
//! Knowledge migrations (0 backward/destructive). No threshold weakened.

use myelin_events::OutboxStore;
use myelin_knowledge::{boot_knowledge, knowledge_app_spec, HOT_TABLES, SERVICE_NAME};
use myelin_substrate::{
    is_destructive, serve, Config, HotTables, Migration, MigrationPhase, MigrationRunner,
    Migrations, Readiness, Startup, StoreKind, Surface,
};

/// **CONSUMER side of 1.1 — the Knowledge AppSpec boots over the harness lifecycle.** The consumer
/// hands its spec to `serve`'s `boot`; the PROVIDER (the harness) opens the three ports and gates
/// readiness on migrate-complete. The consumer re-implements nothing.
#[test]
fn consumer_knowledge_appspec_boots_three_ports_over_harness_provider() {
    let handle =
        boot_knowledge(Config::default(), OutboxStore::new()).expect("the knowledge shell boots");
    assert_eq!(handle.name(), SERVICE_NAME);
    // PROVIDER opened all three surfaces around the CONSUMER's spec.
    assert_eq!(
        handle.surfaces(),
        &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
        "the harness PROVIDER opened the three ports around the Knowledge CONSUMER spec"
    );
    // liveness ≠ readiness: a booted-and-migrated instance is ready (the provider's gate lifted).
    assert_eq!(handle.metrics_health().startup(), Startup::Complete);
    assert_eq!(
        handle.metrics_health().readiness().verdict,
        Readiness::Ready,
        "a booted knowledge instance is ready once the provider's migrate-gate lifts"
    );
    // The OLTP store auto-registered (opening IS registering — the holder mechanism, 1.4-adjacent).
    assert!(
        handle
            .holder_registry()
            .is_registered(StoreKind::Oltp, SERVICE_NAME),
        "the knowledge OLTP store auto-registered as a PersonalDataHolder"
    );
}

/// **CONSUMER side of 1.1 — the whole lifecycle runs end-to-end and graceful-drains.** `serve` of
/// the Knowledge spec boots → migrates → relays → opens the ports → drains cleanly (the PROVIDER's
/// drain order; the consumer just supplies the spec).
#[test]
fn consumer_knowledge_service_serves_and_drains_cleanly() {
    assert_eq!(
        serve(knowledge_app_spec(Config::default(), OutboxStore::new())),
        Ok(()),
        "the knowledge service boots → … → drains cleanly over the harness PROVIDER"
    );
}

/// **CONSUMER side of 1.5 — the Knowledge AppSpec DECLARES its three hot tables.** The consumer's
/// `hot_tables` carries exactly `block`/`db_row`/`doc_op` — the declaration the PROVIDER (the
/// migration runner) + the `forward-only-migration` lint both read.
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

/// **PROVIDER side of 1.5 — the migration runner refuses a blocking ALTER on a declared-hot table.**
/// Given the CONSUMER's hot-table declaration, the PROVIDER (the runner) rejects a non-online
/// (blocking) `ALTER` on `block` at boot — the high-write table must use expand→backfill→contract.
#[test]
fn provider_migration_runner_refuses_blocking_alter_on_hot_table() {
    let mut runner = MigrationRunner::new();
    // A genuinely-blocking ALTER (ADD COLUMN … NOT NULL with no DEFAULT) targeting the hot `block`.
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

/// **PROVIDER side of 1.5 — the runner refuses a destructive (DROP) migration (forward-only).** The
/// Knowledge skeleton carries no destructive DDL; the PROVIDER enforces the rule the
/// `forward-only-migration` lint mirrors over source.
#[test]
fn provider_migration_runner_refuses_destructive_migration() {
    // The CONSUMER's own skeleton is forward-only.
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
    // And the PROVIDER refuses a destructive one at boot.
    let mut runner = MigrationRunner::new();
    let bad = Migrations::of([Migration::plain("0210_bad", "DROP TABLE doc_op")]);
    assert!(
        runner.run(&bad, &HotTables::declare(HOT_TABLES)).is_err(),
        "the PROVIDER refuses a destructive migration (forward-only)"
    );
}
