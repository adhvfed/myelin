//! **CT-004m — the `cost_event` table-name collision is RESOLVED: Storage's money-ledger `cost_event`
//! and CI's metering projection `ci_cost_event` COEXIST in ONE shared `myelin` database.**
//!
//! The platform runs ONE shared Postgres for every service (docs/dev-stack.md — NOT "each service its
//! own DB"), so CI's tables land in the SAME database as Storage's. Before CT-004m both were named
//! `cost_event`; `CREATE TABLE IF NOT EXISTS cost_event` silently no-op'd whichever applied second,
//! leaving the loser's INSERT to fail at runtime on the missing columns. CT-004m renamed CI's table to
//! `ci_cost_event`. This test PROVES the fix end-to-end on live PG:
//!
//!   1. **Both migration sets apply to ONE schema.** Storage's `reserve_settle_durable_migrations()`
//!      (the piece of `all_durable_migrations()` that owns the colliding `cost_event`, migration
//!      `0050`) AND CI's `ci_durable_migrations()` (`ci_run` + `check_attempt` + `ci_cost_event`) both
//!      apply into the SAME schema — no `CREATE TABLE` swallows a differently-shaped sibling.
//!      (A DB-free structural assertion first confirms `reserve_settle_durable_migrations()` really is
//!      a subset of `all_durable_migrations()`, so "apply Storage's cost_event" == "apply the piece of
//!      the aggregate that ships it".)
//!   2. **Both tables exist with their DISTINCT columns.** Storage's `cost_event` carries `ord` + `unit`
//!      (the reserve-keyed money log); CI's `ci_cost_event` carries `cost_id` + `job_id` + `meter` +
//!      `kind` (the run/job-attributed projection). Neither has the other's columns.
//!   3. **A CT-004a settle AND a CT-004b reserve BOTH succeed in the SAME DB.** `CiCostEventStore.settle`
//!      records into `ci_cost_event`; a `ci_run` reserve insert succeeds into `ci_run`; Storage's
//!      `cost_event` is left untouched (no cross-write).
//!
//! Gated behind the `integration` cargo feature. Run against the docker-compose dev stack:
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004m_cost_event_no_collision -- --nocapture
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    ci_durable_migrations, verify_ci_cost_event_shape, CiCostEventStore, CiCostStoreError,
    CostEventRow, CostKind, Meter,
};
use myelin_flow::MinorUnits;
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

/// An admin pool pinned to the per-pid schema (with `public` in the path so the platform RLS helper
/// `myelin_make_tenant_scoped` — defined in `public` — resolves, while unqualified CREATEs land in the
/// per-pid schema first). Admin creates the isolated schema; store operations still carry verified
/// tenant/region scope and transaction-local GUCs.
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
        .expect("connect to dev Postgres as admin (is the stack up? eval \"$(scripts/dev-stack.sh env)\")")
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

/// The set of column names of a table in the per-pid schema.
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
    // ── (0) DB-FREE structural check: Storage's cost-ledger migration IS a subset of the aggregate. ──
    // So applying `reserve_settle_durable_migrations()` == applying the piece of `all_durable_migrations()`
    // that owns the colliding `cost_event` — the aggregate a real service main applies at boot.
    let agg_ids: Vec<&str> = all_durable_migrations().0.iter().map(|m| m.id).collect();
    for m in reserve_settle_durable_migrations().0.iter() {
        assert!(
            agg_ids.contains(&m.id),
            "reserve_settle migration {} must be in all_durable_migrations()",
            m.id
        );
    }

    let schema = schema_name();
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

    // ── (1) Apply BOTH migration sets into the SAME schema — no CREATE swallows a sibling. ──
    for m in reserve_settle_durable_migrations().0.iter() {
        p.execute(m.ddl)
            .await
            .unwrap_or_else(|e| panic!("apply Storage migration {}: {e}", m.id));
    }
    for m in ci_durable_migrations().0.iter() {
        p.execute(m.ddl)
            .await
            .unwrap_or_else(|e| panic!("apply CI migration {}: {e}", m.id));
    }

    // ── (2) BOTH tables exist with their DISTINCT columns. ──
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
    // Storage's cost_event: the reserve-keyed money log (ord + unit); NOT the CI projection columns.
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
    // CI's ci_cost_event: the run/job-attributed projection (cost_id, job_id, meter, kind).
    for c in ["cost_id", "job_id", "meter", "kind", "wholesale_minor_units"] {
        assert!(
            ci_cols.contains(&c.to_string()),
            "CI's ci_cost_event carries `{c}`: {ci_cols:?}"
        );
    }
    assert!(
        !ci_cols.contains(&"ord".to_string()) && !ci_cols.contains(&"unit".to_string()),
        "CI's ci_cost_event does NOT carry Storage's money-log columns: {ci_cols:?}"
    );

    // ── (3a) A CT-004a settle records into ci_cost_event (NOT Storage's cost_event). ──
    let run = uid("ct004m-run");
    let job = uid("ct004m-job");
    let store = CiCostEventStore::with_pg(p.clone(), Region(region.into()));
    let rows = vec![CostEventRow {
        tenant: tenant.clone(),
        run_id: run.to_string(),
        job_id: job.to_string(),
        meter: Meter::CpuSeconds,
        amount: 120,
        wholesale: MinorUnits(100),
        markup: MinorUnits(20),
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
    // Storage's cost_event was NOT written by the CI settle.
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

    // ── (3b) A CT-004b-style ci_run reserve succeeds in the SAME DB. ──
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

    // ── (4) #11 — the BOOT-TIME SHAPE ASSERTION. The correctly-migrated ci_cost_event PASSES. ──
    verify_ci_cost_event_shape(&p)
        .await
        .expect("the correctly-migrated ci_cost_event passes the boot shape assertion");
    // A WRONG-shaped ci_cost_event (the pre-CT-004m hazard: a table bound under the CI name but NOT the
    // metering-projection shape) is REFUSED LOUDLY — the money table is never written wrong-shaped.
    // (Done last: ci_cost_event is no longer needed, and the schema is dropped below.)
    p.execute("DROP TABLE ci_cost_event")
        .await
        .expect("drop ci_cost_event for the wrong-shape leg");
    p.execute("CREATE TABLE ci_cost_event (tenant_id text, cost_id uuid, ord bigint, unit text)")
        .await
        .expect("create a wrong-shaped ci_cost_event (money-ledger-ish; missing the projection columns)");
    match verify_ci_cost_event_shape(&p).await {
        Err(CiCostStoreError::SchemaShapeMismatch { column, actual, .. }) => {
            assert_eq!(column, "region", "the first missing required column is surfaced");
            assert_eq!(actual, "<absent>", "a missing column reads as <absent>, not silently accepted");
        }
        other => panic!("a wrong-shaped ci_cost_event must FAIL the boot shape assertion, got {other:?}"),
    }

    // ── Cleanup. ──
    p.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .ok();
    println!(
        "[CT-004m] PASS no-collision: Storage's cost_event (ord/unit) + CI's ci_cost_event \
         (cost_id/job_id/meter/kind) COEXIST in one shared schema; a CT-004a settle (→ ci_cost_event, \
         0 writes to cost_event) AND a CT-004b ci_run reserve BOTH succeed in the SAME DB"
    );
}
