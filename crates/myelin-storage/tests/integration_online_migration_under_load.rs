#![cfg(feature = "integration")]

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use myelin_config::MyelinConfig;
use sqlx::postgres::PgPoolOptions;

fn unique_suffix() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos()
    )
}

fn lock_wait_budget_ms() -> u64 {
    let thresholds: toml::Value = include_str!("../../../thresholds.toml")
        .parse()
        .expect("thresholds.toml must parse");
    thresholds["online_migration"]["lock_wait_p99_max_ms"]
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
        .expect("online migration lock-wait budget must be a positive integer")
}

fn percentile_99(samples: &mut [u64]) -> u64 {
    samples.sort_unstable();
    let index = samples
        .len()
        .saturating_mul(99)
        .div_ceil(100)
        .saturating_sub(1);
    samples[index]
}

async fn exercise_online_migration(
    pool: &sqlx::PgPool,
    schema: &str,
    table: &str,
    index: &str,
    constraint: &str,
    budget_ms: u64,
) -> Result<(), String> {
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema};
         CREATE TABLE {schema}.{table} (
             id bigint PRIMARY KEY,
             tenant_id text NOT NULL,
             payload bigint NOT NULL DEFAULT 0
         );
         INSERT INTO {schema}.{table} (id, tenant_id)
             SELECT value, 'tenant-' || (value % 32)::text
             FROM generate_series(1, 50000) AS value;"
    ))
    .execute(pool)
    .await
    .map_err(|error| format!("prepare isolated hot table: {error}"))?;

    let mut migration = pool
        .acquire()
        .await
        .map_err(|error| format!("acquire migration connection: {error}"))?;
    sqlx::query(&format!("SET lock_timeout = '{budget_ms}ms'"))
        .execute(&mut *migration)
        .await
        .map_err(|error| format!("set database-enforced lock budget: {error}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let latencies_ms = Arc::new(Mutex::new(Vec::new()));
    let mut writers = Vec::new();

    for writer in 0..4_u64 {
        let writer_pool = pool.clone();
        let writer_stop = stop.clone();
        let writer_completed = completed.clone();
        let writer_errors = errors.clone();
        let writer_latencies = latencies_ms.clone();
        let qualified_table = format!("{schema}.{table}");
        writers.push(tokio::spawn(async move {
            let mut sequence = writer;
            while !writer_stop.load(Ordering::Relaxed) {
                sequence = sequence.wrapping_add(4);
                let id = i64::try_from(sequence % 50_000 + 1).expect("bounded row id");
                let started = Instant::now();
                match sqlx::query(&format!(
                    "UPDATE {qualified_table} SET payload = payload + 1 WHERE id = $1"
                ))
                .bind(id)
                .execute(&writer_pool)
                .await
                {
                    Ok(_) => {
                        writer_completed.fetch_add(1, Ordering::Relaxed);
                        writer_latencies
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
                    }
                    Err(_) => {
                        writer_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                tokio::task::yield_now().await;
            }
        }));
    }

    let warmup_deadline = Instant::now() + Duration::from_secs(5);
    while completed.load(Ordering::Relaxed) < 50 && Instant::now() < warmup_deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if completed.load(Ordering::Relaxed) < 50 {
        stop.store(true, Ordering::Relaxed);
        for writer in writers {
            let _ = writer.await;
        }
        return Err("concurrent writers did not warm up".into());
    }

    let completed_before = completed.load(Ordering::Relaxed);
    let migration_result = async {
        sqlx::query(&format!(
            "ALTER TABLE {schema}.{table} ADD COLUMN priority integer"
        ))
        .execute(&mut *migration)
        .await
        .map_err(|error| format!("expand nullable priority column: {error}"))?;

        for lower in (0..50_000_i64).step_by(1_000) {
            sqlx::query(&format!(
                "UPDATE {schema}.{table} SET priority = 0 WHERE id > $1 AND id <= $2"
            ))
            .bind(lower)
            .bind(lower + 1_000)
            .execute(&mut *migration)
            .await
            .map_err(|error| format!("bounded backfill batch after row {lower}: {error}"))?;
            tokio::task::yield_now().await;
        }

        sqlx::query(&format!(
            "CREATE INDEX CONCURRENTLY {index} ON {schema}.{table} (tenant_id, priority)"
        ))
        .execute(&mut *migration)
        .await
        .map_err(|error| format!("concurrent index build: {error}"))?;

        sqlx::query(&format!(
            "ALTER TABLE {schema}.{table} ADD CONSTRAINT {constraint} \
             CHECK (priority IS NOT NULL) NOT VALID"
        ))
        .execute(&mut *migration)
        .await
        .map_err(|error| format!("install non-blocking constraint: {error}"))?;

        sqlx::query(&format!(
            "ALTER TABLE {schema}.{table} VALIDATE CONSTRAINT {constraint}"
        ))
        .execute(&mut *migration)
        .await
        .map_err(|error| format!("validate constraint under writes: {error}"))?;

        Ok::<(), String>(())
    }
    .await;

    drop(migration);
    stop.store(true, Ordering::Relaxed);
    for writer in writers {
        writer
            .await
            .map_err(|error| format!("writer task join: {error}"))?;
    }
    migration_result?;

    let completed_after = completed.load(Ordering::Relaxed);
    if completed_after <= completed_before {
        return Err("no write completed while the migration ran".into());
    }
    if errors.load(Ordering::Relaxed) != 0 {
        return Err(format!(
            "{} writes errored while the migration ran",
            errors.load(Ordering::Relaxed)
        ));
    }

    let mut samples = {
        latencies_ms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    };
    if samples.is_empty() {
        return Err("writer latency sample is empty".into());
    }
    let p99_ms = percentile_99(&mut samples);
    if p99_ms > budget_ms {
        return Err(format!(
            "observed writer p99 {p99_ms}ms exceeded the {budget_ms}ms lock budget"
        ));
    }

    let null_priorities: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {schema}.{table} WHERE priority IS NULL"
    ))
    .fetch_one(pool)
    .await
    .map_err(|error| format!("verify backfill: {error}"))?;
    if null_priorities != 0 {
        return Err(format!("{null_priorities} rows escaped the backfill"));
    }

    let index_valid: bool = sqlx::query_scalar(
        "SELECT i.indisvalid
         FROM pg_index i
         JOIN pg_class c ON c.oid = i.indexrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = $1 AND c.relname = $2",
    )
    .bind(schema)
    .bind(index)
    .fetch_one(pool)
    .await
    .map_err(|error| format!("verify concurrent index: {error}"))?;
    if !index_valid {
        return Err("concurrent index exists but is invalid".into());
    }

    eprintln!(
        "online migration exercised PostgreSQL with {} successful concurrent writes, \
         writer p99={}ms, errors=0, lock_timeout={}ms",
        completed_after - completed_before,
        p99_ms,
        budget_ms
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn online_migration_keeps_real_postgres_writers_moving() {
    let config = MyelinConfig::dev();
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&config.database_migration_url)
        .await
        .expect("online-migration proof requires the configured PostgreSQL migration backend");

    let suffix = unique_suffix();
    let schema = format!("myelin_online_migration_{suffix}");
    let table = format!("hot_rows_{suffix}");
    let index = format!("hot_rows_priority_{suffix}");
    let constraint = format!("hot_rows_priority_set_{suffix}");
    let result = exercise_online_migration(
        &pool,
        &schema,
        &table,
        &index,
        &constraint,
        lock_wait_budget_ms(),
    )
    .await;
    let cleanup = sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&pool)
        .await;
    pool.close().await;

    if let Err(error) = cleanup {
        panic!("drop only isolated migration schema: {error}");
    }
    if let Err(error) = result {
        panic!("real PostgreSQL online-migration proof failed: {error}");
    }
}
