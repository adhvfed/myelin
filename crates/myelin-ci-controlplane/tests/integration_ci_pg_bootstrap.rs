//! Live proof that CI Controlplane applies its complete schema with the migration role, then serves
//! with only the constrained runtime role.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::{all_durable_migrations, HotTables, PgBootstrap};
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

async fn exercise(admin: &sqlx::PgPool, schema: &str, suffix: &str) -> Result<(), String> {
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema} AUTHORIZATION myelin_admin;
         GRANT USAGE ON SCHEMA {schema} TO myelin_app;
         ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA {schema}
           GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;
         ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA {schema}
           GRANT USAGE, SELECT ON SEQUENCES TO myelin_app;"
    ))
    .execute(admin)
    .await
    .map_err(|e| format!("set up isolated Controlplane schema: {e}"))?;

    let mut config = MyelinConfig::dev();
    config.region = format!("ci-controlplane-bootstrap-{suffix}");
    config.database_url = with_search_path(&config.database_url, schema);
    config.database_migration_url = with_search_path(&config.database_migration_url, schema);

    let bootstrap = PgBootstrap::connect(config, 2)
        .await
        .map_err(|e| format!("validate split Controlplane roles: {e}"))?;
    bootstrap
        .migrate_foundation()
        .await
        .map_err(|e| format!("migrate Controlplane foundation: {e}"))?;
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .map_err(|e| format!("migrate Controlplane durable aggregate: {e}"))?;
    bootstrap
        .migrate(
            &myelin_ci_controlplane::ci_controlplane_migrations(),
            &myelin_ci_controlplane::ci_controlplane_hot_tables(),
        )
        .await
        .map_err(|e| format!("migrate complete Controlplane schema: {e}"))?;

    let provider = bootstrap
        .into_runtime()
        .await
        .map_err(|e| format!("handoff to constrained Controlplane runtime: {e}"))?;
    if !provider.config().database_migration_url.is_empty() {
        return Err("Controlplane runtime retained the migration credential".into());
    }
    myelin_ci_controlplane::verify_ci_cost_event_shape(provider.db_pool())
        .await
        .map_err(|e| format!("verify CI money-table shape: {e}"))?;

    for table in [
        "outbox",
        "ci_run",
        "ci_job",
        "job_queue",
        "ci_job_spec",
        "ci_cost_event",
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM information_schema.tables
                  WHERE table_schema = $1 AND table_name = $2
             )",
        )
        .bind(schema)
        .bind(table)
        .fetch_one(provider.db_pool())
        .await
        .map_err(|e| format!("inspect migrated Controlplane table {table}: {e}"))?;
        if !exists {
            return Err(format!(
                "Controlplane bootstrap omitted required table {table}"
            ));
        }
    }

    if sqlx::query("CREATE TABLE runtime_must_not_create (id integer)")
        .execute(provider.db_pool())
        .await
        .is_ok()
    {
        return Err("Controlplane runtime role unexpectedly retained DDL capability".into());
    }
    if sqlx::query("ALTER TABLE ci_run DISABLE ROW LEVEL SECURITY")
        .execute(provider.db_pool())
        .await
        .is_ok()
    {
        return Err("Controlplane runtime role unexpectedly disabled CI RLS".into());
    }

    let migration_sessions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity
          WHERE usename = 'myelin_admin' AND application_name = $1",
    )
    .bind(format!("myelin:ci-controlplane-bootstrap-{suffix}"))
    .fetch_one(admin)
    .await
    .map_err(|e| format!("inspect closed Controlplane migration pool: {e}"))?;
    if migration_sessions != 0 {
        return Err("Controlplane privileged migration pool survived runtime handoff".into());
    }

    provider.db_pool().close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn complete_controlplane_migrations_precede_constrained_runtime_handoff() {
    let base = MyelinConfig::dev();
    let admin = match PgPoolOptions::new()
        .max_connections(2)
        .connect(&base.database_migration_url)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("SKIP live CI Controlplane bootstrap proof: {error}");
            return;
        }
    };

    let suffix = unique_suffix();
    let schema = format!("ci_controlplane_bootstrap_{suffix}");
    let result = exercise(&admin, &schema, &suffix).await;
    let cleanup = sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await;
    admin.close().await;

    cleanup.expect("drop only the isolated CI Controlplane bootstrap schema");
    if let Err(error) = result {
        panic!("CI Controlplane split-role bootstrap proof failed: {error}");
    }
}
