//! # The race-safe-migrate REGRESSION test (would have caught the `pg_type` race).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Runs ONLY against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' \
//!     cargo test -p myelin-storage --features integration \
//!       --test integration_migrate_concurrent -- --nocapture
//!
//! ## What this proves (and what it would catch)
//! The bug this whole change fixes: [`myelin_storage::pg::PgStore::migrate`] (and the other live-DDL
//! sites) used to run each migration as a bare `sqlx::raw_sql(ddl).execute(&pool)` with NO advisory
//! lock and NO version table. When **N processes/tests migrate the SAME database concurrently**, two
//! concurrent `CREATE TABLE`s each insert a row type and race on Postgres's
//! `pg_type_typname_nsp_index` — a duplicate-key error that fails one racer. `CREATE TABLE IF NOT
//! EXISTS` does NOT close it (the existence check + the row-type insert are not atomic against a
//! concurrent creator).
//!
//! This test spawns **N = 12** concurrent tasks that each drive
//! [`myelin_storage::PgMigrator::apply`] against the SAME live database with the SAME freshly-created
//! migration set, and asserts:
//!   1. **EVERY task returns `Ok`** — no `pg_type_typname_nsp_index` duplicate-key error (the
//!      failure mode WITHOUT the advisory lock).
//!   2. **Each migration id appears EXACTLY ONCE** in `myelin_applied_migration` (applied-once,
//!      version-recorded — the lock + the skip make re-runs/concurrent runs idempotent).
//!
//! **This test FAILS WITHOUT the advisory lock** in [`PgMigrator::apply`]: remove the
//! `pg_advisory_lock` and the 12 concurrent `CREATE TABLE`s race on `pg_type_typname_nsp_index` and
//! at least one task returns `Err` — it reproduces the original race.
//!
//! It skips gracefully if the DB is unreachable (like the other integration tests); on the dev host
//! it really runs against live Postgres.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::migration::{Migration, Migrations};
use myelin_storage::PgMigrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// The number of concurrent migrators racing on the SAME database. ≥ 8 per the regression spec; 12
/// gives the `pg_type` race ample opportunity to fire without the lock.
const N: usize = 12;

/// An admin pool (the migration/owner role) — DDL `CREATE TABLE` needs `CREATE` on `public`, which
/// PG16 revokes for the app role. The dev admin creds are the convention the sibling integration
/// tests use (`integration_backends.rs`).
async fn admin_pool() -> Option<PgPool> {
    let cfg = MyelinConfig::dev();
    let admin_url = cfg
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    PgPoolOptions::new()
        // Each of the N tasks acquires its own dedicated connection for its advisory lock, so the
        // pool must admit at least N simultaneous connections (plus a couple for setup/asserts).
        .max_connections((N as u32) + 4)
        .connect(&admin_url)
        .await
        .ok()
}

/// A fresh migration set with a process-unique table prefix, so the N racers genuinely CREATE the
/// SAME brand-new objects concurrently (the scenario that triggers the `pg_type` race). The ids are
/// stable across the N tasks (same id → applied once), and the table names are leaked as `'static`
/// so they fit `Migration`'s `&'static str` ddl/id contract.
fn concurrent_migration_set() -> (Migrations, Vec<&'static str>) {
    // A per-RUN unique suffix so a re-run of the test starts from a clean (non-existing) object set
    // — the genuine "creates objects concurrently" scenario the regression needs.
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );

    let mut migrations = Vec::new();
    let mut ids = Vec::new();
    for n in 0..4u32 {
        let id: &'static str = Box::leak(format!("racetest_{suffix}_{n:04}").into_boxed_str());
        let table: &'static str = Box::leak(format!("racetest_{suffix}_t{n}").into_boxed_str());
        // A CREATE TABLE (each creates a row type → the pg_type_typname_nsp_index contention point)
        // plus a non-concurrent index to widen the DDL surface. IF NOT EXISTS is present but does
        // NOT close the race on its own — the advisory lock does.
        let ddl: &'static str = Box::leak(
            format!(
                "CREATE TABLE IF NOT EXISTS {table} (\
                    id text PRIMARY KEY, tenant_id text NOT NULL, body text NOT NULL);\
                 CREATE INDEX IF NOT EXISTS {table}_tenant_idx ON {table} (tenant_id);"
            )
            .into_boxed_str(),
        );
        migrations.push(Migration::plain(id, ddl));
        ids.push(id);
    }
    (Migrations::of(migrations), ids)
}

/// Drop the test's tables + their applied-migration rows so the run is repeatable and leaves no
/// residue (best-effort).
async fn cleanup(pool: &PgPool, ids: &[&'static str], migrations: &Migrations) {
    for m in &migrations.0 {
        // The table name is derivable from the ddl, but simplest is to DROP every table the set
        // names; the ddl's first token after IF NOT EXISTS is the table.
        if let Some(table) = table_name_of(m.ddl) {
            let _ = sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {table}"))
                .execute(pool)
                .await;
        }
    }
    for id in ids {
        let _ = sqlx::query("DELETE FROM myelin_applied_migration WHERE id = $1")
            .bind(*id)
            .execute(pool)
            .await;
    }
}

/// Pull the table name out of a `CREATE TABLE IF NOT EXISTS <name> (…)` ddl (test-local helper).
fn table_name_of(ddl: &str) -> Option<&str> {
    let marker = "CREATE TABLE IF NOT EXISTS ";
    let start = ddl.find(marker)? + marker.len();
    let rest = &ddl[start..];
    let end = rest.find([' ', '(']).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// One racer: takes OWNED clones (`PgPool` is `Arc`-backed, cheap to clone) so the borrow
/// `PgMigrator::apply` takes is of a LOCAL owned pool fully contained in this future. The migrator's
/// own future is `Send` (it executes multi-statement DDL via the `Executor::execute(&str)` path, not
/// `raw_sql`), so this spawns cleanly.
async fn run_one(pool: PgPool, migrations: Migrations) -> Result<(), myelin_storage::PgError> {
    PgMigrator::apply(&pool, &migrations).await
}

/// THE REGRESSION: N concurrent migrators against the SAME DB all succeed, and every migration id is
/// applied EXACTLY ONCE. Without the advisory lock in `PgMigrator::apply`, the N concurrent
/// `CREATE TABLE`s race on `pg_type_typname_nsp_index` and at least one task returns `Err`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_migrate_is_race_safe_and_applied_once() {
    let Some(pool) = admin_pool().await else {
        eprintln!(
            "SKIP concurrent_migrate_is_race_safe_and_applied_once: dev Postgres unreachable \
             (is the docker stack up?)"
        );
        return;
    };
    let (migrations, ids) = concurrent_migration_set();
    // Start clean (in case a prior aborted run left residue) — the genuine create-concurrently case.
    cleanup(&pool, &ids, &migrations).await;

    // Spawn N tasks that ALL race to apply the SAME set against the SAME database at once. Each task
    // gets an OWNED `PgPool` clone (Arc-backed → same underlying pool/DB) + an owned `Migrations`
    // clone, so the migrator's borrow is of locals contained in the spawned future.
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let pool = pool.clone();
        let migrations = migrations.clone();
        handles.push(tokio::spawn(run_one(pool, migrations)));
    }

    // (1) EVERY task returns Ok — no pg_type_typname_nsp_index duplicate-key error (the without-lock
    //     failure mode). This is the assertion that goes RED if the advisory lock is removed.
    let mut ok = 0usize;
    for (i, h) in handles.into_iter().enumerate() {
        match h.await.expect("migrator task did not panic") {
            Ok(()) => ok += 1,
            Err(e) => panic!(
                "concurrent migrator task {i} FAILED: {e} — this is the pg_type_typname_nsp_index \
                 race the advisory lock fixes (it fires WITHOUT the lock)"
            ),
        }
    }
    assert_eq!(ok, N, "all {N} concurrent migrators must return Ok");

    // (2) Each migration id appears EXACTLY ONCE in the version table (applied-once, recorded).
    for id in &ids {
        let count = PgMigrator::applied_count(&pool, id)
            .await
            .expect("count applied migration");
        assert_eq!(
            count, 1,
            "migration id {id} must be recorded EXACTLY once (applied-once), got {count}"
        );
    }

    // And every target table genuinely exists (the DDL actually ran, once, under the lock).
    for m in &migrations.0 {
        let table = table_name_of(m.ddl).expect("ddl names a table");
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&pool)
            .await
            .expect("probe table existence");
        assert!(exists, "table {table} must exist after concurrent migrate");
    }

    println!(
        "OK: {N} concurrent migrators all returned Ok; {} migrations each applied exactly once \
         (no pg_type_typname_nsp_index race).",
        ids.len()
    );

    // Leave the DB clean.
    cleanup(&pool, &ids, &migrations).await;
}
