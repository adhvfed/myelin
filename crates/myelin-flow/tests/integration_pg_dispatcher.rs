#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::{Actor, IdMinter, MonotonicMinter, OutboxStore};
use myelin_flow::{
    boot_flow, job_idem_token, migrations::migrations, ActivityError, DurableExecutor, JobKind,
    JobOutcome, JobRunner, JobSpec, PgClaimedDriveInput, PgFlowDriveStore, PgFlowWorker,
    PgInputResolveError, PgRunOnceOutcome, PgWorkerError, PgWorkerScope, RetryPolicy, SignalSpec,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_refs::ArtifactRef;
use myelin_storage::{provider::foundation_migrations, HotTables, PgMigrator, PgOutboxBacking};
use myelin_substrate::Config;
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Default)]
struct RecordingJobRunner {
    calls: AtomicUsize,
}

impl JobRunner for RecordingJobRunner {
    fn dispatch(&self, _spec: &JobSpec) -> Result<(), ActivityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

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
async fn immutable_input_resolution_retries_without_commit_and_permanent_refusal_fail_stops() {
    let _guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("input_resolution").await;
    let attempts = Arc::new(AtomicUsize::new(0));
    let body_calls = Arc::new(AtomicUsize::new(0));
    let mut flow = worker(&pool, "acme", "no-osl", 5, "worker-resolver");
    let resolver_attempts = Arc::clone(&attempts);
    let successful_body_calls = Arc::clone(&body_calls);
    flow.register_definition_with_input_resolver(
        "wf.resolved",
        1,
        "resolved-v1",
        move |input: PgClaimedDriveInput| {
            let resolver_attempts = Arc::clone(&resolver_attempts);
            async move {
                assert_eq!(input.tenant, TenantId("acme".into()));
                assert_eq!(input.region, Region("no-osl".into()));
                assert_eq!(input.run_id, "R-resolved");
                if resolver_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(PgInputResolveError::Retry(
                        "manifest store unavailable".into(),
                    ))
                } else {
                    Ok(b"pinned-manifest".to_vec())
                }
            }
        },
        move |input, ctx| {
            assert_eq!(input.material, b"pinned-manifest");
            successful_body_calls.fetch_add(1, Ordering::SeqCst);
            ctx.now();
            Ok(Vec::new())
        },
    )
    .unwrap();

    let refused_body_calls = Arc::clone(&body_calls);
    flow.register_definition_with_input_resolver(
        "wf.refused",
        1,
        "refused-v1",
        |_input: PgClaimedDriveInput| async {
            Err(PgInputResolveError::Permanent(
                "manifest digest mismatch".into(),
            ))
        },
        move |_input, _ctx| {
            refused_body_calls.fetch_add(100, Ordering::SeqCst);
            Ok(Vec::new())
        },
    )
    .unwrap();

    seed_run(&pool, "acme", "no-osl", "R-resolved", "wf.resolved", 0, 5).await;
    assert!(matches!(
        flow.run_once(1_752_796_800, "2026-07-18T00:00:00Z")
            .await,
        Err(PgWorkerError::InputUnavailable(ref detail))
            if detail == "manifest store unavailable"
    ));
    let retry_state: (String, i64, Option<String>, i64, i64) = sqlx::query_as(
        "SELECT state, cursor, lease_owner, \
           (SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='no-osl' \
             AND run_id='R-resolved'), \
           (SELECT count(*) FROM outbox) \
         FROM workflow_run WHERE tenant_id='acme' AND region='no-osl' AND run_id='R-resolved'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(retry_state, ("running".into(), 0, None, 0, 0));
    assert_eq!(body_calls.load(Ordering::SeqCst), 0);

    assert!(matches!(
        flow.run_once(1_752_796_801, "2026-07-18T00:00:01Z")
            .await
            .unwrap(),
        PgRunOnceOutcome::Driven { .. }
    ));
    let completed: (String, i64, i64) = sqlx::query_as(
        "SELECT state, cursor, \
           (SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='no-osl' \
             AND run_id='R-resolved') \
         FROM workflow_run WHERE tenant_id='acme' AND region='no-osl' AND run_id='R-resolved'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completed, ("completed".into(), 1, 1));
    assert_eq!(body_calls.load(Ordering::SeqCst), 1);

    seed_run(&pool, "acme", "no-osl", "R-refused", "wf.refused", 0, 5).await;
    assert!(matches!(
        flow.run_once(1_752_796_802, "2026-07-18T00:00:02Z")
            .await
            .unwrap(),
        PgRunOnceOutcome::Driven { .. }
    ));
    let refused: (String, i64, Option<String>, i64) = sqlx::query_as(
        "SELECT state, cursor, lease_owner, \
           (SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='no-osl' \
             AND run_id='R-refused') \
         FROM workflow_run WHERE tenant_id='acme' AND region='no-osl' AND run_id='R-refused'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(refused, ("nondeterministic".into(), 0, None, 0));
    assert_eq!(
        body_calls.load(Ordering::SeqCst),
        1,
        "refused body never ran"
    );

    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_drive_never_hides_that_its_lease_could_not_be_released() {
    let _guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("release_failure").await;
    let mut flow = worker(&pool, "acme", "no-osl", 5, "worker-release-failure");
    let fault_pool = pool.clone();
    flow.register_definition_with_input_resolver(
        "wf.release-failure",
        1,
        "release-failure-v1",
        move |_input: PgClaimedDriveInput| {
            let fault_pool = fault_pool.clone();
            async move {
                let removed = sqlx::query(
                    "DELETE FROM workflow_run \
                     WHERE tenant_id = 'acme' AND region = 'no-osl' \
                       AND run_id = 'R-release-failure'",
                )
                .execute(&fault_pool)
                .await
                .expect("remove the isolated claimed run after the drive was claimed")
                .rows_affected();
                assert_eq!(removed, 1, "the fault must remove exactly the claimed run");
                Err(PgInputResolveError::Retry(
                    "immutable input is temporarily unavailable".into(),
                ))
            }
        },
        |_input, _ctx| panic!("a retryable input failure must never run the workflow body"),
    )
    .unwrap();
    seed_run(
        &pool,
        "acme",
        "no-osl",
        "R-release-failure",
        "wf.release-failure",
        0,
        5,
    )
    .await;

    let error = flow
        .run_once(1_752_796_800, "2026-07-18T00:00:00Z")
        .await
        .unwrap_err();

    let PgWorkerError::LeaseReleaseFailed { drive, release } = error else {
        panic!("the worker must preserve both failures, got {error:?}");
    };
    assert!(
        matches!(*drive, PgWorkerError::InputUnavailable(ref detail)
            if detail == "immutable input is temporarily unavailable"),
        "the original drive failure must remain inspectable, got {drive:?}"
    );
    assert_eq!(release, myelin_flow::DriveStoreError::LeaseLost);

    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ready_batches_fire_every_due_deadline_amid_ordinary_work() {
    let _guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("timer_fairness").await;
    let mut flow = worker(&pool, "acme", "no-osl", 6, "worker-fair");
    flow.register_definition("wf.deadline", 1, "deadline-v1", |_input, ctx| {
        match ctx
            .wait_for_signal("finish", Some(10))
            .map_err(|error| format!("{error:?}"))?
        {
            myelin_flow::WaitOutcome::Parked | myelin_flow::WaitOutcome::TimedOut => Ok(Vec::new()),
            myelin_flow::WaitOutcome::Signalled { payload, .. } => Ok(payload),
        }
    })
    .unwrap();
    flow.register_definition("wf.ordinary", 1, "ordinary-v1", |_input, ctx| {
        ctx.now();
        Ok(Vec::new())
    })
    .unwrap();

    let base = chrono::Utc::now().timestamp();
    for ordinal in 0..5 {
        seed_run(
            &pool,
            "acme",
            "no-osl",
            &format!("R-deadline-{ordinal}"),
            "wf.deadline",
            0,
            6,
        )
        .await;
    }
    let armed = flow
        .run_until_idle(5, base, "2026-07-18T00:00:00Z")
        .await
        .unwrap();
    assert_eq!((armed.driven, armed.timers_fired), (5, 0));

    for ordinal in 0..5 {
        seed_run(
            &pool,
            "acme",
            "no-osl",
            &format!("R-ordinary-{ordinal}"),
            "wf.ordinary",
            0,
            6,
        )
        .await;
    }
    let settled = flow
        .run_until_idle(10, base + 10, "2026-07-18T00:00:10Z")
        .await
        .unwrap();
    assert_eq!(
        (settled.timers_fired, settled.driven),
        (5, 10),
        "five deadlines wake and settle even though five ordinary runs are also runnable"
    );

    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_run
         WHERE tenant_id='acme' AND region='no-osl'
           AND state IN ('running', 'waiting')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 0, "the fair batch leaves no hidden ready work");
    let fired: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_timer
         WHERE tenant_id='acme' AND region='no-osl' AND fired",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(fired, 5, "every user deadline has one durable firing");

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_frontier_arms_both_deadlines_and_fired_earliest_replays_without_reopening() {
    let _guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("job_frontier").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 9, "worker-frontier");
    let body_runner = Arc::clone(&runner);
    flow.register_definition("wf.frontier", 1, "frontier-v1", move |_input, ctx| {
        let earlier = ctx
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/earlier"),
                body_runner.as_ref(),
                Some(10),
            )
            .map_err(|error| format!("{error:?}"))?;
        let later = ctx
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/later"),
                body_runner.as_ref(),
                Some(120),
            )
            .map_err(|error| format!("{error:?}"))?;

        match ctx
            .join_dispatched_job(&earlier)
            .map_err(|error| format!("{error:?}"))?
        {
            JobOutcome::Parked => return Ok(Vec::new()),
            JobOutcome::TimedOut => {}
            JobOutcome::Completed { .. } => return Err("earlier job unexpectedly completed".into()),
        }
        match ctx
            .join_dispatched_job(&later)
            .map_err(|error| format!("{error:?}"))?
        {
            JobOutcome::Completed { result, .. } => Ok(result),
            JobOutcome::Parked => Ok(Vec::new()),
            JobOutcome::TimedOut => Err("later job unexpectedly timed out".into()),
        }
    })
    .unwrap();
    seed_run(&pool, "acme", "no-osl", "R-frontier", "wf.frontier", 0, 9).await;
    let base = chrono::Utc::now().timestamp();
    assert!(matches!(
        flow.run_once(base, "2026-07-18T00:00:00Z").await.unwrap(),
        PgRunOnceOutcome::Driven { .. }
    ));
    let timer_rows = sqlx::query(
        "SELECT timer_id, command_id, EXTRACT(EPOCH FROM fire_at)::bigint AS fire_at, fired \
         FROM wf_timer WHERE tenant_id='acme' AND region='no-osl' AND run_id='R-frontier' \
         ORDER BY fire_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        timer_rows.len(),
        2,
        "both sibling SLAs arm before the first join"
    );
    assert_eq!(timer_rows[0].get::<i64, _>("fire_at"), base + 10);
    assert_eq!(timer_rows[1].get::<i64, _>("fire_at"), base + 120);

    let later_token = job_idem_token("R-frontier", "wf.frontier:1");
    assert_eq!(
        tokio::task::block_in_place(|| flow.executor().signal(SignalSpec {
            run: myelin_flow::RunId("R-frontier".into()),
            signal_name: "job.done".into(),
            idem_key: later_token.clone(),
            payload: vec![ArtifactRef("myelin://acme/ci/artifact/later".into())],
            payload_key_ref: None,
        }))
        .unwrap(),
        myelin_flow::SignalOutcome::Buffered
    );
    flow.run_once(base + 1, "2026-07-18T00:00:01Z")
        .await
        .unwrap();
    let state: String = sqlx::query_scalar(
        "SELECT state FROM workflow_run WHERE tenant_id='acme' AND region='no-osl' \
         AND run_id='R-frontier'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "waiting");

    let drive_store = PgFlowDriveStore::new(
        pool.clone(),
        TenantId("acme".into()),
        Region("no-osl".into()),
    );
    let fired = drive_store
        .fire_due_timer(9, base + 10)
        .await
        .unwrap()
        .expect("earliest sibling timer is due");
    assert_eq!(fired.command_id, "wf.frontier:0/job-timeout");
    assert!(
        drive_store
            .fire_due_timer(9, base + 10)
            .await
            .unwrap()
            .is_none(),
        "the same deadline fires effectively once"
    );

    flow.run_once(base + 11, "2026-07-18T00:00:11Z")
        .await
        .unwrap();
    let final_state: String = sqlx::query_scalar(
        "SELECT state FROM workflow_run WHERE tenant_id='acme' AND region='no-osl' \
         AND run_id='R-frontier'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(final_state, "completed");
    let fired_flags: Vec<bool> = sqlx::query_scalar(
        "SELECT fired FROM wf_timer WHERE tenant_id='acme' AND region='no-osl' \
         AND run_id='R-frontier' ORDER BY fire_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        fired_flags,
        vec![true, true],
        "replayed dispatch did not reopen A; successful B disarmed its SLA"
    );
    let fire_history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='no-osl' \
         AND run_id='R-frontier' AND kind='timer_fired' \
         AND command_id='wf.frontier:0/job-timeout/fired'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(fire_history, 1, "one durable wake/history row");
    let later_consumed: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_seq FROM wf_signal WHERE tenant_id='acme' AND region='no-osl' \
         AND run_id='R-frontier' AND signal_name='job.done' AND idem_key=$1",
    )
    .bind(&later_token)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(later_consumed.is_some());
    let bound_receipt: serde_json::Value = sqlx::query_scalar(
        "SELECT result FROM wf_history WHERE tenant_id='acme' AND region='no-osl' \
         AND run_id='R-frontier' AND command_id='wf.frontier:3' AND kind='signal_received'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        bound_receipt,
        serde_json::json!([
            format!("myelin://flow/signal-idem/{later_token}"),
            "myelin://flow/signal-name/job.done",
            "myelin://acme/ci/artifact/later"
        ]),
        "the durable exact receipt binds signal name, idem key, and payload"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "replay never redispatched"
    );
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_receipt_time_accepts_equality_and_late_result_never_satisfies_timed_out_join() {
    let _guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("receipt_race").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 10, "worker-receipt");
    let body_runner = Arc::clone(&runner);
    flow.register_definition("wf.receipt", 1, "receipt-v1", move |_input, ctx| {
        let job = ctx
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/receipt"),
                body_runner.as_ref(),
                Some(10),
            )
            .map_err(|error| format!("{error:?}"))?;
        match ctx
            .join_dispatched_job(&job)
            .map_err(|error| format!("{error:?}"))?
        {
            JobOutcome::Completed { result, .. } => Ok(result),
            JobOutcome::Parked => Ok(Vec::new()),
            JobOutcome::TimedOut => match ctx
                .wait_for_signal("finish", None)
                .map_err(|error| format!("{error:?}"))?
            {
                myelin_flow::WaitOutcome::Signalled { payload, .. } => Ok(payload),
                myelin_flow::WaitOutcome::Parked => Ok(Vec::new()),
                myelin_flow::WaitOutcome::TimedOut => unreachable!("finish is unbounded"),
            },
        }
    })
    .unwrap();
    let base = chrono::Utc::now().timestamp();

    seed_run(&pool, "acme", "no-osl", "R-equal", "wf.receipt", 0, 10).await;
    flow.run_once(base, "2026-07-18T00:00:00Z").await.unwrap();
    let equal_token = job_idem_token("R-equal", "wf.receipt:0");
    sqlx::query(
        "INSERT INTO wf_signal \
           (tenant_id,region,run_id,signal_name,idem_key,payload,received_at) \
         VALUES ('acme','no-osl','R-equal','job.done',$1,$2,to_timestamp($3))",
    )
    .bind(&equal_token)
    .bind(serde_json::json!(["myelin://acme/ci/artifact/equal"]))
    .bind((base + 10) as f64)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_run SET state='running' \
         WHERE tenant_id='acme' AND region='no-osl' AND run_id='R-equal'",
    )
    .execute(&pool)
    .await
    .unwrap();
    flow.run_once(base + 20, "2026-07-18T00:00:20Z")
        .await
        .unwrap();
    let equal: (String, Option<i64>) = sqlx::query_as(
        "SELECT run.state, signal.consumed_seq FROM workflow_run AS run \
         JOIN wf_signal AS signal ON signal.tenant_id=run.tenant_id \
           AND signal.region=run.region AND signal.run_id=run.run_id \
         WHERE run.tenant_id='acme' AND run.region='no-osl' AND run.run_id='R-equal'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(equal.0, "completed");
    assert!(
        equal.1.is_some(),
        "receipt exactly at the deadline is accepted"
    );

    seed_run(&pool, "acme", "no-osl", "R-late", "wf.receipt", 0, 10).await;
    flow.run_once(base, "2026-07-18T00:00:00Z").await.unwrap();
    let late_token = job_idem_token("R-late", "wf.receipt:0");
    sqlx::query(
        "INSERT INTO wf_signal \
           (tenant_id,region,run_id,signal_name,idem_key,payload,received_at) \
         VALUES ('acme','no-osl','R-late','job.done',$1,$2,to_timestamp($3))",
    )
    .bind(&late_token)
    .bind(serde_json::json!(["myelin://acme/ci/artifact/late"]))
    .bind((base + 10) as f64 + 0.001)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow_run SET state='running' \
         WHERE tenant_id='acme' AND region='no-osl' AND run_id='R-late'",
    )
    .execute(&pool)
    .await
    .unwrap();
    flow.run_once(base + 20, "2026-07-18T00:00:20Z")
        .await
        .unwrap();
    let late_after_timeout: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_seq FROM wf_signal WHERE tenant_id='acme' AND region='no-osl' \
         AND run_id='R-late' AND signal_name='job.done' AND idem_key=$1",
    )
    .bind(&late_token)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        late_after_timeout, None,
        "late completion loses and remains unconsumed"
    );
    let waiting: String = sqlx::query_scalar(
        "SELECT state FROM workflow_run WHERE tenant_id='acme' AND region='no-osl' \
         AND run_id='R-late'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        waiting, "waiting",
        "timeout receipt committed before the next wait"
    );

    tokio::task::block_in_place(|| {
        flow.executor().signal(SignalSpec {
            run: myelin_flow::RunId("R-late".into()),
            signal_name: "finish".into(),
            idem_key: "finish-1".into(),
            payload: Vec::new(),
            payload_key_ref: None,
        })
    })
    .unwrap();
    flow.run_once(base + 21, "2026-07-18T00:00:21Z")
        .await
        .unwrap();
    let late_final: (String, Option<i64>) = sqlx::query_as(
        "SELECT run.state, signal.consumed_seq FROM workflow_run AS run \
         JOIN wf_signal AS signal ON signal.tenant_id=run.tenant_id \
           AND signal.region=run.region AND signal.run_id=run.run_id \
           AND signal.signal_name='job.done' \
         WHERE run.tenant_id='acme' AND run.region='no-osl' AND run.run_id='R-late'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(late_final.0, "completed");
    assert_eq!(
        late_final.1, None,
        "replay short-circuits to the journaled timeout and never consumes the late row"
    );
    assert_eq!(
        runner.calls.load(Ordering::SeqCst),
        2,
        "one dispatch per run"
    );
    cleanup(bare, pool, schema).await;
}
