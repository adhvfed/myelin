#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::migration::{Migration, Migrations};
use myelin_storage::PgMigrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

mod common;

const N: usize = 12;

async fn admin_pool() -> Option<PgPool> {
    let cfg = MyelinConfig::dev();
    let admin_url = cfg
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    PgPoolOptions::new()
        .max_connections((N as u32) + 4)
        .connect(&admin_url)
        .await
        .ok()
}

fn concurrent_migration_set() -> (Migrations, Vec<&'static str>) {
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

async fn cleanup(pool: &PgPool, ids: &[&'static str], migrations: &Migrations) {
    for m in &migrations.0 {
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

fn table_name_of(ddl: &str) -> Option<&str> {
    let marker = "CREATE TABLE IF NOT EXISTS ";
    let start = ddl.find(marker)? + marker.len();
    let rest = &ddl[start..];
    let end = rest.find([' ', '(']).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

async fn run_one(pool: PgPool, migrations: Migrations) -> Result<(), myelin_storage::PgError> {
    PgMigrator::apply(&pool, &migrations).await
}

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
    cleanup(&pool, &ids, &migrations).await;

    common::with_cleanup(
        || async {
            let mut handles = Vec::with_capacity(N);
            for _ in 0..N {
                let pool = pool.clone();
                let migrations = migrations.clone();
                handles.push(tokio::spawn(run_one(pool, migrations)));
            }

            let mut ok = 0usize;
            for (i, h) in handles.into_iter().enumerate() {
                match h.await.expect("migrator task did not panic") {
                    Ok(()) => ok += 1,
                    Err(e) => panic!(
                        "concurrent migrator task {i} FAILED: {e} - this is the pg_type_typname_nsp_index \
                         race the advisory lock fixes (it fires WITHOUT the lock)"
                    ),
                }
            }
            assert_eq!(ok, N, "all {N} concurrent migrators must return Ok");

            for id in &ids {
                let count = PgMigrator::applied_count(&pool, id)
                    .await
                    .expect("count applied migration");
                assert_eq!(
                    count, 1,
                    "migration id {id} must be recorded EXACTLY once (applied-once), got {count}"
                );
            }

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
        },
        || cleanup(&pool, &ids, &migrations),
    )
    .await;
}
