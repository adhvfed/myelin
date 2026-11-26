#![cfg(feature = "integration")]

//! Idle workflow partitions sleep until a commit, a deadline or a reconnect wakes them, and
//! re-check on a safety interval for work no wake announces. Every test except the
//! safety-interval one gives the loop an idle bound of an hour, so work is only driven in time
//! if the wake path under test works.

use myelin_config::MyelinConfig;
use myelin_events::{Actor, IdMinter, MonotonicMinter};
use myelin_flow::{
    migrations::migrations, DurableExecutor, IdlePacing, PgFlowDriveStore, PgFlowWorker,
    PgWorkerScope, RetryPolicy, SignalSpec, WaitOutcome,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::work_wake::{WorkWake, WorkWakeHub, WORK_WAKE_CHANNEL};
use myelin_storage::{provider::foundation_migrations, HotTables, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::{PgConnectOptions, PgListener};
use sqlx::{Executor, PgPool};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

static SCHEMA_SEQ: AtomicU64 = AtomicU64::new(0);
static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const TENANT: &str = "acme";
const REGION: &str = "no-osl";
const PARTITION: i16 = 5;
const NEVER: Duration = Duration::from_secs(3600);
const PACING: IdlePacing = IdlePacing {
    busy: Duration::from_millis(250),
    max_idle: NEVER,
};

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| MyelinConfig::dev().database_url)
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

struct Stack {
    bare: PgPool,
    pool: PgPool,
    schema: String,
    shutdown: tokio::sync::watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Stack {
    async fn new(label: &str) -> Self {
        let bare = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url())
            .await
            .expect("connect to live PostgreSQL");
        let n = SCHEMA_SEQ.fetch_add(1, Ordering::Relaxed);
        let schema = format!("flow_wake_{label}_{}_{}", std::process::id(), n);
        bare.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
            .await
            .expect("drop stale schema");
        bare.execute(format!("CREATE SCHEMA {schema}").as_str())
            .await
            .expect("create schema");
        let pinned = schema.clone();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
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
        let (shutdown, _) = tokio::sync::watch::channel(false);
        Self {
            bare,
            pool,
            schema,
            shutdown,
            tasks: Vec::new(),
        }
    }

    fn worker(&self, name: &str, lease_ttl_secs: i64) -> PgFlowWorker {
        let actor = Actor(Principal::new(
            TenantId(TENANT.into()),
            Region(REGION.into()),
            PrincipalId("svc:flow-wake-test".into()),
            PrincipalKind::Service,
            DataRole::Processor,
            PrincipalStatus::Active,
        ));
        let scope = PgWorkerScope::new(
            TenantId(TENANT.into()),
            Region(REGION.into()),
            PARTITION,
            name,
            lease_ttl_secs,
            actor,
            1,
        )
        .expect("valid exact worker scope");
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        PgFlowWorker::new(
            self.pool.clone(),
            tokio::runtime::Handle::current(),
            minter,
            scope,
        )
    }

    /// The listener's own connection, named after the schema so a test can find it.
    fn listen(&self) -> PgConnectOptions {
        (*self.pool.connect_options())
            .clone()
            .application_name(&self.listener_name())
    }

    fn listener_name(&self) -> String {
        format!("{}-wake", self.schema)
    }

    /// Runs one process: a listener and the given workers, each on its own partition wake.
    fn run(&mut self, workers: Vec<Arc<PgFlowWorker>>) {
        self.run_with(workers, PACING);
    }

    fn run_with(&mut self, workers: Vec<Arc<PgFlowWorker>>, pacing: IdlePacing) {
        let mut hub = WorkWakeHub::new(REGION);
        for worker in workers {
            let wake = hub.flow_partition(TENANT, PARTITION);
            let shutdown = self.shutdown.subscribe();
            self.tasks.push(tokio::spawn(async move {
                worker
                    .run_until_shutdown(shutdown, &wake, pacing, 32)
                    .await
                    .expect("partition loop runs until shutdown");
            }));
        }
        self.spawn_hub(hub, self.listen(), NEVER);
    }

    fn spawn_hub(&mut self, hub: WorkWakeHub, listen: PgConnectOptions, degraded: Duration) {
        let pool = self.pool.clone();
        let shutdown = self.shutdown.subscribe();
        self.tasks.push(tokio::spawn(async move {
            hub.run(pool, listen, shutdown, degraded)
                .await
                .expect("wake listener runs until shutdown");
        }));
    }

    async fn terminate_listener(&self) {
        let terminated: Vec<bool> = sqlx::query_scalar(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = $1",
        )
        .bind(self.listener_name())
        .fetch_all(&self.bare)
        .await
        .expect("terminate the listener connection");
        assert_eq!(terminated, vec![true], "exactly one listener was connected");
    }

    async fn insert_run(&self, run_id: &str, wf_type: &str) {
        sqlx::query(
            "INSERT INTO workflow_run \
               (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, budget, \
                correlation_id, causation_id, caused_by, depth, partition, idem_key) \
             VALUES ($1,$2,$3,$4,1,'[]'::jsonb,'running',0,NULL,$3,NULL,NULL,0,$5,$3)",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(run_id)
        .bind(wf_type)
        .bind(PARTITION)
        .execute(&self.pool)
        .await
        .expect("insert run");
    }

    async fn state(&self, run_id: &str) -> String {
        sqlx::query_scalar("SELECT state FROM workflow_run WHERE tenant_id=$1 AND run_id=$2")
            .bind(TENANT)
            .bind(run_id)
            .fetch_one(&self.pool)
            .await
            .expect("read run state")
    }

    async fn wait_for_state(&self, run_id: &str, state: &str, within: Duration) -> Duration {
        let started = Instant::now();
        loop {
            if self.state(run_id).await == state {
                return started.elapsed();
            }
            assert!(
                started.elapsed() < within,
                "run {run_id} did not reach `{state}` within {within:?}; it is `{}`",
                self.state(run_id).await
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn stop(self) {
        let _ = self.shutdown.send(true);
        for task in self.tasks {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("loops stop on shutdown")
                .expect("loop task did not panic");
        }
        self.pool.close().await;
        self.bare
            .execute(format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema).as_str())
            .await
            .expect("drop schema");
    }
}

fn completes(worker: &mut PgFlowWorker, wf_type: &str) {
    worker
        .register_definition(wf_type, 1, &format!("{wf_type}-v1"), |_input, ctx| {
            ctx.now();
            Ok(Vec::new())
        })
        .expect("register body");
}

/// Let the loop finish its startup batch and settle into its idle wait.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_partition_drives_a_new_run_when_it_commits() {
    let _guard = TEST_LOCK.lock().await;
    let mut stack = Stack::new("start").await;
    let mut worker = stack.worker("wake-start", 30);
    completes(&mut worker, "wf.start");
    stack.run(vec![Arc::new(worker)]);
    settle().await;

    stack.insert_run("R-start", "wf.start").await;
    let took = stack
        .wait_for_state("R-start", "completed", Duration::from_secs(3))
        .await;
    assert!(
        took < Duration::from_secs(1),
        "driven {took:?} after commit"
    );
    stack.stop().await;
}

/// End to end through the executor. Delivery also moves the run from `waiting` to `running`,
/// which the run trigger announces; the signal trigger itself is covered by
/// `inserted_signals_and_timers_announce_their_partition`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_signal_resumes_a_parked_run_without_a_poll() {
    let _guard = TEST_LOCK.lock().await;
    let mut stack = Stack::new("signal").await;
    let mut worker = stack.worker("wake-signal", 30);
    worker
        .register_definition("wf.signal", 1, "signal-v1", |_input, ctx| {
            match ctx
                .wait_for_signal("finish", None)
                .map_err(|error| format!("{error:?}"))?
            {
                WaitOutcome::Signalled { payload, .. } => Ok(payload),
                WaitOutcome::Parked | WaitOutcome::TimedOut => Ok(Vec::new()),
            }
        })
        .unwrap();
    let worker = Arc::new(worker);
    stack.run(vec![worker.clone()]);
    settle().await;

    stack.insert_run("R-signal", "wf.signal").await;
    stack
        .wait_for_state("R-signal", "waiting", Duration::from_secs(3))
        .await;
    settle().await;
    tokio::task::block_in_place(|| {
        worker.executor().signal(SignalSpec {
            run: myelin_flow::RunId("R-signal".into()),
            signal_name: "finish".into(),
            idem_key: "finish-1".into(),
            payload: Vec::new(),
            payload_key_ref: None,
        })
    })
    .expect("deliver the signal");
    let took = stack
        .wait_for_state("R-signal", "completed", Duration::from_secs(3))
        .await;
    assert!(
        took < Duration::from_secs(1),
        "resumed {took:?} after signal"
    );
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_due_timer_wakes_its_partition_at_its_deadline_with_no_write() {
    let _guard = TEST_LOCK.lock().await;
    let mut stack = Stack::new("timer").await;
    let mut worker = stack.worker("wake-timer", 30);
    worker
        .register_definition("wf.timer", 1, "timer-v1", |_input, ctx| {
            match ctx
                .wait_for_signal("never", Some(2))
                .map_err(|error| format!("{error:?}"))?
            {
                WaitOutcome::TimedOut | WaitOutcome::Signalled { .. } => Ok(Vec::new()),
                WaitOutcome::Parked => Ok(Vec::new()),
            }
        })
        .unwrap();
    stack.run(vec![Arc::new(worker)]);
    settle().await;

    stack.insert_run("R-timer", "wf.timer").await;
    stack
        .wait_for_state("R-timer", "waiting", Duration::from_secs(3))
        .await;
    let took = stack
        .wait_for_state("R-timer", "completed", Duration::from_secs(6))
        .await;
    assert!(
        took <= Duration::from_millis(4_500),
        "the two-second timer fired after {took:?}"
    );
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_foreign_lease_is_reclaimed_at_its_deadline() {
    let _guard = TEST_LOCK.lock().await;
    let mut stack = Stack::new("lease").await;
    let mut worker = stack.worker("wake-lease", 30);
    completes(&mut worker, "wf.lease");
    stack.run(vec![Arc::new(worker)]);
    settle().await;

    // A crashed process claimed the run: its lease runs out in two seconds and nothing will
    // ever write to the run again.
    sqlx::query(
        "INSERT INTO workflow_run \
           (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, budget, \
            correlation_id, causation_id, caused_by, depth, partition, idem_key, \
            lease_owner, lease_expires, lease_epoch) \
         VALUES ($1,$2,'R-lease','wf.lease',1,'[]'::jsonb,'running',0,NULL,'R-lease',NULL, \
                 NULL,0,$3,'R-lease','crashed-worker', \
                 clock_timestamp() + INTERVAL '2 seconds', 1)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(PARTITION)
    .execute(&stack.pool)
    .await
    .expect("insert a run held by a dead lease");
    let took = stack
        .wait_for_state("R-lease", "completed", Duration::from_secs(6))
        .await;
    assert!(
        (Duration::from_millis(1_500)..=Duration::from_millis(3_500)).contains(&took),
        "the dead lease was reclaimed after {took:?}, not at its two-second expiry"
    );
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_listener_resubscribes_and_misses_nothing() {
    let _guard = TEST_LOCK.lock().await;
    let mut stack = Stack::new("reconnect").await;
    let mut worker = stack.worker("wake-reconnect", 30);
    completes(&mut worker, "wf.reconnect");
    stack.run(vec![Arc::new(worker)]);
    settle().await;

    stack.terminate_listener().await;

    stack.insert_run("R-reconnect", "wf.reconnect").await;
    let took = stack
        .wait_for_state("R-reconnect", "completed", Duration::from_secs(5))
        .await;
    assert!(
        took < Duration::from_secs(3),
        "driven {took:?} after a reconnect"
    );
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_woken_processes_drive_every_run_exactly_once() {
    let _guard = TEST_LOCK.lock().await;
    let mut stack = Stack::new("instances").await;
    let executions = Arc::new(AtomicUsize::new(0));
    let mut processes = Vec::new();
    for name in ["instance-a", "instance-b"] {
        let mut worker = stack.worker(name, 30);
        let executions = executions.clone();
        worker
            .register_definition("wf.once", 1, "once-v1", move |_input, ctx| {
                ctx.activity(RetryPolicy { max_attempts: 1 }, |_, _| {
                    executions.fetch_add(1, Ordering::SeqCst);
                    Ok(Vec::new())
                })
                .map_err(|error| format!("{error:?}"))?;
                Ok(Vec::new())
            })
            .unwrap();
        processes.push(Arc::new(worker));
    }
    for worker in processes {
        stack.run(vec![worker]);
    }
    settle().await;

    for ordinal in 0..20 {
        stack
            .insert_run(&format!("R-once-{ordinal}"), "wf.once")
            .await;
    }
    for ordinal in 0..20 {
        stack
            .wait_for_state(
                &format!("R-once-{ordinal}"),
                "completed",
                Duration::from_secs(5),
            )
            .await;
    }
    assert_eq!(
        executions.load(Ordering::SeqCst),
        20,
        "both processes were woken for every run; each run's activity ran once"
    );
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claims_and_renewals_are_silent_and_releases_notify() {
    let _guard = TEST_LOCK.lock().await;
    let stack = Stack::new("silence").await;
    let mut listener = PgListener::connect_with(&stack.pool).await.unwrap();
    listener.listen(WORK_WAKE_CHANNEL).await.unwrap();
    let expected = serde_json::json!([stack.schema, "flow", REGION, TENANT, PARTITION]);

    stack.insert_run("R-silence", "wf.silence").await;
    assert_eq!(next_wake(&mut listener).await, Some(expected.clone()));

    let store = PgFlowDriveStore::new(
        stack.pool.clone(),
        TenantId(TENANT.into()),
        Region(REGION.into()),
    );
    let lease = store
        .claim_runnable_definition(PARTITION, "wf.silence", 1, "silent-worker", 30)
        .await
        .unwrap()
        .expect("the run is claimable");
    store.renew_lease(&lease, 30).await.unwrap();
    assert_eq!(
        next_wake(&mut listener).await,
        None,
        "a claim and a renewal create no work for anyone"
    );

    store.release_lease(&lease).await.unwrap();
    assert_eq!(
        next_wake(&mut listener).await,
        Some(expected),
        "a released lease makes the run claimable again"
    );
    drop(listener);
    stack.stop().await;
}

async fn next_wake(listener: &mut PgListener) -> Option<serde_json::Value> {
    let notification = tokio::time::timeout(Duration::from_millis(500), listener.recv())
        .await
        .ok()?
        .expect("listener stays connected");
    Some(serde_json::from_str(notification.payload()).expect("wake payload is JSON"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inserted_signals_and_timers_announce_their_partition() {
    let _guard = TEST_LOCK.lock().await;
    let stack = Stack::new("inserts").await;
    stack.insert_run("R-inserts", "wf.inserts").await;
    let mut listener = PgListener::connect_with(&stack.pool).await.unwrap();
    listener.listen(WORK_WAKE_CHANNEL).await.unwrap();
    let partition_wake = serde_json::json!([stack.schema, "flow", REGION, TENANT, PARTITION]);

    sqlx::query(
        "INSERT INTO wf_signal (tenant_id, region, run_id, signal_name, idem_key, payload) \
         VALUES ($1, $2, 'R-inserts', 'finish', 'finish-1', '[]'::jsonb)",
    )
    .bind(TENANT)
    .bind(REGION)
    .execute(&stack.pool)
    .await
    .unwrap();
    assert_eq!(
        next_wake(&mut listener).await,
        Some(partition_wake.clone()),
        "a signal names its run's partition"
    );

    sqlx::query(
        "INSERT INTO wf_signal (tenant_id, region, run_id, signal_name, idem_key, payload) \
         VALUES ($1, $2, 'R-not-visible', 'finish', 'finish-1', '[]'::jsonb)",
    )
    .bind(TENANT)
    .bind(REGION)
    .execute(&stack.pool)
    .await
    .unwrap();
    assert_eq!(
        next_wake(&mut listener).await,
        Some(serde_json::json!([
            stack.schema,
            "flow",
            REGION,
            TENANT,
            null
        ])),
        "a signal whose run is not visible wakes every partition of the tenant"
    );

    sqlx::query(
        "INSERT INTO wf_timer \
           (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, partition) \
         VALUES ($1, $2, 'T-inserts', 'R-inserts', 'wf.inserts:0', \
                 clock_timestamp() + INTERVAL '1 hour', 0, $3)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(PARTITION)
    .execute(&stack.pool)
    .await
    .unwrap();
    assert_eq!(
        next_wake(&mut listener).await,
        Some(partition_wake),
        "an armed timer wakes its partition to read the new deadline"
    );
    drop(listener);
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resubscribing_wakes_every_loop_without_any_write() {
    let _guard = TEST_LOCK.lock().await;
    let mut stack = Stack::new("resubscribe").await;
    let mut hub = WorkWakeHub::new(REGION);
    let wake = hub.flow_partition(TENANT, PARTITION);
    stack.spawn_hub(hub, stack.listen(), NEVER);
    assert!(
        woken(&wake, Duration::from_secs(2)).await,
        "subscribing wakes once"
    );
    settle().await;
    assert!(
        !woken(&wake, Duration::from_millis(200)).await,
        "then stays quiet"
    );

    stack.terminate_listener().await;
    assert!(
        woken(&wake, Duration::from_secs(3)).await,
        "losing the subscription wakes every loop, with nothing written"
    );
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_listener_falls_back_to_polling() {
    let _guard = TEST_LOCK.lock().await;
    let mut stack = Stack::new("degraded").await;
    let mut hub = WorkWakeHub::new(REGION);
    let wake = hub.flow_partition(TENANT, PARTITION);
    let unreachable = stack.listen().host("127.0.0.1").port(1);
    stack.spawn_hub(hub, unreachable, Duration::from_millis(100));
    let started = Instant::now();
    for _ in 0..5 {
        assert!(
            woken(&wake, Duration::from_secs(2)).await,
            "a loop is woken on the degraded interval"
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "five degraded wakes took {:?}",
        started.elapsed()
    );
    stack.stop().await;
}

/// A peer claims a run silently (claims never notify) after this loop read its deadlines, then
/// dies. No write or deadline will ever wake this loop; only the safety interval finds the
/// run once the dead lease expires.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_claim_by_a_dying_peer_is_recovered_within_the_safety_interval() {
    let _guard = TEST_LOCK.lock().await;
    let mut stack = Stack::new("silent_peer").await;
    let mut worker = stack.worker("survivor", 30);
    completes(&mut worker, "wf.peer");
    stack.run_with(
        vec![Arc::new(worker)],
        IdlePacing {
            busy: Duration::from_millis(250),
            max_idle: Duration::from_secs(2),
        },
    );
    settle().await;

    let mut tx = stack.pool.begin().await.unwrap();
    tx.execute("ALTER TABLE workflow_run DISABLE TRIGGER workflow_run_work_wake")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO workflow_run \
           (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, budget, \
            correlation_id, causation_id, caused_by, depth, partition, idem_key, \
            lease_owner, lease_expires, lease_epoch) \
         VALUES ($1,$2,'R-peer','wf.peer',1,'[]'::jsonb,'running',0,NULL,'R-peer',NULL, \
                 NULL,0,$3,'R-peer','dead-peer', clock_timestamp() + INTERVAL '1 second', 1)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(PARTITION)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.execute("ALTER TABLE workflow_run ENABLE TRIGGER workflow_run_work_wake")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let took = stack
        .wait_for_state("R-peer", "completed", Duration::from_secs(8))
        .await;
    assert!(
        took < Duration::from_millis(4_500),
        "recovered {took:?} after the silent claim; the bound is 2.4s of jittered safety \
         interval plus the 1s lease"
    );
    stack.stop().await;
}

async fn woken(wake: &WorkWake, within: Duration) -> bool {
    tokio::time::timeout(within, wake.woken()).await.is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_payload_too_long_once_escaped_wakes_everyone_and_never_fails_the_write() {
    let _guard = TEST_LOCK.lock().await;
    let stack = Stack::new("payload").await;
    let mut listener = PgListener::connect_with(&stack.pool).await.unwrap();
    listener.listen(WORK_WAKE_CHANNEL).await.unwrap();

    // 1,500 raw bytes pass a raw-length guard, but each escapes to `\u0001` in JSON: 9,000
    // bytes, past PostgreSQL's 8,000-byte notification limit, so notifying it would raise.
    let tenant = "\u{1}".repeat(1_500);
    sqlx::query(
        "INSERT INTO workflow_run \
           (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, budget, \
            correlation_id, causation_id, caused_by, depth, partition, idem_key) \
         VALUES ($1,$2,'R-escaped','wf.escaped',1,'[]'::jsonb,'running',0,NULL,'R-escaped', \
                 NULL,NULL,0,$3,'R-escaped')",
    )
    .bind(&tenant)
    .bind(REGION)
    .bind(PARTITION)
    .execute(&stack.pool)
    .await
    .expect("the wake trigger never fails the write it observes");
    assert_eq!(
        next_wake(&mut listener).await,
        Some(serde_json::json!([stack.schema, "all"]))
    );
    drop(listener);
    stack.stop().await;
}
