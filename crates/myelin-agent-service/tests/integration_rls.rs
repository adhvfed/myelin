//! Live-Postgres integration test (Stage 1 / infra) — the Agent-Fabric `(tenant, region)` RLS
//! cross-tenant DENIAL proof (AG-P2 / P-131; the GATE: 0 cross-tenant rows readable).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding policy floor — no DB at build). This runs
//! ONLY against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-agent-service --features integration --test integration_rls -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the
//! same test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, that the Fabric's actual `run`-table migration DDL +
//! `myelin_make_tenant_scoped` RLS policy isolate rows end-to-end: a session set to tenant A reads
//! ONLY tenant A's run, and a session set to tenant B reads ONLY tenant B's — **0 cross-tenant rows
//! readable** (the §4 / EI-02 §1 no-cross-tenant-query-path invariant, enforced in Postgres). The
//! app role is `NOSUPERUSER NOBYPASSRLS`, so the policy is actually in force (a BYPASSRLS role would
//! silently ignore it).
#![cfg(feature = "integration")]

use myelin_agent_service::migrations::{rls_scope_sql, RUN_DDL};
use myelin_config::MyelinConfig;

#[tokio::test]
async fn agent_run_rls_denies_cross_tenant_reads() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    // The app role (NOSUPERUSER NOBYPASSRLS) — the role under which RLS is actually enforced.
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres as the app role (is the stack up?)");
    // The owner/migration role runs the DDL (production migrations run as the owner).
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&cfg.database_url.replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"))
        .await
        .expect("connect as admin");

    // A unique table name per process so concurrent runs don't collide — but the DDL is the REAL
    // run-table shape (we substitute the table name so cleanup is safe + parallel runs isolate).
    let tbl = format!("agent_run_rls_probe_{}", std::process::id());
    let create = RUN_DDL.replacen("agent_run", &tbl, 2); // CREATE TABLE name + PRIMARY KEY name.

    // Clean slate, then apply the REAL migration DDL + the REAL RLS-scope convention call.
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&admin).await.unwrap();
    sqlx::query(&create).execute(&admin).await.expect("the run-table DDL applies");
    sqlx::query(&rls_scope_sql(&tbl))
        .execute(&admin)
        .await
        .expect("myelin_make_tenant_scoped installs the (tenant_id, region) RLS policy");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app")).execute(&admin).await.unwrap();

    // Seed two tenants' runs (as admin, who is FORCEd under RLS too — so set the GUCs first).
    for (run_id, t) in [(1i64, "tenantA"), (2i64, "tenantB")] {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)").bind(t).execute(&mut *conn).await.unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)").execute(&mut *conn).await.unwrap();
        sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, run_id, agent_principal, on_behalf_of, binding_id, \
                trigger_event, correlation_id, causation_id, depth, runtime_ref, state, \
                reservation_id, budget, trace_ref) \
             VALUES ($1, 'fr-par', $2, 'psn:agent', 'psn:human', 0, 'evt', 'corr', 'cause', 0, \
                     'skeleton', 'running', 'rsv', 0, NULL)"
        ))
        .bind(t)
        .bind(run_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    // As the APP role set to tenant A: only tenant A's run is visible (RLS hides tenant B's).
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)").execute(&mut *conn).await.unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)").execute(&mut *conn).await.unwrap();

    let rows = sqlx::query(&format!("SELECT tenant_id FROM {tbl}")).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(rows.len(), 1, "RLS must hide the other tenant's run — 0 cross-tenant rows");
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");

    // The cross-tenant read is structurally 0: a tenant-A session counts ZERO tenant-B rows even
    // with an explicit predicate naming tenant B (the policy AND-s the GUCs — no path to B's rows).
    let cross: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {tbl} WHERE tenant_id = 'tenantB'"))
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(cross, 0, "a tenant-A session must read 0 cross-tenant (tenantB) rows");

    // And the mirror: a tenant-B session sees only tenant B's run.
    let mut conn_b = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantB', false)").execute(&mut *conn_b).await.unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)").execute(&mut *conn_b).await.unwrap();
    let rows_b = sqlx::query(&format!("SELECT tenant_id FROM {tbl}")).fetch_all(&mut *conn_b).await.unwrap();
    assert_eq!(rows_b.len(), 1, "tenant B sees only its own run");
    assert_eq!(rows_b[0].get::<String, _>("tenant_id"), "tenantB");

    sqlx::query(&format!("DROP TABLE {tbl}")).execute(&admin).await.unwrap();
}
