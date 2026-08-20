#![cfg(feature = "integration")]

use myelin_storage::migration::{Migration, Migrations};
use myelin_storage::pg_migrator::ddl_checksum;
use myelin_storage::PgMigrator;
use sqlx::PgPool;

mod common;

const DRIFT_RACERS: usize = 8;

fn unique_suffix(label: &str) -> String {
    format!(
        "{label}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos()
    )
}

fn leaked(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

async fn cleanup(pool: &PgPool, ids: &[&str], tables: &[&str]) {
    for table in tables {
        let _ = sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {table}"))
            .execute(pool)
            .await;
    }
    for id in ids {
        let _ = sqlx::query("DELETE FROM myelin_applied_migration WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await;
    }
}

#[tokio::test]
async fn same_id_same_ddl_is_checksum_verified_and_skipped() {
    let pool = common::admin_pool(4).await;
    let suffix = unique_suffix("checksum_same");
    let id = leaked(format!("{suffix}_0001"));
    let table = leaked(format!("{suffix}_table"));
    let ddl = leaked(format!("CREATE TABLE {table} (id text PRIMARY KEY)"));
    let migrations = Migrations::of([Migration::plain(id, ddl)]);
    let ids = [id];
    let tables = [table];
    cleanup(&pool, &ids, &tables).await;

    common::with_cleanup(
        || async {
            PgMigrator::apply(&pool, &migrations)
                .await
                .expect("first apply executes and records the DDL");
            PgMigrator::apply(&pool, &migrations)
                .await
                .expect("same id and same DDL checksum skips idempotently");

            let stored: String =
                sqlx::query_scalar("SELECT checksum FROM myelin_applied_migration WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .expect("migration row is recorded");
            assert_eq!(stored, ddl_checksum(ddl));
            assert_eq!(
                PgMigrator::applied_count(&pool, id)
                    .await
                    .expect("count migration rows"),
                1,
                "the id remains recorded exactly once"
            );
        },
        || cleanup(&pool, &ids, &tables),
    )
    .await;
}

#[tokio::test]
async fn same_id_different_ddl_fails_before_any_later_migration() {
    let pool = common::admin_pool(4).await;
    let suffix = unique_suffix("checksum_drift");
    let id = leaked(format!("{suffix}_0001"));
    let first_table = leaked(format!("{suffix}_original"));
    let drift_table = leaked(format!("{suffix}_drift"));
    let later_id = leaked(format!("{suffix}_0002"));
    let later_table = leaked(format!("{suffix}_later"));
    let original_ddl = leaked(format!("CREATE TABLE {first_table} (id text PRIMARY KEY)"));
    let drifted_ddl = leaked(format!("CREATE TABLE {drift_table} (id text PRIMARY KEY)"));
    let later_ddl = leaked(format!("CREATE TABLE {later_table} (id text PRIMARY KEY)"));
    let ids = [id, later_id];
    let tables = [first_table, drift_table, later_table];
    cleanup(&pool, &ids, &tables).await;

    common::with_cleanup(
        || async {
            PgMigrator::apply(&pool, &Migrations::of([Migration::plain(id, original_ddl)]))
                .await
                .expect("seed the original immutable migration");

            let error = PgMigrator::apply(
                &pool,
                &Migrations::of([
                    Migration::plain(id, drifted_ddl),
                    Migration::plain(later_id, later_ddl),
                ]),
            )
            .await
            .expect_err("changed DDL behind an applied id must fail");
            let message = error.to_string();
            assert!(message.contains("checksum mismatch"), "{message}");
            assert!(message.contains(id), "{message}");
            assert!(message.contains(&ddl_checksum(original_ddl)), "{message}");
            assert!(message.contains(&ddl_checksum(drifted_ddl)), "{message}");

            let drift_exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                .bind(drift_table)
                .fetch_one(&pool)
                .await
                .expect("probe drift table");
            let later_exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                .bind(later_table)
                .fetch_one(&pool)
                .await
                .expect("probe later table");
            assert!(!drift_exists, "the drifted DDL itself never executes");
            assert!(!later_exists, "no migration after the mismatch executes");
            assert!(
                !PgMigrator::is_applied(&pool, later_id)
                    .await
                    .expect("probe later migration row"),
                "the later migration is not recorded"
            );
        },
        || cleanup(&pool, &ids, &tables),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_drift_attempts_all_fail_closed_without_running_later_ddl() {
    let pool = common::admin_pool((DRIFT_RACERS as u32) + 4).await;
    let suffix = unique_suffix("checksum_race");
    let id = leaked(format!("{suffix}_0001"));
    let original_table = leaked(format!("{suffix}_original"));
    let drift_table = leaked(format!("{suffix}_drift"));
    let later_id = leaked(format!("{suffix}_0002"));
    let later_table = leaked(format!("{suffix}_later"));
    let original_ddl = leaked(format!(
        "CREATE TABLE {original_table} (id text PRIMARY KEY)"
    ));
    let drifted_ddl = leaked(format!("CREATE TABLE {drift_table} (id text PRIMARY KEY)"));
    let later_ddl = leaked(format!("CREATE TABLE {later_table} (id text PRIMARY KEY)"));
    let ids = [id, later_id];
    let tables = [original_table, drift_table, later_table];
    cleanup(&pool, &ids, &tables).await;

    common::with_cleanup(
        || async {
            PgMigrator::apply(&pool, &Migrations::of([Migration::plain(id, original_ddl)]))
                .await
                .expect("seed the original immutable migration");

            let drifted = Migrations::of([
                Migration::plain(id, drifted_ddl),
                Migration::plain(later_id, later_ddl),
            ]);
            let mut tasks = Vec::with_capacity(DRIFT_RACERS);
            for _ in 0..DRIFT_RACERS {
                let pool = pool.clone();
                let drifted = drifted.clone();
                tasks.push(tokio::spawn(async move {
                    PgMigrator::apply(&pool, &drifted).await
                }));
            }
            for (index, task) in tasks.into_iter().enumerate() {
                let error = task
                    .await
                    .expect("drift racer did not panic")
                    .expect_err("every drift racer must fail closed");
                assert!(
                    error.to_string().contains("checksum mismatch"),
                    "racer {index} returned the wrong error: {error}"
                );
            }

            let stored: String =
                sqlx::query_scalar("SELECT checksum FROM myelin_applied_migration WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .expect("original checksum remains recorded");
            assert_eq!(stored, ddl_checksum(original_ddl));
            for table in [drift_table, later_table] {
                let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
                    .bind(table)
                    .fetch_one(&pool)
                    .await
                    .expect("probe forbidden table");
                assert!(!exists, "concurrent mismatch never executes {table}");
            }
            assert!(
                !PgMigrator::is_applied(&pool, later_id)
                    .await
                    .expect("probe later migration row"),
                "the later migration remains unapplied across all racers"
            );
        },
        || cleanup(&pool, &ids, &tables),
    )
    .await;
}
