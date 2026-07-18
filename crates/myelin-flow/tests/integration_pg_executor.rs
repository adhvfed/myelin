//! Live-Postgres proof for the durable workflow control surface.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::MonotonicMinter;
use myelin_flow::{
    migrations::migrations, DurableExecutor, ExecutorError, PgFlowExecutor, RunId, SignalOutcome,
    SignalSpec, StartSpec,
};
use myelin_storage::{HotTables, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};
use std::sync::Arc;

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| MyelinConfig::dev().database_url)
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn schema_name() -> String {
    format!("flow_pg_executor_{}", std::process::id())
}

async fn open_pinned_pool() -> PgPool {
    let schema = schema_name();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
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
        .expect("connect to the dev Postgres admin endpoint")
}

fn executor(pool: &PgPool, tenant: &str) -> PgFlowExecutor {
    PgFlowExecutor::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        TenantId(tenant.into()),
        Region("fr-par".into()),
    )
}

fn start_spec(idem_key: &str) -> StartSpec {
    StartSpec {
        wf_type: "ci.pipeline".into(),
        input: vec![],
        budget: None,
        idem_key: idem_key.into(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_control_survives_restart_and_fails_closed_on_drift_and_cross_tenant_reads() {
    let bare = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .expect("connect for isolated schema setup");
    let schema = schema_name();
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&bare)
        .await
        .expect("drop stale probe schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&bare)
        .await
        .expect("create isolated probe schema");

    let pool = open_pinned_pool().await;
    PgMigrator::apply_validated(&pool, &migrations(), &HotTables::declare(["workflow_run"]))
        .await
        .expect("apply the real flow migrations");

    let first_process = executor(&pool, "acme");
    tokio::task::block_in_place(|| {
        first_process
            .register_definition("ci.pipeline", 1, "blake3:body-v1")
            .expect("register definition");
        first_process
            .register_definition("ci.pipeline", 1, "blake3:body-v1")
            .expect("exact registration is idempotent");
        assert!(matches!(
            first_process.register_definition("ci.pipeline", 1, "blake3:changed"),
            Err(ExecutorError::DefinitionDrift(_))
        ));
    });

    let run = tokio::task::block_in_place(|| {
        first_process
            .start(start_spec("trigger:event-1"))
            .expect("persist first start")
    });
    drop(first_process);

    // A fresh executor handle models a process restart: the idempotency row, definition, and run
    // state must come from Postgres, not a surviving Arc<Mutex<...>>.
    let restarted = executor(&pool, "acme");
    let replay = tokio::task::block_in_place(|| {
        restarted
            .start(start_spec("trigger:event-1"))
            .expect("restart returns the existing handle")
    });
    assert_eq!(replay, run);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_run WHERE tenant_id = $1 AND region = $2 AND idem_key = $3",
    )
    .bind("acme")
    .bind("fr-par")
    .bind("trigger:event-1")
    .fetch_one(&pool)
    .await
    .expect("count idempotent starts");
    assert_eq!(count, 1, "restart/re-delivery creates exactly one run");

    sqlx::query(
        "UPDATE workflow_run SET state = 'waiting' WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind("acme")
    .bind("fr-par")
    .bind(&run.0)
    .execute(&pool)
    .await
    .expect("park the run for the signal proof");
    let signal = || SignalSpec {
        run: run.clone(),
        signal_name: "job.done".into(),
        idem_key: "job-token-1".into(),
        payload: vec![],
        payload_key_ref: None,
    };
    assert_eq!(
        tokio::task::block_in_place(|| restarted.signal(signal())).expect("buffer signal"),
        SignalOutcome::Buffered
    );
    assert_eq!(
        tokio::task::block_in_place(|| restarted.signal(signal())).expect("dedup signal"),
        SignalOutcome::Duplicate
    );
    let status = tokio::task::block_in_place(|| restarted.describe(&run)).expect("describe wake");
    assert_eq!(
        status.state, "running",
        "signal insertion atomically wakes the run"
    );

    tokio::task::block_in_place(|| restarted.cancel(&run, "operator_requested"))
        .expect("persist cancellation");
    drop(restarted);
    let after_second_restart = executor(&pool, "acme");
    let status = tokio::task::block_in_place(|| after_second_restart.describe(&run))
        .expect("read cancellation after restart");
    assert!(status.terminal);
    assert_eq!(status.state, "terminated");
    tokio::task::block_in_place(|| after_second_restart.cancel(&run, "duplicate"))
        .expect("terminal cancellation is idempotent");

    let other_tenant = executor(&pool, "other");
    assert_eq!(
        tokio::task::block_in_place(|| other_tenant.describe(&RunId(run.0.clone())))
            .expect_err("explicit predicates deny a cross-tenant read"),
        ExecutorError::UnknownRun(run.0.clone())
    );

    drop(pool);
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&bare)
        .await
        .expect("clean isolated probe schema");
}
