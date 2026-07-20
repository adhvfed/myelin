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

/// Split a migration's DDL into top-level statements on `;`, but NOT on a `;` that sits inside a
/// PostgreSQL dollar-quoted body (`$tag$ … $tag$`). The concurrent-index VALIDATION migration is a
/// lone `DO $myelin$ … $myelin$` block whose body contains semicolons; a naive `split(';')` would
/// shred it into unterminated fragments. (Production `PgMigrator` already parses dollar-quoting; this
/// mirrors it for the test's per-statement executor, which the extended protocol requires so a lone
/// `CREATE INDEX CONCURRENTLY` runs on its own.)
fn split_sql_statements(ddl: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut rest = ddl;
    let mut in_tag: Option<String> = None;
    while let Some(ch) = rest.chars().next() {
        if let Some(tag) = &in_tag {
            if rest.starts_with(tag.as_str()) {
                current.push_str(tag);
                rest = &rest[tag.len()..];
                in_tag = None;
                continue;
            }
            current.push(ch);
            rest = &rest[ch.len_utf8()..];
        } else if ch == '$' {
            // A dollar tag is `$` + [A-Za-z0-9_]* + `$`.
            if let Some(close) = rest[1..].find('$') {
                let body = &rest[1..1 + close];
                if body.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    let tag = format!("${body}$");
                    current.push_str(&tag);
                    rest = &rest[tag.len()..];
                    in_tag = Some(tag);
                    continue;
                }
            }
            current.push('$');
            rest = &rest[1..];
        } else if ch == ';' {
            out.push(std::mem::take(&mut current));
            rest = &rest[1..];
        } else {
            current.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

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
        12,
        "six table creates plus six online workflow control/drive/repair expands (incl. the concurrent-index validation)"
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
    // [+ RLS-scope call], or a lone `DO $tag$…$tag$` catalog check) against live Postgres — exactly
    // what boot's migrate phase runs. The split is DOLLAR-QUOTE-AWARE so a `;` INSIDE a `$tag$…$tag$`
    // body (the concurrent-index validation DO block) is not a statement boundary.
    for migration in &spec.migrations.0 {
        for stmt in split_sql_statements(migration.ddl) {
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
