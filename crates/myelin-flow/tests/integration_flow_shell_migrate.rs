//! Live-Postgres integration test (Stage 1 / infra) — the myelin-flow AppSpec service shell's
//! MIGRATE phase applies the P-FLOW-01 six-table set against REAL Postgres (P-FLOW-02 / P-198).
//!
//! P-FLOW-02 wires the migrate-at-boot phase (`flow_app_spec(...).migrations`). The harness's
//! boot-time migration RUNNER is the M0 in-memory MODEL (a NAMED substrate floor, P-S15 — the
//! real driver lands there); so to honor the dev-real binding policy for the migrate path THIS
//! prompt wires, this test takes the EXACT migration set the AppSpec carries and applies every
//! DDL statement against the live dev-stack Postgres — proving the shell's migrate phase is real
//! DDL that Postgres accepts (the six `(tenant, region)`-first tables + their indexes + the
//! `myelin_make_tenant_scoped` RLS-scope calls), not just an in-memory string list.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build`/`cargo test
//! --workspace` stay DB-free (the binding-policy floor — no DB at build). Run ONLY against the
//! docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-flow --features integration --test integration_flow_shell_migrate -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam) — the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars, never a code change.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::OutboxStore;
use myelin_flow::{flow_app_spec, SERVICE_NAME};
use myelin_substrate::Config;

/// **The shell's migrate phase is REAL DDL Postgres accepts.** Take the migration set the
/// `flow_app_spec` AppSpec carries (the six P-FLOW-01 tables), apply every statement against live
/// Postgres as the admin/migration role into a per-process schema, and assert all six tables
/// exist afterward. This proves the boot→migrate phase the harness runs over this AppSpec is real
/// (the in-memory runner is the named P-S15 floor); a malformed DDL would fail HERE, loudly.
#[tokio::test]
async fn flow_shell_migration_set_applies_against_live_postgres() {
    let cfg = MyelinConfig::dev();
    // The owner/migration role runs the DDL (production migrations run as the owner).
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin (is the dev stack up?)");

    // The EXACT migration set the shell's AppSpec wires (no second schema — same `migrations()`).
    let spec = flow_app_spec(Config::default(), OutboxStore::new());
    assert_eq!(spec.name, SERVICE_NAME);
    assert_eq!(
        spec.migrations.0.len(),
        6,
        "the six-table P-FLOW-01 set the shell migrates over"
    );

    // A per-process schema so concurrent test runs isolate + cleanup is a single DROP SCHEMA. All
    // DDL + the existence checks run on ONE pinned connection (the search_path is connection-local).
    let schema = format!("flow_shell_probe_{}", std::process::id());
    let mut conn = admin.acquire().await.unwrap();
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&mut *conn)
        .await
        .unwrap();
    // Route unqualified table names into the probe schema; keep `public` so
    // `myelin_make_tenant_scoped` (defined in public by pg-init) resolves.
    sqlx::query(&format!("SET search_path TO {schema}, public"))
        .execute(&mut *conn)
        .await
        .unwrap();

    // Apply each migration's DDL (it is one-or-more `;`-separated statements: CREATE TABLE [+ index]
    // [+ RLS-scope call]) against live Postgres — exactly what boot's migrate phase runs.
    for migration in &spec.migrations.0 {
        for stmt in migration.ddl.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            sqlx::query(stmt)
                .execute(&mut *conn)
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "migration `{}` statement failed live: {e}\nSQL: {stmt}",
                        migration.id
                    )
                });
        }
    }

    // All six tables exist in the probe schema after the shell's migrate phase ran.
    for table in myelin_flow::migrations::TABLES {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = $2)",
        )
        .bind(&schema)
        .bind(table)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(
            exists,
            "the shell's migrate phase created `{table}` in Postgres"
        );
    }

    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&mut *conn)
        .await
        .unwrap();
}
