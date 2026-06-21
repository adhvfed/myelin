//! Live-Postgres integration test (Stage 1 / infra) — the myelin-flow data-model `(tenant, region)`
//! RLS cross-tenant DENIAL + the load-bearing idempotency constraints, proven against REAL Postgres
//! (P-FLOW-01 / P-197; the GATE: 0 cross-tenant rows readable; the journal/signal idempotency
//! UNIQUE/PK bite).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs
//! ONLY against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-flow --features integration --test integration_flow_schema -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, that:
//!   1. The flow `workflow_run` migration DDL + `myelin_make_tenant_scoped` RLS policy isolate rows
//!      end-to-end: a session set to tenant A reads ONLY tenant A's run — **0 cross-tenant rows
//!      readable** (the §3 / EI-02 §1 no-cross-tenant-query-path invariant, in Postgres). The app
//!      role is `NOSUPERUSER NOBYPASSRLS`, so the policy is actually in force.
//!   2. The `wf_history` `UNIQUE(tenant_id, run_id, command_id)` journaling-idempotency key BITES —
//!      a second journal row with the same `(run_id, command_id)` is rejected by Postgres (the
//!      replay-safe journal is a real constraint, §3.2, not a convention — the silent-data-loss
//!      floor: a crash between "do the activity" and "journal its result" replays safely).
//!   3. The `wf_signal` PK `(tenant_id, run_id, signal_name, idem_key)` BITES — a re-delivered signal
//!      with the same key is rejected by Postgres (the per-effect idempotency anchor, §3.4 / §6.4).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_flow::migrations::{rls_scope_sql, WF_HISTORY_DDL, WF_SIGNAL_DDL, WORKFLOW_RUN_DDL};

#[tokio::test]
async fn flow_workflow_run_rls_denies_cross_tenant_and_idempotency_keys_bite() {
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

    // Unique table names per process so concurrent runs don't collide — the DDL is the REAL shape
    // (we substitute the table name so cleanup is safe + parallel runs isolate).
    let pid = std::process::id();
    let run_tbl = format!("workflow_run_probe_{pid}");
    let hist_tbl = format!("wf_history_probe_{pid}");
    let sig_tbl = format!("wf_signal_probe_{pid}");

    let run_create = WORKFLOW_RUN_DDL.replacen("workflow_run", &run_tbl, 1);
    let hist_create = WF_HISTORY_DDL.replacen("wf_history", &hist_tbl, 1);
    let sig_create = WF_SIGNAL_DDL.replacen("wf_signal", &sig_tbl, 1);

    for tbl in [&run_tbl, &hist_tbl, &sig_tbl] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&admin).await.unwrap();
    }
    sqlx::query(&run_create).execute(&admin).await.expect("the workflow_run DDL applies");
    sqlx::query(&hist_create).execute(&admin).await.expect("the wf_history DDL applies");
    sqlx::query(&sig_create).execute(&admin).await.expect("the wf_signal DDL applies");
    for tbl in [&run_tbl, &hist_tbl, &sig_tbl] {
        sqlx::query(&rls_scope_sql(tbl))
            .execute(&admin)
            .await
            .expect("myelin_make_tenant_scoped installs the (tenant_id, region) RLS policy");
        sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app")).execute(&admin).await.unwrap();
    }

    // (1) Seed two tenants' runs (as admin, who is FORCEd under RLS too — set the GUCs first).
    for (run_id, t) in [("run-A", "tenantA"), ("run-B", "tenantB")] {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)").bind(t).execute(&mut *conn).await.unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)").execute(&mut *conn).await.unwrap();
        sqlx::query(&format!(
            "INSERT INTO {run_tbl} \
               (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, \
                correlation_id, depth, partition) \
             VALUES ($1, 'fr-par', $2, 'agent.run', 1, '[]'::jsonb, 'running', 0, 'corr-1', 0, 3)"
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

    let rows = sqlx::query(&format!("SELECT tenant_id FROM {run_tbl}")).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(rows.len(), 1, "RLS must hide the other tenant's run — 0 cross-tenant rows");
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");

    let cross: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {run_tbl} WHERE tenant_id = 'tenantB'"))
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(cross, 0, "a tenant-A session must read 0 cross-tenant (tenantB) rows");

    // (2) The wf_history journaling-idempotency UNIQUE bites: a second journal row with the SAME
    // (run_id, command_id) under tenant A is rejected by Postgres (the replay-safe journal, §3.2).
    let hist_insert = |seq: i64| {
        format!(
            "INSERT INTO {hist_tbl} \
               (tenant_id, region, run_id, seq, kind, command_id, result) \
             VALUES ('tenantA', 'fr-par', 'run-A', {seq}, 'activity_completed', 'agent.run:0', '[]'::jsonb)"
        )
    };
    sqlx::query(&hist_insert(1)).execute(&mut *conn).await.expect("the first journal row inserts");
    let dup_hist = sqlx::query(&hist_insert(2)).execute(&mut *conn).await;
    assert!(
        dup_hist.is_err(),
        "a duplicate (run_id, command_id) must be REJECTED by UNIQUE(tenant_id, run_id, command_id) — the replay-safe journal (§3.2)"
    );

    // (3) The wf_signal PK (tenant, run_id, signal_name, idem_key) bites: a re-delivered signal with
    // the same key is rejected (the per-effect idempotency anchor, §3.4 / §6.4).
    let sig_insert = "INSERT INTO {TBL} \
           (tenant_id, region, run_id, signal_name, idem_key, payload) \
         VALUES ('tenantA', 'fr-par', 'run-A', 'job.done', 'tok-1', '[]'::jsonb)";
    let sig_insert = sig_insert.replace("{TBL}", &sig_tbl);
    sqlx::query(&sig_insert).execute(&mut *conn).await.expect("the first signal inserts");
    let dup_sig = sqlx::query(&sig_insert).execute(&mut *conn).await;
    assert!(
        dup_sig.is_err(),
        "a re-delivered signal (same idem_key) must be REJECTED by PK(tenant, run_id, signal_name, idem_key) (§3.4)"
    );

    for tbl in [&run_tbl, &hist_tbl, &sig_tbl] {
        sqlx::query(&format!("DROP TABLE {tbl}")).execute(&admin).await.unwrap();
    }
}
