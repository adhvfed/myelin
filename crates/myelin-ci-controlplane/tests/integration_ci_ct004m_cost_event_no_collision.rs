#![cfg(feature = "integration")]

mod common;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_durable_migrations, verify_ci_cost_event_shape, CiCostEventStore, CiCostStoreError,
    CostEventRow, CostKind, Meter,
};
use myelin_flow::MicroUsd;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{all_durable_migrations, reserve_settle_durable_migrations, TenantScope};
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;
use sqlx::{Executor, PgPool, Row};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn schema_name() -> String {
    format!("ci_ct004m_{}", std::process::id())
}

fn verified_scope(tenant: &TenantId, region: &str) -> TenantScope {
    let mut principal = Principal::stub(
        PrincipalId("cost-store-test".into()),
        PrincipalKind::Service,
        tenant.clone(),
    );
    principal.region = Region(region.into());
    TenantScope::from_verified_token(&principal, principal.region.clone())
}

async fn pool() -> PgPool {
    let schema = schema_name();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? `fed test:backend`)")
}

fn uid(name: &str) -> Uuid {
    let mut bytes = [0u8; 16];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    bytes[..8].copy_from_slice(&h.to_be_bytes());
    let mut h2: u64 = h ^ 0x00ff_00ff_00ff_00ff;
    for b in name.bytes().rev() {
        h2 ^= b as u64;
        h2 = h2.wrapping_mul(0x0000_0100_0000_01b3);
    }
    bytes[8..].copy_from_slice(&h2.to_be_bytes());
    Uuid::from_bytes(bytes)
}

async fn columns(p: &PgPool, schema: &str, table: &str) -> Vec<String> {
    sqlx::query(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = $1 AND table_name = $2 ORDER BY column_name",
    )
    .bind(schema)
    .bind(table)
    .fetch_all(p)
    .await
    .unwrap()
    .iter()
    .map(|r| r.get::<String, _>("column_name"))
    .collect()
}

#[tokio::test]
async fn storage_cost_event_and_ci_cost_event_coexist_and_both_stores_write() {
    let aggregate = all_durable_migrations();
    let agg_ids: Vec<&str> = aggregate.0.iter().map(|m| m.id.as_ref()).collect();
    for m in reserve_settle_durable_migrations().0.iter() {
        assert!(
            agg_ids.contains(&m.id.as_ref()),
            "reserve_settle migration {} must be in all_durable_migrations()",
            m.id
        );
    }

    let schema = schema_name();
    let cleanup_pool = pool().await;
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_pool, &schema_for_cleanup, move || async move {
    let tenant = TenantId("acme".into());
    let region = "fr-par";
    let scope = verified_scope(&tenant, region);

    let p = pool().await;
    p.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop any prior schema");
    p.execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create the per-pid schema");

    for m in reserve_settle_durable_migrations().0.iter() {
        p.execute(m.ddl.as_ref())
            .await
            .unwrap_or_else(|e| panic!("apply Storage migration {}: {e}", m.id));
    }
    for m in ci_durable_migrations().0.iter() {
        p.execute(m.ddl.as_ref())
            .await
            .unwrap_or_else(|e| panic!("apply CI migration {}: {e}", m.id));
    }

    let storage_cols = columns(&p, &schema, "cost_event").await;
    let ci_cols = columns(&p, &schema, "ci_cost_event").await;
    assert!(
        !storage_cols.is_empty(),
        "Storage's money-ledger `cost_event` exists in the shared schema"
    );
    assert!(
        !ci_cols.is_empty(),
        "CI's projection `ci_cost_event` exists in the SAME schema (no collision no-op)"
    );
    assert!(
        storage_cols.contains(&"ord".to_string()) && storage_cols.contains(&"unit".to_string()),
        "Storage's cost_event carries its money-log columns (ord, unit): {storage_cols:?}"
    );
    assert!(
        !storage_cols.contains(&"cost_id".to_string())
            && !storage_cols.contains(&"job_id".to_string())
            && !storage_cols.contains(&"meter".to_string()),
        "Storage's cost_event does NOT carry CI's projection columns: {storage_cols:?}"
    );
    for c in [
        "cost_id",
        "job_id",
        "meter",
        "kind",
        "wholesale_minor_units",
    ] {
        assert!(
            ci_cols.contains(&c.to_string()),
            "CI's ci_cost_event carries `{c}`: {ci_cols:?}"
        );
    }
    assert!(
        !ci_cols.contains(&"ord".to_string()) && !ci_cols.contains(&"unit".to_string()),
        "CI's ci_cost_event does NOT carry Storage's money-log columns: {ci_cols:?}"
    );

    let run = uid("ct004m-run");
    let job = uid("ct004m-job");
    let store = CiCostEventStore::with_pg(p.clone(), Region(region.into()));
    let rows = vec![CostEventRow {
        tenant: tenant.clone(),
        run_id: run.to_string(),
        job_id: job.to_string(),
        meter: Meter::CpuSeconds,
        amount: 120,
        wholesale: MicroUsd(100),
        markup: MicroUsd(20),
        kind: CostKind::Ci,
    }];
    let affected = store
        .settle(&scope, &rows)
        .await
        .expect("the CT-004a settle records into ci_cost_event");
    assert_eq!(affected, 1, "one metered unit recorded into ci_cost_event");
    let read = store
        .cost_events_for_run(&scope, &run.to_string())
        .await
        .expect("read back the settled unit");
    assert_eq!(read.len(), 1, "the unit is in ci_cost_event");
    assert_eq!(read[0].meter, Meter::CpuSeconds);
    let storage_rows: i64 =
        sqlx::query("SELECT count(*)::bigint AS n FROM cost_event WHERE tenant_id = $1")
            .bind(tenant.as_str())
            .fetch_one(&p)
            .await
            .unwrap()
            .get("n");
    assert_eq!(
        storage_rows, 0,
        "the CI settle did NOT write Storage's money-ledger cost_event (no cross-write)"
    );

    let reserve_run = uid("ct004m-reserve-run");
    sqlx::query(
        "INSERT INTO ci_run (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
         definition_snapshot, trigger_kind, trust_tier, state, correlation_id) \
         VALUES ($1,$2,$3::uuid, gen_random_uuid(), gen_random_uuid(), gen_random_uuid(), \
         'blake3:snap', 'push', 'trusted', 'queued', 'corr-ct004m') \
         ON CONFLICT (tenant_id, run_id) DO NOTHING",
    )
    .bind(tenant.as_str())
    .bind(region)
    .bind(reserve_run.to_string())
    .execute(&p)
    .await
    .expect("the CT-004b ci_run reserve succeeds in the same DB");
    let runs: i64 = sqlx::query("SELECT count(*)::bigint AS n FROM ci_run WHERE run_id = $1::uuid")
        .bind(reserve_run.to_string())
        .fetch_one(&p)
        .await
        .unwrap()
        .get("n");
    assert_eq!(runs, 1, "the ci_run row is durable in the shared DB");

    verify_ci_cost_event_shape(&p)
        .await
        .expect("the correctly-migrated ci_cost_event passes the boot shape assertion");
    p.execute("DROP TABLE ci_cost_event")
        .await
        .expect("drop ci_cost_event for the wrong-shape leg");
    p.execute("CREATE TABLE ci_cost_event (tenant_id text, cost_id uuid, ord bigint, unit text)")
        .await
        .expect("create a wrong-shaped ci_cost_event (money-ledger-ish; missing the projection columns)");
    match verify_ci_cost_event_shape(&p).await {
        Err(CiCostStoreError::SchemaShapeMismatch { column, actual, .. }) => {
            assert_eq!(
                column, "region",
                "the first missing required column is surfaced"
            );
            assert_eq!(
                actual, "<absent>",
                "a missing column reads as <absent>, not silently accepted"
            );
        }
        other => {
            panic!("a wrong-shaped ci_cost_event must FAIL the boot shape assertion, got {other:?}")
        }
    }

    println!(
        "[CT-004m] PASS no-collision: Storage's cost_event (ord/unit) + CI's ci_cost_event \
         (cost_id/job_id/meter/kind) COEXIST in one shared schema; a CT-004a settle (→ ci_cost_event, \
         0 writes to cost_event) AND a CT-004b ci_run reserve BOTH succeed in the SAME DB"
    );
    })
    .await;
}
