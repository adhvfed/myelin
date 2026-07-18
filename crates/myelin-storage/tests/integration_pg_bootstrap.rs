//! Live proof for the split-credential PostgreSQL bootstrap.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::migration::{HotTables, Migration, Migrations};
use myelin_storage::{PgBootstrap, PgError};
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
    format!("{url}{separator}options=-csearch_path%3D{schema}")
}

async fn exercise_bootstrap(
    admin_pool: &sqlx::PgPool,
    schema: &str,
    suffix: &str,
) -> Result<(), String> {
    let setup = format!(
        "CREATE SCHEMA {schema} AUTHORIZATION myelin_admin;
         GRANT USAGE ON SCHEMA {schema} TO myelin_app;
         ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA {schema}
           GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;"
    );
    sqlx::raw_sql(&setup)
        .execute(admin_pool)
        .await
        .map_err(|e| format!("set up isolated schema: {e}"))?;

    let mut config = MyelinConfig::dev();
    config.region = format!("bootstrap-{suffix}");
    config.database_url = with_search_path(&config.database_url, schema);
    config.database_migration_url = with_search_path(&config.database_migration_url, schema);

    let bootstrap = PgBootstrap::connect(config, 2)
        .await
        .map_err(|e| format!("connect bootstrap: {e}"))?;
    bootstrap
        .migrate_foundation()
        .await
        .map_err(|e| format!("migrate foundation: {e}"))?;

    let table = format!("bootstrap_rls_{suffix}");
    let ddl: &'static str = Box::leak(
        format!(
            "CREATE TABLE {table} (
                 tenant_id text NOT NULL,
                 region text NOT NULL,
                 value text NOT NULL,
                 PRIMARY KEY (tenant_id, region, value)
             );
             ALTER TABLE {table} ENABLE ROW LEVEL SECURITY;
             ALTER TABLE {table} FORCE ROW LEVEL SECURITY;
             CREATE POLICY tenant_isolation ON {table}
               USING (
                 tenant_id = current_setting('myelin.tenant_id', true)
                 AND region = current_setting('myelin.region', true)
               )
               WITH CHECK (
                 tenant_id = current_setting('myelin.tenant_id', true)
                 AND region = current_setting('myelin.region', true)
               );"
        )
        .into_boxed_str(),
    );
    let migration_id: &'static str =
        Box::leak(format!("9000_pg_bootstrap_{suffix}").into_boxed_str());
    bootstrap
        .migrate(
            &Migrations::of([Migration::plain(migration_id, ddl)]),
            &HotTables::none(),
        )
        .await
        .map_err(|e| format!("migrate isolated RLS table: {e}"))?;

    let provider = bootstrap
        .into_runtime()
        .await
        .map_err(|e| format!("handoff runtime: {e}"))?;
    if !provider.config().database_migration_url.is_empty() {
        return Err("runtime provider retained the migration credential".into());
    }

    let migration_sessions: i64 = sqlx::query_scalar(
        "SELECT count(*)
           FROM pg_stat_activity
          WHERE usename = 'myelin_admin' AND application_name = $1",
    )
    .bind(format!("myelin:bootstrap-{suffix}"))
    .fetch_one(admin_pool)
    .await
    .map_err(|e| format!("inspect closed migration pool: {e}"))?;
    if migration_sessions != 0 {
        return Err("privileged migration pool still has live sessions after handoff".into());
    }

    if sqlx::query("CREATE TABLE runtime_must_not_create (id integer)")
        .execute(provider.db_pool())
        .await
        .is_ok()
    {
        return Err("runtime role unexpectedly created a table".into());
    }
    if sqlx::query(&format!("ALTER TABLE {table} DISABLE ROW LEVEL SECURITY"))
        .execute(provider.db_pool())
        .await
        .is_ok()
    {
        return Err("runtime role unexpectedly disabled RLS".into());
    }
    if sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN elevated text"))
        .execute(provider.db_pool())
        .await
        .is_ok()
    {
        return Err("runtime role unexpectedly altered an application table".into());
    }

    let region_a = format!("bootstrap-{suffix}");
    let table_a = table.clone();
    provider
        .with_tenant_tx("tenant-a", move |conn| {
            Box::pin(async move {
                sqlx::query(&format!(
                    "INSERT INTO {table_a} (tenant_id, region, value) VALUES ($1, $2, 'a')"
                ))
                .bind("tenant-a")
                .bind(region_a)
                .execute(conn)
                .await
                .map_err(|e| PgError::Query(format!("insert tenant A: {e}")))?;
                Ok(())
            })
        })
        .await
        .map_err(|e| format!("tenant A write: {e}"))?;

    let region_b = format!("bootstrap-{suffix}");
    let table_b = table.clone();
    provider
        .with_tenant_tx("tenant-b", move |conn| {
            Box::pin(async move {
                sqlx::query(&format!(
                    "INSERT INTO {table_b} (tenant_id, region, value) VALUES ($1, $2, 'b')"
                ))
                .bind("tenant-b")
                .bind(region_b)
                .execute(conn)
                .await
                .map_err(|e| PgError::Query(format!("insert tenant B: {e}")))?;
                Ok(())
            })
        })
        .await
        .map_err(|e| format!("tenant B write: {e}"))?;

    let table_read = table.clone();
    let visible: Vec<String> = provider
        .with_tenant_tx("tenant-a", move |conn| {
            Box::pin(async move {
                sqlx::query_scalar(&format!("SELECT value FROM {table_read} ORDER BY value"))
                    .fetch_all(conn)
                    .await
                    .map_err(|e| PgError::Query(format!("read tenant A: {e}")))
            })
        })
        .await
        .map_err(|e| format!("tenant A read: {e}"))?;
    if visible != ["a"] {
        return Err(format!("tenant A saw unexpected rows: {visible:?}"));
    }

    provider.db_pool().close().await;
    Ok(())
}

#[tokio::test]
async fn bootstrap_migrates_then_hands_off_only_constrained_runtime() {
    let base = MyelinConfig::dev();
    let admin_pool = match PgPoolOptions::new()
        .max_connections(2)
        .connect(&base.database_migration_url)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("SKIP live split-credential bootstrap proof: {error}");
            return;
        }
    };

    let suffix = unique_suffix();
    let schema = format!("myelin_bootstrap_{suffix}");
    let result = exercise_bootstrap(&admin_pool, &schema, &suffix).await;
    let cleanup = sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin_pool)
        .await;
    admin_pool.close().await;

    if let Err(error) = cleanup {
        panic!("drop only isolated bootstrap schema: {error}");
    }
    if let Err(error) = result {
        panic!("split-credential bootstrap proof failed: {error}");
    }
}
