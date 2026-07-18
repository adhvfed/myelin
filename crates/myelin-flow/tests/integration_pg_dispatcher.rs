//! Live PostgreSQL proofs for the production drive adapter.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::{Actor, IdMinter, MonotonicMinter, OutboxStore};
use myelin_flow::{
    boot_flow, migrations::migrations, PgFlowWorker, PgRunOnceOutcome, PgWorkerScope, RetryPolicy,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{provider::foundation_migrations, HotTables, PgMigrator, PgOutboxBacking};
use myelin_substrate::Config;
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| MyelinConfig::dev().database_url)
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

async fn setup(label: &str) -> (PgPool, PgPool, String) {
    let bare = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("connect to live PostgreSQL");
    let n = SCHEMA_SEQ.fetch_add(1, Ordering::Relaxed);
    let schema = format!("flow_dispatch_{label}_{}_{}", std::process::id(), n);
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&bare)
        .await
        .expect("drop stale schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&bare)
        .await
        .expect("create schema");
    let pinned = schema.clone();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(16)
        .after_connect(move |conn, _| {
            let pinned = pinned.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {pinned}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("connect schema-pinned pool");
    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .expect("apply outbox foundation");
    PgMigrator::apply_validated(&pool, &migrations(), &HotTables::declare(["workflow_run"]))
        .await
        .expect("apply flow migrations");
    (bare, pool, schema)
}

async fn cleanup(bare: PgPool, pool: PgPool, schema: String) {
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&bare)
        .await
        .expect("drop schema");
}

fn actor(tenant: &str, region: &str) -> Actor {
    Actor(Principal::new(
        TenantId(tenant.into()),
        Region(region.into()),
        PrincipalId("svc:flow-test".into()),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    ))
}

fn worker(pool: &PgPool, tenant: &str, region: &str, partition: i16, name: &str) -> PgFlowWorker {
    worker_with_ttl(pool, tenant, region, partition, name, 60)
}

fn worker_with_ttl(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    partition: i16,
    name: &str,
    lease_ttl_secs: i64,
) -> PgFlowWorker {
    let scope = PgWorkerScope::new(
        TenantId(tenant.into()),
        Region(region.into()),
        partition,
        name,
        lease_ttl_secs,
        actor(tenant, region),
        1,
    )
    .expect("valid exact worker scope");
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    PgFlowWorker::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        minter,
        scope,
    )
}

async fn seed_run(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    run_id: &str,
    wf_type: &str,
    cursor: i64,
    partition: i16,
) {
    sqlx::query(
        "INSERT INTO workflow_run \
           (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, budget, \
            correlation_id, causation_id, caused_by, depth, partition, idem_key) \
         VALUES ($1,$2,$3,$4,1,'[]'::jsonb,'running',$5,NULL,$3,NULL,NULL,0,$6,$3)",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(wf_type)
    .bind(cursor)
    .bind(partition)
    .execute(pool)
    .await
    .expect("seed run");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restarted_worker_replays_history_without_reexecuting_and_releases_terminal_lease() {
    let _guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("restart").await;
    let service = boot_flow(
        Config::default(),
        OutboxStore::durable(Arc::new(PgOutboxBacking::new(
            pool.clone(),
            tokio::runtime::Handle::current(),
        ))),
    )
    .expect("boot service surfaces before worker drive");
    let executions = Arc::new(AtomicUsize::new(0));
    let mut first_process = worker(&pool, "acme", "no-osl", 2, "worker-before-restart");
    let observed = Arc::clone(&executions);
    first_process
        .register_definition("wf.recover", 1, "recover-v1", move |input, ctx| {
            assert_eq!(input.run_id, "R-recover");
            assert_eq!(input.wf_type, "wf.recover");
            assert_eq!(input.wf_version, 1);
            assert!(input.input.is_empty());
            ctx.activity(RetryPolicy { max_attempts: 1 }, |_, _| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            })
            .map_err(|error| format!("{error:?}"))?;
            Ok(Vec::new())
        })
        .expect("register first process body");
    seed_run(&pool, "acme", "no-osl", "R-recover", "wf.recover", 1, 2).await;
    sqlx::query(
        "INSERT INTO wf_history \
           (tenant_id,region,run_id,seq,kind,command_id,result) \
         VALUES ('acme','no-osl','R-recover',0,'activity_completed','wf.recover:0','[]'::jsonb)",
    )
    .execute(&pool)
    .await
    .unwrap();
    drop(first_process);

    let mut restarted = worker(&pool, "acme", "no-osl", 2, "worker-after-restart");
    let observed = Arc::clone(&executions);
    restarted
        .register_definition("wf.recover", 1, "recover-v1", move |input, ctx| {
            assert_eq!(input.run_id, "R-recover");
            assert_eq!(input.wf_type, "wf.recover");
            assert_eq!(input.wf_version, 1);
            assert!(input.input.is_empty());
            ctx.activity(RetryPolicy { max_attempts: 1 }, |_, _| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            })
            .map_err(|error| format!("{error:?}"))?;
            Ok(Vec::new())
        })
        .expect("register restarted body");
    assert!(matches!(
        restarted
            .run_once(1_752_796_800, "2026-07-18T00:00:00Z")
            .await
            .unwrap(),
        PgRunOnceOutcome::Driven { .. }
    ));
    assert_eq!(
        executions.load(Ordering::SeqCst),
        0,
        "journal replay ran no side effect"
    );
    let row = sqlx::query(
        "SELECT state, cursor, lease_owner, lease_expires FROM workflow_run \
         WHERE tenant_id='acme' AND region='no-osl' AND run_id='R-recover'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("state"), "completed");
    assert_eq!(row.get::<i64, _>("cursor"), 1);
    assert!(row.get::<Option<String>, _>("lease_owner").is_none());
    assert!(row
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("lease_expires")
        .is_none());
    service.signal_drain();
    let telemetry = service.drain();
    assert_eq!(
        telemetry.outbox_depth(),
        0,
        "graceful shutdown drained cleanly"
    );
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_skip_locked_drive_once_and_never_cross_tenant_or_region() {
    let _guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("workers").await;
    let mut one = worker(&pool, "acme", "no-osl", 4, "worker-one");
    let mut two = worker(&pool, "acme", "no-osl", 4, "worker-two");
    one.register_definition("wf.once", 1, "once-v1", |_input, ctx| {
        ctx.now();
        Ok(Vec::new())
    })
    .unwrap();
    two.register_definition("wf.once", 1, "once-v1", |_input, ctx| {
        ctx.now();
        Ok(Vec::new())
    })
    .unwrap();
    for run_id in ["R-one", "R-two"] {
        seed_run(&pool, "acme", "no-osl", run_id, "wf.once", 0, 4).await;
    }
    seed_run(&pool, "other", "no-osl", "R-other", "wf.once", 0, 4).await;
    seed_run(&pool, "acme", "se-sto", "R-region", "wf.once", 0, 4).await;

    let (left, right) = tokio::join!(
        one.run_once(1_752_796_800, "2026-07-18T00:00:00Z"),
        two.run_once(1_752_796_800, "2026-07-18T00:00:00Z")
    );
    assert!(matches!(left.unwrap(), PgRunOnceOutcome::Driven { .. }));
    assert!(matches!(right.unwrap(), PgRunOnceOutcome::Driven { .. }));
    let completed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_run \
         WHERE tenant_id='acme' AND region='no-osl' AND state='completed' AND cursor=1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completed, 2, "each exact-scope run was driven once");
    let history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='no-osl'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(history, 2, "SKIP LOCKED produced no duplicate drive");
    for (tenant, region, run_id) in [
        ("other", "no-osl", "R-other"),
        ("acme", "se-sto", "R-region"),
    ] {
        let state: String = sqlx::query_scalar(
            "SELECT state FROM workflow_run WHERE tenant_id=$1 AND region=$2 AND run_id=$3",
        )
        .bind(tenant)
        .bind(region)
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, "running", "neighbour scope was not observed");
    }
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renewal_failure_joins_body_before_run_once_returns_and_refuses_commit() {
    let _guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("renewal_join").await;
    let mut worker = worker_with_ttl(&pool, "acme", "no-osl", 7, "worker-fenced", 3);
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel();
    let finish_rx = Arc::new(std::sync::Mutex::new(finish_rx));
    let body_exited = Arc::new(AtomicUsize::new(0));
    let wait_for_finish = Arc::clone(&finish_rx);
    let exited = Arc::clone(&body_exited);
    worker
        .register_definition("wf.slow", 1, "slow-v1", move |_input, _ctx| {
            started_tx.send(()).expect("announce body start");
            wait_for_finish
                .lock()
                .expect("finish receiver lock")
                .recv()
                .expect("test releases body");
            exited.store(1, Ordering::SeqCst);
            Ok(Vec::new())
        })
        .unwrap();
    seed_run(&pool, "acme", "no-osl", "R-slow", "wf.slow", 0, 7).await;
    let worker = Arc::new(worker);
    let task = {
        let worker = Arc::clone(&worker);
        tokio::spawn(async move { worker.run_once(1_752_796_800, "2026-07-18T00:00:00Z").await })
    };
    tokio::task::spawn_blocking(move || {
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("body started")
    })
    .await
    .unwrap();

    // Invalidate the exact owner+epoch while the synchronous body is still blocked. The next
    // heartbeat observes LeaseLost, but run_once must continue joining the body rather than detach.
    sqlx::query(
        "UPDATE workflow_run SET lease_epoch = lease_epoch + 1 \
         WHERE tenant_id='acme' AND region='no-osl' AND run_id='R-slow'",
    )
    .execute(&pool)
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1_300)).await;
    assert!(
        !task.is_finished(),
        "renewal failure did not detach the live body"
    );
    assert_eq!(body_exited.load(Ordering::SeqCst), 0);

    finish_tx.send(()).unwrap();
    let result = task.await.unwrap();
    assert!(result.is_err(), "lost lease refuses the stale drive commit");
    assert_eq!(
        body_exited.load(Ordering::SeqCst),
        1,
        "run_once returned only after the synchronous body exited"
    );
    let row = sqlx::query(
        "SELECT state, cursor, last_drive_id FROM workflow_run \
         WHERE tenant_id='acme' AND region='no-osl' AND run_id='R-slow'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("state"), "running");
    assert_eq!(row.get::<i64, _>("cursor"), 0);
    assert!(row.get::<Option<String>, _>("last_drive_id").is_none());
    cleanup(bare, pool, schema).await;
}
