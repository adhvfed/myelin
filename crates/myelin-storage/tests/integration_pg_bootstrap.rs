#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::migration::{HotTables, Migration, Migrations};
use myelin_storage::{IndexReadinessSpec, PgBootstrap, PgError};
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

fn ordinary_btree<'a>(
    index: &'a str,
    table: &'a str,
    keys: &'a [&'a str],
    predicate: Option<&'a str>,
) -> IndexReadinessSpec<'a> {
    IndexReadinessSpec::new(index, table, "r", "i", "btree", keys, predicate)
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

    let index = format!("bootstrap_exact_{suffix}");
    let index_ddl: &'static str = Box::leak(
        format!(
            "CREATE INDEX CONCURRENTLY {index} ON {table} \
             (tenant_id, region, value DESC) WHERE value <> ''"
        )
        .into_boxed_str(),
    );
    let index_migration_id: &'static str =
        Box::leak(format!("9001_pg_bootstrap_index_{suffix}").into_boxed_str());
    bootstrap
        .migrate(
            &Migrations::of([Migration::plain(index_migration_id, index_ddl)]),
            &HotTables::none(),
        )
        .await
        .map_err(|e| format!("migrate isolated exact index: {e}"))?;
    let keys = ["tenant_id", "region", "value DESC"];
    bootstrap
        .verify_index_ready_exact(ordinary_btree(
            &index,
            &table,
            &keys,
            Some("(value <> ''::text)"),
        ))
        .await
        .map_err(|e| format!("verify exact index identity: {e}"))?;
    for wrong in [
        ordinary_btree(&index, "wrong_table", &keys, Some("(value <> ''::text)")),
        ordinary_btree(
            &index,
            &table,
            &["tenant_id", "region", "value"],
            Some("(value <> ''::text)"),
        ),
        ordinary_btree(&index, &table, &keys, None),
    ] {
        if bootstrap.verify_index_ready_exact(wrong).await.is_ok() {
            return Err(format!(
                "exact readiness admitted wrong index identity: {wrong:?}"
            ));
        }
    }

    let other_schema = format!("{schema}_other");
    let other_table = format!("bootstrap_other_{suffix}");
    let other_index = format!("bootstrap_other_index_{suffix}");
    let order_index = format!("bootstrap_order_{suffix}");
    let nulls_index = format!("bootstrap_nulls_{suffix}");
    let predicate_index = format!("bootstrap_predicate_{suffix}");
    let hash_index = format!("bootstrap_hash_{suffix}");
    let materialized = format!("bootstrap_materialized_{suffix}");
    let materialized_index = format!("bootstrap_materialized_index_{suffix}");
    let partitioned = format!("bootstrap_partitioned_{suffix}");
    let partitioned_index = format!("bootstrap_partitioned_index_{suffix}");
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {other_schema} AUTHORIZATION myelin_admin;
         CREATE TABLE {other_schema}.{other_table} (
             tenant_id text NOT NULL,
             region text NOT NULL,
             value text NOT NULL
         );
         CREATE INDEX {other_index} ON {other_schema}.{other_table}
             (tenant_id, region, value DESC) WHERE value <> '';
         CREATE INDEX {order_index} ON {schema}.{table}
             (region, tenant_id, value DESC) WHERE value <> '';
         CREATE INDEX {nulls_index} ON {schema}.{table}
             (tenant_id, region, value DESC NULLS LAST) WHERE value <> '';
         CREATE INDEX {predicate_index} ON {schema}.{table}
             (tenant_id, region, value DESC) WHERE value <> 'different';
         CREATE INDEX {hash_index} ON {schema}.{table} USING hash (value);
         CREATE MATERIALIZED VIEW {schema}.{materialized} AS
             SELECT tenant_id, region, value FROM {schema}.{table};
         CREATE INDEX {materialized_index} ON {schema}.{materialized}
             (tenant_id, region, value DESC) WHERE value <> '';
         CREATE TABLE {schema}.{partitioned} (
             tenant_id text NOT NULL,
             region text NOT NULL,
             value text NOT NULL
         ) PARTITION BY LIST (tenant_id);
         CREATE INDEX {partitioned_index} ON {schema}.{partitioned}
             (tenant_id, region, value DESC) WHERE value <> '';"
    ))
    .execute(admin_pool)
    .await
    .map_err(|e| format!("create exact-readiness negative fixtures: {e}"))?;

    for (label, wrong) in [
        (
            "wrong schema",
            ordinary_btree(
                &other_index,
                &other_table,
                &keys,
                Some("(value <> ''::text)"),
            ),
        ),
        (
            "wrong key order",
            ordinary_btree(&order_index, &table, &keys, Some("(value <> ''::text)")),
        ),
        (
            "wrong null order",
            ordinary_btree(&nulls_index, &table, &keys, Some("(value <> ''::text)")),
        ),
        (
            "different predicate",
            ordinary_btree(&predicate_index, &table, &keys, Some("(value <> ''::text)")),
        ),
        (
            "wrong access method",
            ordinary_btree(&hash_index, &table, &["value"], None),
        ),
        (
            "materialized-view table kind",
            ordinary_btree(
                &materialized_index,
                &materialized,
                &keys,
                Some("(value <> ''::text)"),
            ),
        ),
        (
            "partitioned table/index kind",
            ordinary_btree(
                &partitioned_index,
                &partitioned,
                &keys,
                Some("(value <> ''::text)"),
            ),
        ),
    ] {
        if bootstrap.verify_index_ready_exact(wrong).await.is_ok() {
            return Err(format!(
                "exact readiness admitted the physical {label} fixture: {wrong:?}"
            ));
        }
    }

    sqlx::query(
        "UPDATE pg_index
            SET indisvalid = false
          WHERE indexrelid = to_regclass($1)",
    )
    .bind(format!("{schema}.{index}"))
    .execute(admin_pool)
    .await
    .map_err(|e| format!("mark exact-readiness fixture invalid: {e}"))?;
    if bootstrap
        .verify_index_ready_exact(ordinary_btree(
            &index,
            &table,
            &keys,
            Some("(value <> ''::text)"),
        ))
        .await
        .is_ok()
    {
        return Err("exact readiness admitted an invalid index".into());
    }
    sqlx::query(
        "UPDATE pg_index
            SET indisvalid = true, indisready = false
          WHERE indexrelid = to_regclass($1)",
    )
    .bind(format!("{schema}.{index}"))
    .execute(admin_pool)
    .await
    .map_err(|e| format!("mark exact-readiness fixture unready: {e}"))?;
    if bootstrap
        .verify_index_ready_exact(ordinary_btree(
            &index,
            &table,
            &keys,
            Some("(value <> ''::text)"),
        ))
        .await
        .is_ok()
    {
        return Err("exact readiness admitted an unready index".into());
    }
    sqlx::query(
        "UPDATE pg_index
            SET indisvalid = true, indisready = true, indislive = false
          WHERE indexrelid = to_regclass($1)",
    )
    .bind(format!("{schema}.{index}"))
    .execute(admin_pool)
    .await
    .map_err(|e| format!("mark exact-readiness fixture non-live: {e}"))?;
    if bootstrap
        .verify_index_ready_exact(ordinary_btree(
            &index,
            &table,
            &keys,
            Some("(value <> ''::text)"),
        ))
        .await
        .is_ok()
    {
        return Err("exact readiness admitted a non-live index".into());
    }
    sqlx::query(
        "UPDATE pg_index
            SET indislive = true, indcheckxmin = true
          WHERE indexrelid = to_regclass($1)",
    )
    .bind(format!("{schema}.{index}"))
    .execute(admin_pool)
    .await
    .map_err(|e| format!("mark exact-readiness fixture xmin-blocked: {e}"))?;
    if bootstrap
        .verify_index_ready_exact(ordinary_btree(
            &index,
            &table,
            &keys,
            Some("(value <> ''::text)"),
        ))
        .await
        .is_ok()
    {
        return Err("exact readiness admitted an xmin-blocked index".into());
    }
    sqlx::query(
        "UPDATE pg_index
            SET indisvalid = true,
                indisready = true,
                indislive = true,
                indcheckxmin = false
          WHERE indexrelid = to_regclass($1)",
    )
    .bind(format!("{schema}.{index}"))
    .execute(admin_pool)
    .await
    .map_err(|e| format!("restore exact-readiness fixture state: {e}"))?;

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
