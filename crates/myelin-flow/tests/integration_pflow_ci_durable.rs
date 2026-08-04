#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::{Actor, IdMinter, MonotonicMinter};
use myelin_flow::{
    job_idem_token, migrations::migrations, ActivityError, DriveCommit, DriveOutcome,
    DriveStoreError, DurableExecutor, ExecutorError, HistoryWrite, JobKind, JobOutcome, JobRunner,
    JobSpec, ParkCondition, PgDriveCommitOutcome, PgFlowDriveStore, PgFlowWorker, PgRunOnceOutcome,
    PgWorkerScope, RunId, SignalOutcome, SignalPayload, TypedSignalSpec,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_refs::ArtifactRef;
use myelin_storage::{provider::foundation_migrations, HotTables, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};
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
    let schema = format!("flow_pflow_ci_{label}_{}_{}", std::process::id(), n);
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
    let scope = PgWorkerScope::new(
        TenantId(tenant.into()),
        Region(region.into()),
        partition,
        name,
        60,
        actor(tenant, region),
        1,
    )
    .expect("valid worker scope");
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    PgFlowWorker::new(pool.clone(), tokio::runtime::Handle::current(), minter, scope)
}

fn register_cijob(flow: &mut PgFlowWorker, runner: Arc<RecordingJobRunner>) {
    flow.register_definition("wf.cijob", 1, "cijob-v1", move |_input, ctx| {
        let job = ctx
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/build"),
                runner.as_ref(),
                None,
            )
            .map_err(|error| format!("{error:?}"))?;
        match ctx
            .join_dispatched_job(&job)
            .map_err(|error| format!("{error:?}"))?
        {
            JobOutcome::Completed { result, .. } => Ok(result),
            JobOutcome::Parked => Ok(Vec::new()),
            JobOutcome::TimedOut => Err("unbounded wait cannot time out".into()),
        }
    })
    .expect("register wf.cijob body");
}

async fn seed_run(pool: &PgPool, tenant: &str, region: &str, run_id: &str, partition: i16) {
    seed_run_typed(pool, tenant, region, run_id, "wf.cijob", partition).await;
}

async fn seed_run_typed(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    run_id: &str,
    wf_type: &str,
    partition: i16,
) {
    sqlx::query(
        "INSERT INTO workflow_run \
           (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, budget, \
            correlation_id, causation_id, caused_by, depth, partition, idem_key) \
         VALUES ($1,$2,$3,$4,1,'[]'::jsonb,'running',0,NULL,$3,NULL,NULL,0,$5,$3)",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(wf_type)
    .bind(partition)
    .execute(pool)
    .await
    .expect("seed run");
}

async fn run_state(pool: &PgPool, run_id: &str) -> String {
    sqlx::query_scalar("SELECT state FROM workflow_run WHERE run_id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
        .expect("read run state")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typed_ci_job_done_wakes_parked_run_and_stores_the_legacy_string_array() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("typed_done").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 5, "worker-typed");
    register_cijob(&mut flow, Arc::clone(&runner));

    seed_run(&pool, "acme", "no-osl", "R-typed", 5).await;
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    assert_eq!(run_state(&pool, "R-typed").await, "waiting", "parked on job.done");

    let token = job_idem_token("R-typed", "wf.cijob:0");
    let outcome = tokio::task::block_in_place(|| {
        flow.executor().signal_typed(TypedSignalSpec {
            run: RunId("R-typed".into()),
            signal_name: "job.done".into(),
            idem_key: token.clone(),
            payload: SignalPayload::CiJobDone {
                stage: "build".into(),
                passed: true,
                result_refs: vec![ArtifactRef("myelin://acme/ci/artifact/build".into())],
            },
            payload_key_ref: None,
        })
    })
    .expect("typed CiJobDone accepted");
    assert_eq!(outcome, SignalOutcome::Buffered, "the first typed delivery buffered");
    assert_eq!(
        run_state(&pool, "R-typed").await,
        "running",
        "the typed delivery woke the parked run (waiting → running)"
    );

    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM wf_signal WHERE run_id='R-typed' AND signal_name='job.done' AND idem_key=$1",
    )
    .bind(&token)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        serde_json::json!([
            "ci.stage.verdict:pass:build",
            "myelin://acme/ci/artifact/build"
        ]),
        "the durable payload is the legacy Vec<ArtifactRef>-of-strings encoding, byte-shape unchanged"
    );

    let driven = flow.run_once(1001, "2026-07-20T00:00:01Z").await.unwrap();
    assert!(matches!(driven, PgRunOnceOutcome::Driven { .. }));
    assert_eq!(run_state(&pool, "R-typed").await, "completed", "the run ran to completion");
    let received: serde_json::Value = sqlx::query_scalar(
        "SELECT result FROM wf_history WHERE run_id='R-typed' AND kind='signal_received'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let received = received.to_string();
    assert!(
        received.contains("ci.stage.verdict:pass:build"),
        "the journaled signal_received carries the typed verdict marker, replay-stable"
    );
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1, "exactly one dispatch");
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typed_ci_job_done_rejects_bad_grammar_wrong_name_and_unvalidated_ref() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("typed_reject").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 6, "worker-reject");
    register_cijob(&mut flow, Arc::clone(&runner));
    seed_run(&pool, "acme", "no-osl", "R-reject", 6).await;
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    let token = job_idem_token("R-reject", "wf.cijob:0");

    let deliver = |payload: SignalPayload, name: &str| {
        let name = name.to_string();
        let token = token.clone();
        tokio::task::block_in_place(|| {
            flow.executor().signal_typed(TypedSignalSpec {
                run: RunId("R-reject".into()),
                signal_name: name,
                idem_key: token,
                payload,
                payload_key_ref: None,
            })
        })
    };

    let bad_stage = deliver(
        SignalPayload::CiJobDone {
            stage: "build/linux".into(),
            passed: true,
            result_refs: vec![],
        },
        "job.done",
    )
    .expect_err("a non-machine-token stage is rejected");
    assert!(matches!(bad_stage, ExecutorError::InvalidInput(m) if m.contains("machine token")));

    let wrong_name = deliver(
        SignalPayload::CiJobDone {
            stage: "build".into(),
            passed: true,
            result_refs: vec![],
        },
        "approval",
    )
    .expect_err("a CiJobDone on a non-job.done signal is rejected");
    assert!(matches!(wrong_name, ExecutorError::InvalidInput(m) if m.contains("job.done")));

    let bad_ref = deliver(
        SignalPayload::CiJobDone {
            stage: "build".into(),
            passed: false,
            result_refs: vec![ArtifactRef("myelin://other/ci/artifact/x".into())],
        },
        "job.done",
    )
    .expect_err("an unvalidated result ref is rejected");
    assert!(matches!(bad_ref, ExecutorError::InvalidInput(_)));

    let signals: i64 = sqlx::query_scalar("SELECT count(*) FROM wf_signal WHERE run_id='R-reject'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(signals, 0, "no rejected delivery buffered a row");
    assert_eq!(run_state(&pool, "R-reject").await, "waiting", "the run is still parked");
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_settles_runnable_iff_a_matching_signal_is_pending() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("race").await;
    let store = PgFlowDriveStore::new(pool.clone(), TenantId("acme".into()), Region("no-osl".into()));

    async fn park_with_pending(
        pool: &PgPool,
        store: &PgFlowDriveStore,
        run_id: &str,
        partition: i16,
        pending_name: &str,
        pending_idem: &str,
        park: ParkCondition,
    ) -> String {
        seed_run(pool, "acme", "no-osl", run_id, partition).await;
        let lease = store
            .claim_runnable(partition, "worker-race", 60)
            .await
            .expect("claim ok")
            .expect("claimed the seeded run");
        assert_eq!(lease.run_id, run_id);
        sqlx::query(
            "INSERT INTO wf_signal (tenant_id,region,run_id,signal_name,idem_key,payload,received_at) \
             VALUES ('acme','no-osl',$1,$2,$3,'[]'::jsonb, clock_timestamp())",
        )
        .bind(run_id)
        .bind(pending_name)
        .bind(pending_idem)
        .execute(pool)
        .await
        .expect("buffer pending signal");
        let commit = DriveCommit {
            drive_id: format!("{run_id}/cursor-0/epoch-{}", lease.lease_epoch),
            expected_cursor: 0,
            next_state: "waiting".into(),
            history: vec![HistoryWrite {
                seq: 0,
                kind: "signal_waited".into(),
                command_id: "wf.cijob:1".into(),
                result: None,
                result_key_ref: None,
                consume_signal: None,
            }],
            attempts: vec![],
            timers: vec![],
            timer_disarms: vec![],
            outbox: vec![],
            park: Some(park),
        };
        assert_eq!(
            store.commit_drive(&lease, commit).await.expect("commit ok"),
            PgDriveCommitOutcome::Committed
        );
        run_state(pool, run_id).await
    }

    let matched = park_with_pending(
        &pool,
        &store,
        "R-match",
        7,
        "job.done",
        "tok-1",
        ParkCondition::Signal {
            name: "job.done".into(),
            idem_key: Some("tok-1".into()),
        },
    )
    .await;
    assert_eq!(matched, "running", "a matching mid-drive signal settles the run runnable");

    let wrong_idem = park_with_pending(
        &pool,
        &store,
        "R-wrong-idem",
        8,
        "job.done",
        "tok-OTHER",
        ParkCondition::Signal {
            name: "job.done".into(),
            idem_key: Some("tok-1".into()),
        },
    )
    .await;
    assert_eq!(wrong_idem, "waiting", "a non-matching idem_key does not re-arm the run");

    let unrelated = park_with_pending(
        &pool,
        &store,
        "R-finish",
        9,
        "job.done",
        "tok-late",
        ParkCondition::Signal {
            name: "finish".into(),
            idem_key: None,
        },
    )
    .await;
    assert_eq!(
        unrelated, "waiting",
        "an unrelated buffered job.done does not re-arm a run parked on `finish` (no hot loop)"
    );
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claim_repairs_a_stranded_waiting_run_with_a_matching_pending_signal() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("repair").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 8, "worker-repair");
    register_cijob(&mut flow, Arc::clone(&runner));

    seed_run(&pool, "acme", "no-osl", "R-strand", 8).await;
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    assert_eq!(run_state(&pool, "R-strand").await, "waiting");

    let token = job_idem_token("R-strand", "wf.cijob:0");
    sqlx::query(
        "INSERT INTO wf_signal (tenant_id,region,run_id,signal_name,idem_key,payload,received_at) \
         VALUES ('acme','no-osl','R-strand','job.done',$1, \
                 '[\"ci.stage.verdict:pass:build\"]'::jsonb, clock_timestamp())",
    )
    .bind(&token)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(run_state(&pool, "R-strand").await, "waiting", "stranded: waiting with a pending match");

    let driven = flow.run_once(1001, "2026-07-20T00:00:01Z").await.unwrap();
    assert!(matches!(driven, PgRunOnceOutcome::Driven { .. }), "the repair claim drove the run");
    assert_eq!(run_state(&pool, "R-strand").await, "completed", "the stranded run recovered + completed");
    let consumed: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_seq FROM wf_signal WHERE run_id='R-strand' AND idem_key=$1",
    )
    .bind(&token)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(consumed.is_some(), "the repaired drive consumed the previously-stranded signal");
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_job_done_to_a_terminal_run_is_an_acknowledged_no_op() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("terminal").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 9, "worker-terminal");
    register_cijob(&mut flow, Arc::clone(&runner));

    seed_run(&pool, "acme", "no-osl", "R-term", 9).await;
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    tokio::task::block_in_place(|| {
        flow.executor()
            .cancel(&RunId("R-term".into()), "workflow_timeout")
    })
    .expect("cancel the run");
    assert_eq!(run_state(&pool, "R-term").await, "terminated");
    let history_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM wf_history WHERE run_id='R-term'")
            .fetch_one(&pool)
            .await
            .unwrap();

    let token = job_idem_token("R-term", "wf.cijob:0");
    let outcome = tokio::task::block_in_place(|| {
        flow.executor().signal_typed(TypedSignalSpec {
            run: RunId("R-term".into()),
            signal_name: "job.done".into(),
            idem_key: token,
            payload: SignalPayload::CiJobDone {
                stage: "build".into(),
                passed: true,
                result_refs: vec![],
            },
            payload_key_ref: None,
        })
    })
    .expect("a verified terminal delivery is an acknowledged no-op");
    assert_eq!(outcome, SignalOutcome::TerminalNoOp);
    let signals: i64 = sqlx::query_scalar("SELECT count(*) FROM wf_signal WHERE run_id='R-term'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(signals, 0, "the terminal no-op buffered no signal row");
    let history_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM wf_history WHERE run_id='R-term'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(history_after, history_before, "terminal history is unchanged");

    sqlx::query(
        "INSERT INTO wf_signal (tenant_id,region,run_id,signal_name,idem_key,payload,received_at) \
         VALUES ('acme','no-osl','R-term','job.done','tok-div', \
                 '[\"ci.stage.verdict:pass:build\"]'::jsonb, clock_timestamp())",
    )
    .execute(&pool)
    .await
    .unwrap();
    let deliver_div = |passed: bool| {
        tokio::task::block_in_place(|| {
            flow.executor().signal_typed(TypedSignalSpec {
                run: RunId("R-term".into()),
                signal_name: "job.done".into(),
                idem_key: "tok-div".into(),
                payload: SignalPayload::CiJobDone {
                    stage: "build".into(),
                    passed,
                    result_refs: vec![],
                },
                payload_key_ref: None,
            })
        })
    };
    let div_err = deliver_div(false).expect_err("a divergent terminal redelivery is surfaced");
    assert!(matches!(div_err, ExecutorError::InvalidInput(m) if m.contains("divergent")));
    assert_eq!(
        deliver_div(true).expect("an identical terminal redelivery is the acknowledged no-op"),
        SignalOutcome::TerminalNoOp
    );

    let wrong_tenant_exec = PgFlowWorker::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
        Arc::new(MonotonicMinter::new()),
        PgWorkerScope::new(
            TenantId("other".into()),
            Region("no-osl".into()),
            9,
            "worker-other",
            60,
            actor("other", "no-osl"),
            1,
        )
        .unwrap(),
    );
    let err = tokio::task::block_in_place(|| {
        wrong_tenant_exec.executor().signal(myelin_flow::SignalSpec {
            run: RunId("R-term".into()),
            signal_name: "job.done".into(),
            idem_key: "x".into(),
            payload: vec![],
            payload_key_ref: None,
        })
    })
    .expect_err("a cross-tenant delivery is surfaced, never a no-op");
    assert!(matches!(err, ExecutorError::UnknownRun(_)));
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn passed_false_ci_job_done_decodes_through_read_stage_verdict_to_completion() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("fail_verdict").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 11, "worker-fail");
    let body_runner = Arc::clone(&runner);
    flow.register_definition("wf.cijobv", 1, "cijobv-v1", move |_input, ctx| {
        let job = ctx
            .dispatch_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/test"),
                body_runner.as_ref(),
                None,
            )
            .map_err(|error| format!("{error:?}"))?;
        match ctx
            .join_dispatched_job(&job)
            .map_err(|error| format!("{error:?}"))?
        {
            JobOutcome::Completed { result, .. } => {
                let (stage, passed) = myelin_flow::read_stage_verdict(&result)
                    .ok_or_else(|| "no verdict marker in job.done result".to_string())?;
                let verdict = if passed { "pass" } else { "fail" };
                Ok(vec![ArtifactRef(format!(
                    "myelin://acme/ci/decoded/{stage}-{verdict}"
                ))])
            }
            JobOutcome::Parked => Ok(Vec::new()),
            JobOutcome::TimedOut => Err("unbounded wait cannot time out".into()),
        }
    })
    .expect("register wf.cijobv body");

    seed_run_typed(&pool, "acme", "no-osl", "R-fail", "wf.cijobv", 11).await;
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    assert_eq!(run_state(&pool, "R-fail").await, "waiting", "parked on job.done");

    let token = job_idem_token("R-fail", "wf.cijobv:0");
    tokio::task::block_in_place(|| {
        flow.executor().signal_typed(TypedSignalSpec {
            run: RunId("R-fail".into()),
            signal_name: "job.done".into(),
            idem_key: token,
            payload: SignalPayload::CiJobDone {
                stage: "test".into(),
                passed: false,
                result_refs: vec![],
            },
            payload_key_ref: None,
        })
    })
    .expect("typed failing verdict accepted");

    let driven = flow.run_once(1001, "2026-07-20T00:00:01Z").await.unwrap();
    assert_eq!(run_state(&pool, "R-fail").await, "completed", "the run completed");
    match driven {
        PgRunOnceOutcome::Driven {
            outcome: DriveOutcome::Completed(result),
            ..
        } => assert!(
            result
                .iter()
                .any(|r| r.0 == "myelin://acme/ci/decoded/test-fail"),
            "read_stage_verdict decoded the failing verdict end-to-end, got {result:?}"
        ),
        other => panic!("expected a completed drive, got {other:?}"),
    }
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn race_settled_running_row_is_claimed_and_driven_to_completion() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("race_drive").await;
    let mut flow = worker(&pool, "acme", "no-osl", 12, "worker-race-drive");
    flow.register_definition("wf.wait", 1, "wait-v1", |_input, ctx| {
        match ctx
            .wait_for_signal_exact("job.done", "tok-race", None)
            .map_err(|error| format!("{error:?}"))?
        {
            myelin_flow::WaitOutcome::Signalled { payload, .. } => Ok(payload),
            myelin_flow::WaitOutcome::Parked => Ok(Vec::new()),
            myelin_flow::WaitOutcome::TimedOut => Err("unbounded wait cannot time out".into()),
        }
    })
    .expect("register wf.wait body");

    let store =
        PgFlowDriveStore::new(pool.clone(), TenantId("acme".into()), Region("no-osl".into()));
    seed_run_typed(&pool, "acme", "no-osl", "R-drive", "wf.wait", 12).await;
    let lease = store
        .claim_runnable(12, "worker-race-drive", 60)
        .await
        .unwrap()
        .expect("claimed the seeded run");
    sqlx::query(
        "INSERT INTO wf_signal (tenant_id,region,run_id,signal_name,idem_key,payload,received_at) \
         VALUES ('acme','no-osl','R-drive','job.done','tok-race', \
                 '[\"myelin://acme/ci/artifact/race\"]'::jsonb, clock_timestamp())",
    )
    .execute(&pool)
    .await
    .unwrap();
    let commit = DriveCommit {
        drive_id: format!("R-drive/cursor-0/epoch-{}", lease.lease_epoch),
        expected_cursor: 0,
        next_state: "waiting".into(),
        history: vec![HistoryWrite {
            seq: 0,
            kind: "signal_waited".into(),
            command_id: "wf.wait:0".into(),
            result: None,
            result_key_ref: None,
            consume_signal: None,
        }],
        attempts: vec![],
        timers: vec![],
        timer_disarms: vec![],
        outbox: vec![],
        park: Some(ParkCondition::Signal {
            name: "job.done".into(),
            idem_key: Some("tok-race".into()),
        }),
    };
    assert_eq!(
        store.commit_drive(&lease, commit).await.expect("commit ok"),
        PgDriveCommitOutcome::Committed
    );
    assert_eq!(
        run_state(&pool, "R-drive").await,
        "running",
        "the race fix settled the parked run runnable"
    );

    let driven = flow.run_once(2000, "2026-07-20T00:00:02Z").await.unwrap();
    assert!(
        matches!(driven, PgRunOnceOutcome::Driven { .. }),
        "the race-settled running row was claimed and driven"
    );
    assert_eq!(
        run_state(&pool, "R-drive").await,
        "completed",
        "the race-settled run drove to completion"
    );
    let consumed: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_seq FROM wf_signal WHERE run_id='R-drive' AND idem_key='tok-race'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(consumed.is_some(), "the driven run consumed the raced signal");
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_drive_id_with_a_different_park_descriptor_fails_closed() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("fingerprint").await;
    let store =
        PgFlowDriveStore::new(pool.clone(), TenantId("acme".into()), Region("no-osl".into()));
    seed_run_typed(&pool, "acme", "no-osl", "R-fp", "wf.wait", 13).await;
    let lease = store
        .claim_runnable(13, "worker-fp", 60)
        .await
        .unwrap()
        .expect("claimed the seeded run");

    let commit_with = |park_idem: &str| DriveCommit {
        drive_id: "R-fp/d1".into(),
        expected_cursor: 0,
        next_state: "waiting".into(),
        history: vec![HistoryWrite {
            seq: 0,
            kind: "signal_waited".into(),
            command_id: "wf.wait:0".into(),
            result: None,
            result_key_ref: None,
            consume_signal: None,
        }],
        attempts: vec![],
        timers: vec![],
        timer_disarms: vec![],
        outbox: vec![],
        park: Some(ParkCondition::Signal {
            name: "job.done".into(),
            idem_key: Some(park_idem.into()),
        }),
    };

    assert_eq!(
        store
            .commit_drive(&lease, commit_with("tok-A"))
            .await
            .expect("first commit ok"),
        PgDriveCommitOutcome::Committed
    );
    assert_eq!(
        store
            .commit_drive(&lease, commit_with("tok-A"))
            .await
            .expect("identical re-entry"),
        PgDriveCommitOutcome::AlreadyCommitted
    );
    let err = store
        .commit_drive(&lease, commit_with("tok-B"))
        .await
        .expect_err("a different park under the same drive_id must fail closed");
    assert!(
        matches!(&err, DriveStoreError::DuplicateDrive(id) if id == "R-fp/d1"),
        "same drive_id + different park must be DuplicateDrive, got {err:?}"
    );
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forced_repair_cadence_recovers_a_stranded_run_despite_a_continuous_backlog() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("cadence").await;
    let mut flow = worker(&pool, "acme", "no-osl", 14, "worker-cadence");
    flow.register_definition("wf.noop", 1, "noop-v1", |_input, _ctx| Ok(Vec::new()))
        .expect("register wf.noop");
    flow.register_definition("wf.wait", 1, "wait-v1", |_input, ctx| {
        match ctx
            .wait_for_signal_exact("job.done", "tok-cadence", None)
            .map_err(|error| format!("{error:?}"))?
        {
            myelin_flow::WaitOutcome::Signalled { payload, .. } => Ok(payload),
            myelin_flow::WaitOutcome::Parked => Ok(Vec::new()),
            myelin_flow::WaitOutcome::TimedOut => Err("unbounded".into()),
        }
    })
    .expect("register wf.wait");

    seed_run_typed(&pool, "acme", "no-osl", "R-strand", "wf.wait", 14).await;
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    assert_eq!(run_state(&pool, "R-strand").await, "waiting");
    sqlx::query(
        "INSERT INTO wf_signal (tenant_id,region,run_id,signal_name,idem_key,payload,received_at) \
         VALUES ('acme','no-osl','R-strand','job.done','tok-cadence','[]'::jsonb, clock_timestamp())",
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO workflow_run \
           (tenant_id,region,run_id,wf_type,wf_version,input,state,cursor,budget, \
            correlation_id,causation_id,caused_by,depth,partition,idem_key) \
         SELECT 'acme','no-osl','noop-'||g,'wf.noop',1,'[]'::jsonb,'running',0,NULL, \
                'noop-'||g,NULL,NULL,0,14,'noop-'||g \
         FROM generate_series(1,300) g",
    )
    .execute(&pool)
    .await
    .unwrap();

    for i in 0..70 {
        flow.run_once(1001 + i, "2026-07-20T00:01:00Z").await.unwrap();
    }
    assert_eq!(
        run_state(&pool, "R-strand").await,
        "completed",
        "the forced-repair cadence recovered + completed the stranded run under a full backlog"
    );
    let backlog: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_run WHERE wf_type='wf.noop' AND state='running'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        backlog > 0,
        "the runnable backlog persisted throughout, so the fallback repair never fired"
    );
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_repair_index_validation_raises_when_the_index_is_missing_or_invalid() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("index_validate").await;
    sqlx::query("DROP INDEX wf_waiting_repair")
        .execute(&pool)
        .await
        .expect("drop the repair index");
    let err = sqlx::query(myelin_flow::migrations::VALIDATE_WORKFLOW_RUN_WAITING_REPAIR_INDEX_DDL)
        .execute(&pool)
        .await
        .expect_err("the validation DO block must RAISE when the index is missing/invalid");
    assert!(
        err.to_string().contains("wf_waiting_repair"),
        "the guard names the offending index, got: {err}"
    );
    cleanup(bare, pool, schema).await;
}
