//! Live proof that CI Controlplane applies its complete schema with the migration role, then serves
//! with only the constrained runtime role.
#![cfg(feature = "integration")]

mod common;

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

fn test_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(url) = std::env::var("DATABASE_URL") {
        config.database_url = url;
    }
    if let Ok(url) = std::env::var("DATABASE_MIGRATION_URL") {
        config.database_migration_url = url;
    }
    config
}

async fn exercise(
    admin: &sqlx::PgPool,
    schema: &str,
    suffix: &str,
    base: &MyelinConfig,
) -> Result<(), String> {
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

    let mut config = base.clone();
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
            &myelin_flow::migrations::migrations(),
            &HotTables::declare(["workflow_run"]),
        )
        .await
        .map_err(|e| format!("migrate Controlplane Flow prerequisite: {e}"))?;
    // The CI set installs `myelin_ci_region_scheduler` grants in THIS schema, and the production
    // scheduler provider's excess-privilege probe scans every non-system schema. Hold the fixture
    // lock across the apply, then strip those grants, so a concurrent scheduler-boundary or
    // production-boot test can never observe them.
    let mut migrate_result = Ok(());
    common::with_fixture_migration_lock(&base.database_migration_url, admin, schema, || async {
        migrate_result = bootstrap
            .migrate(
                &myelin_ci_controlplane::ci_controlplane_migrations(),
                &myelin_ci_controlplane::ci_controlplane_hot_tables(),
            )
            .await
            .map_err(|e| format!("migrate complete Controlplane schema: {e}"));
    })
    .await;
    migrate_result?;
    bootstrap
        .verify_index_ready_exact(myelin_ci_controlplane::CI_RUN_SURFACE_INDEX_READINESS)
        .await
        .map_err(|e| format!("verify exact CI run-list index identity: {e}"))?;

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
        "workflow_run",
        "ci_drive_manifest",
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

    let ledger_index_valid_and_ready: Option<bool> = sqlx::query_scalar(
        "SELECT index_state.indisvalid AND index_state.indisready
           FROM pg_catalog.pg_index AS index_state
           JOIN pg_catalog.pg_class AS index_relation
             ON index_relation.oid = index_state.indexrelid
           JOIN pg_catalog.pg_class AS table_relation
             ON table_relation.oid = index_state.indrelid
           JOIN pg_catalog.pg_namespace AS relation_namespace
             ON relation_namespace.oid = table_relation.relnamespace
          WHERE relation_namespace.nspname = $1
            AND table_relation.relname = 'ci_job'
            AND table_relation.relkind = 'r'
            AND index_relation.relnamespace = relation_namespace.oid
            AND index_relation.relname = $2
            AND index_relation.relkind = 'i'",
    )
    .bind(schema)
    .bind(myelin_ci_controlplane::CI_JOB_RUN_LEDGER_INDEX)
    .fetch_optional(provider.db_pool())
    .await
    .map_err(|e| format!("inspect migrated ci_job run-ledger index state: {e}"))?;
    if ledger_index_valid_and_ready != Some(true) {
        return Err(format!(
            "Controlplane bootstrap left ci_job_run_ledger missing, invalid, or not ready: {ledger_index_valid_and_ready:?}"
        ));
    }

    let validation_applied: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM myelin_applied_migration WHERE id = $1
         )",
    )
    .bind(myelin_ci_controlplane::CI_JOB_RUN_LEDGER_VALIDATION_MIGRATION_ID)
    .fetch_one(provider.db_pool())
    .await
    .map_err(|e| format!("inspect applied ci_job run-ledger validator: {e}"))?;
    if !validation_applied {
        return Err("Controlplane bootstrap did not execute ci_job_run_ledger validator".into());
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
    let base = test_config();
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
    let result = exercise(&admin, &schema, &suffix, &base).await;
    let cleanup = sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await;
    admin.close().await;

    cleanup.expect("drop only the isolated CI Controlplane bootstrap schema");
    if let Err(error) = result {
        panic!("CI Controlplane split-role bootstrap proof failed: {error}");
    }
}
