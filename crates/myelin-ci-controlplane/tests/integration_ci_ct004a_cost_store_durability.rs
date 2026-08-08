#![cfg(feature = "integration")]

mod common;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_durable_migrations, CiCostEventStore, CiCostStoreError, CostEventRow, CostKind, Meter,
};
use myelin_flow::MicroUsd;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;
use sqlx::{Executor, PgPool};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn schema_name() -> String {
    format!("ci_ct004a_{}", std::process::id())
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

async fn reopen() -> PgPool {
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
        .expect("reconnect to dev Postgres as admin (is the stack up? `fed test:backend`)")
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

fn settle_rows(tenant: &TenantId, run: Uuid, job: Uuid) -> Vec<CostEventRow> {
    vec![
        CostEventRow {
            tenant: tenant.clone(),
            run_id: run.to_string(),
            job_id: job.to_string(),
            meter: Meter::CpuSeconds,
            amount: 120,
            wholesale: MicroUsd(100),
            markup: MicroUsd(20),
            kind: CostKind::Ci,
        },
        CostEventRow {
            tenant: tenant.clone(),
            run_id: run.to_string(),
            job_id: job.to_string(),
            meter: Meter::MemGbSeconds,
            amount: 4096,
            wholesale: MicroUsd(50),
            markup: MicroUsd(10),
            kind: CostKind::Ci,
        },
    ]
}

#[tokio::test]
async fn cost_store_settle_survives_kill9_no_ghost_no_double_bill() {
    let schema = schema_name();
    let cleanup_pool = reopen().await;
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_pool, &schema_for_cleanup, move || async move {
    let tenant = TenantId("acme".into());
    let region = "fr-par";
    let scope = verified_scope(&tenant, region);
    let run = uid("ct004a-run-ok");
    let job = uid("ct004a-job-ok");
    let crash_run = uid("ct004a-run-crash");
    let crash_job = uid("ct004a-job-crash");

    let p1 = reopen().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&p1)
        .await
        .expect("drop any prior schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&p1)
        .await
        .expect("create the per-pid schema");
    for m in ci_durable_migrations().0.iter() {
        p1.execute(m.ddl)
            .await
            .unwrap_or_else(|e| panic!("apply CI durable migration {} into the schema: {e}", m.id));
    }

    let store1 = CiCostEventStore::with_pg(p1.clone(), Region(region.into()));
    let rows = settle_rows(&tenant, run, job);
    let affected = store1
        .settle(&scope, &rows)
        .await
        .expect("the committed settle records the metered units");
    assert_eq!(
        affected, 2,
        "two metered units recorded (cost_events_per_unit == 1 each)"
    );

    let redelivered = store1
        .settle(&scope, &rows)
        .await
        .expect("a re-delivered settle is a no-op success");
    assert_eq!(
        redelivered, 0,
        "a doubly-delivered settle records ONCE (double-effect = 0 - the bookends do not double-bill)"
    );

    let mut divergent = settle_rows(&tenant, run, job);
    divergent[0].amount = 999;
    let err = store1
        .settle(&scope, &divergent)
        .await
        .expect_err("a divergent re-delivered settle must be refused, never silently dropped");
    match err {
        CiCostStoreError::AmountDivergence {
            column,
            recorded,
            incoming,
        } => {
            assert_eq!(column, "amount");
            assert_eq!(
                recorded, 120,
                "the FIRST settle's amount is preserved (never overwritten)"
            );
            assert_eq!(incoming, 999, "the divergent incoming amount is surfaced");
        }
        other => panic!("expected AmountDivergence, got {other}"),
    }
    let after = store1
        .cost_events_for_run(&scope, &run.to_string())
        .await
        .expect("read back after the refused divergent settle");
    let cpu = after
        .iter()
        .find(|r| r.meter == Meter::CpuSeconds)
        .expect("the CpuSeconds unit");
    assert_eq!(
        cpu.amount, 120,
        "the divergent settle was refused - the recorded amount is unchanged"
    );

    {
        let mut tx = p1.begin().await.unwrap();
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true), \
                    set_config('myelin.region', $2, true)",
        )
        .bind(scope.tenant().as_str())
        .bind(&scope.region().0)
        .execute(&mut *tx)
        .await
        .expect("scope the caller-owned co-commit transaction");
        let crash_rows = settle_rows(&tenant, crash_run, crash_job);
        let n = store1
            .settle_in_tx(&mut tx, &scope, &crash_rows)
            .await
            .expect("the in-tx settle inserts (uncommitted)");
        assert_eq!(n, 2, "the in-tx settle wrote its rows (pending commit)");
        drop(tx);
    }

    drop(store1);
    drop(p1);

    let p2 = reopen().await;
    let store2 = CiCostEventStore::with_pg(p2.clone(), Region(region.into()));

    let read = store2
        .cost_events_for_run(&scope, &run.to_string())
        .await
        .expect("read back the settled units after reopen");
    assert_eq!(
        read.len(),
        2,
        "both committed metered units survive kill-9/reopen (durable, not in-memory)"
    );
    assert_eq!(read[0].meter, Meter::CpuSeconds);
    assert_eq!(
        read[0].job_id,
        job.to_string(),
        "attributed to its (run, job)"
    );
    assert_eq!(read[0].wholesale, MicroUsd(100));
    assert_eq!(read[0].markup, MicroUsd(20));
    assert_ne!(
        read[0].wholesale, read[0].markup,
        "wholesale ≠ markup (the two cost columns are distinct, arch 02 §8)"
    );
    assert_eq!(
        read[0].kind,
        CostKind::Ci,
        "settled kind stays ci after reopen"
    );
    assert_eq!(read[1].meter, Meter::MemGbSeconds);
    assert_eq!(read[1].wholesale, MicroUsd(50));
    assert_eq!(read[1].markup, MicroUsd(10));

    let ghost = store2
        .cost_events_for_run(&scope, &crash_run.to_string())
        .await
        .expect("read back the crashed run after reopen");
    assert!(
        ghost.is_empty(),
        "the uncommitted settle left NO ghost cost row (no half-billed run - all-or-nothing)"
    );

    println!(
        "[CT-004a] PASS cost_store: committed settle survives kill-9/reopen (2 units, attributed to \
         (run,job), wholesale≠markup, kind=ci); re-delivered settle → 0 rows (double-effect = 0, no \
         double-bill); uncommitted settle-in-tx → 0 ghost rows (all-or-nothing) - proven THROUGH the store"
    );
    })
    .await;
}
