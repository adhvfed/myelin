#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::migration::{HotTables, Migration, Migrations};
use myelin_storage::PgBootstrap;
use sqlx::postgres::PgPoolOptions;

fn unique_suffix() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos()
    )
}

fn with_search_path(url: &str, schema: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}options=-csearch_path%3D{schema}%2Cpublic")
}

#[tokio::test]
async fn legacy_bundled_ci_0004_could_not_be_recorded_by_pg_migrator() {
    let base = MyelinConfig::dev();
    let admin = match PgPoolOptions::new()
        .max_connections(2)
        .connect(&base.database_migration_url)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("SKIP live CI migration activation-boundary proof: {error}");
            return;
        }
    };

    let suffix = unique_suffix();
    let schema = format!("ci_migration_boundary_{suffix}");
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema} AUTHORIZATION myelin_admin;
         GRANT USAGE ON SCHEMA {schema} TO myelin_app;"
    ))
    .execute(&admin)
    .await
    .expect("create only the isolated activation-boundary schema");

    let mut config = MyelinConfig::dev();
    config.region = format!("ci-migration-boundary-{suffix}");
    config.database_url = with_search_path(&config.database_url, &schema);
    config.database_migration_url = with_search_path(&config.database_migration_url, &schema);
    let bootstrap = PgBootstrap::connect(config, 2)
        .await
        .expect("validate split roles for the isolated schema");

    let mut legacy = String::from(myelin_ci_controlplane::CREATE_JOB_QUEUE_DDL);
    legacy.push(';');
    for (_, index) in myelin_ci_controlplane::CREATE_JOB_QUEUE_INDEXES_DDL {
        legacy.push('\n');
        legacy.push_str(index);
        legacy.push(';');
    }
    legacy.push('\n');
    legacy.push_str("SELECT myelin_make_tenant_scoped('job_queue');");
    let legacy: &'static str = Box::leak(legacy.into_boxed_str());
    let error = bootstrap
        .migrate(
            &Migrations::of([Migration::plain_on(
                "ci_0004_job_queue",
                legacy,
                "job_queue",
            )]),
            &HotTables::declare(["job_queue"]),
        )
        .await
        .expect_err("legacy bundled concurrent indexes must be rejected atomically");
    assert!(
        error
            .to_string()
            .contains("cannot run inside a transaction block"),
        "the refusal is PostgreSQL's non-transactional concurrent-index rule: {error}"
    );

    let recorded: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS (
             SELECT 1 FROM {schema}.myelin_applied_migration
              WHERE id = 'ci_0004_job_queue'
         )"
    ))
    .fetch_one(&admin)
    .await
    .expect("inspect the isolated migration ledger");
    assert!(
        !recorded,
        "failed legacy ci_0004 was never version-recorded"
    );

    let table_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM information_schema.tables
              WHERE table_schema = $1 AND table_name = 'job_queue'
         )",
    )
    .bind(&schema)
    .fetch_one(&admin)
    .await
    .expect("inspect the atomic rollback");
    assert!(
        !table_exists,
        "the rejected legacy command rolled its table back too"
    );

    drop(bootstrap);
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("drop only the isolated activation-boundary schema");
    admin.close().await;
}
