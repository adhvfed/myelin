//! Live PostgreSQL proofs for the durable CI-pipeline drive extensions (P-FLOW CI durability):
//!
//! 1. **Typed CI completion (`SignalPayload::CiJobDone`) through `PgFlowExecutor::signal_typed`** —
//!    accepted, journaled, wakes a parked run to completion, and the stored `wf_signal.payload` row is
//!    byte-shape identical to the legacy `Vec<ArtifactRef>`-of-strings verdict-marker encoding.
//! 2. **Typed rejection** — a bad stage grammar / wrong signal name / unvalidated result ref is a
//!    typed `InvalidInput`, never a silently-accepted or stored bad row.
//! 3. **The signal/park race is closed at commit** — a matching signal buffered while the run was
//!    mid-drive (signal-first interleaving) settles the parked run RUNNABLE, not stranded `waiting`;
//!    a NON-matching pending signal (the R-late guard) leaves it `waiting` (no hot loop).
//! 4. **Claim-side repair** — a hand-crafted run stranded `waiting` with a matching pending signal is
//!    recovered by the worker claim (`waiting → running`) and driven to completion.
//! 5. **Typed terminal no-op** — a late `job.done` to a terminal run is an acknowledged `TerminalNoOp`
//!    that mutates NO `wf_history`, so a late producer can settle its lease instead of retrying forever.
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
/// Serialize the tests in this binary — each applies the flow migrations under a Postgres advisory
/// lock (`PgMigrator::with_migration_lock`), so running them in parallel (the default `cargo test`
/// posture) contends on that global lock and deadlocks. The same idiom as
/// `integration_pg_drive_store.rs:21`; it lets this target pass WITHOUT `--test-threads=1`.
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

/// A `wf.cijob` body: dispatch ONE `kind=ci` job (command 0), then park on its `job.done` (an exact
/// wait keyed on the dispatch `idem_token`). On completion it returns the consumed result verbatim, so
/// a test can assert the typed verdict marker round-tripped through the journal into the body.
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

// ── Proof 1 + 2: the typed CI completion boundary. ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typed_ci_job_done_wakes_parked_run_and_stores_the_legacy_string_array() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("typed_done").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 5, "worker-typed");
    register_cijob(&mut flow, Arc::clone(&runner));

    seed_run(&pool, "acme", "no-osl", "R-typed", 5).await;
    // Drive once: the body dispatches the stage and PARKS on `job.done` (holds no runtime).
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    assert_eq!(run_state(&pool, "R-typed").await, "waiting", "parked on job.done");

    // Deliver the stage verdict as a TYPED CiJobDone through `signal_typed` — the caller never
    // hand-encodes the `ci.stage.verdict:*` marker (which `validate_refs` would reject).
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

    // The stored payload row is byte-shape IDENTICAL to the legacy string-array verdict encoding:
    // `[ci.stage.verdict:pass:build] ++ result_refs`. No typed/dual shape leaked into the column.
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

    // Re-drive: the body replays (0 re-dispatch), consumes the journaled `job.done`, and completes —
    // the typed verdict marker fed the body through the journal (the returned result carries it).
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

    // A stage that is NOT a bounded machine token (a `/` is outside `[A-Za-z0-9_.-]`).
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

    // A CiJobDone on the WRONG signal name is rejected (bound to `job.done` only).
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

    // A result ref that is NOT a valid scoped ArtifactRef (cross-tenant) is rejected as usual.
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

    // None of the rejected deliveries mutated the buffer — the run is still parked, nothing stored.
    let signals: i64 = sqlx::query_scalar("SELECT count(*) FROM wf_signal WHERE run_id='R-reject'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(signals, 0, "no rejected delivery buffered a row");
    assert_eq!(run_state(&pool, "R-reject").await, "waiting", "the run is still parked");
    cleanup(bare, pool, schema).await;
}

// ── Proof 3: the signal/park race is closed at commit (store-level, deterministic interleaving). ──

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn commit_settles_runnable_iff_a_matching_signal_is_pending() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("race").await;
    let store = PgFlowDriveStore::new(pool.clone(), TenantId("acme".into()), Region("no-osl".into()));

    // A helper that: claims a running run, inserts a pending `job.done` (buffered while the run is
    // running/leased — the exact race window), and commits a `waiting` park with the given descriptor.
    // Returns the settled state so the test asserts runnable-vs-waiting.
    async fn park_with_pending(
        pool: &PgPool,
        store: &PgFlowDriveStore,
        run_id: &str,
        partition: i16,
        pending_name: &str,
        pending_idem: &str,
        park: ParkCondition,
    ) -> String {
        // Each case gets its OWN partition so `claim_runnable` targets exactly this run (a run the
        // race fix re-arms `running` must not be re-claimed as another case's candidate).
        seed_run(pool, "acme", "no-osl", run_id, partition).await;
        let lease = store
            .claim_runnable(partition, "worker-race", 60)
            .await
            .expect("claim ok")
            .expect("claimed the seeded run");
        assert_eq!(lease.run_id, run_id);
        // Signal-first interleaving: the signal INSERT commits FIRST (run still `running`+leased, so
        // it buffers but cannot fire the waiting→running wake), THEN the drive's commit runs.
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

    // (a) A pending signal MATCHING the exact park descriptor → the run settles RUNNABLE (the race is
    // closed: the mid-drive signal is not stranded behind a parked run).
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

    // (b) A pending signal that does NOT match the descriptor's idem_key → the run stays `waiting`
    // (descriptor-scoped, not any-signal).
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

    // (c) The R-late guard at the store level: a `job.done` is buffered-and-unconsumed while the body
    // parks on a LATER `finish` signal. An "any unconsumed signal" check would hot-loop; the
    // descriptor-scoped check (name `finish`) leaves the run `waiting`.
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

// ── Proof 4: claim-side repair of a stranded waiting run. ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claim_repairs_a_stranded_waiting_run_with_a_matching_pending_signal() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("repair").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 8, "worker-repair");
    register_cijob(&mut flow, Arc::clone(&runner));

    seed_run(&pool, "acme", "no-osl", "R-strand", 8).await;
    // Drive once → a real park on `job.done` (a journaled `signal_waited` carrying the exact
    // name+idem descriptor markers). The run is `waiting`.
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    assert_eq!(run_state(&pool, "R-strand").await, "waiting");

    // STRAND it: buffer the matching `job.done` DIRECTLY (bypassing `signal`, so the waiting→running
    // wake never fires — exactly how older code / a manual repair leaves a row stranded).
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

    // The next worker pass finds nothing runnable, then the belt-and-braces claim-side REPAIR claims
    // the stranded run (waiting → running) and drives it to completion (replay consumes the signal).
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

// ── Proof 5: typed terminal no-op leaves history unchanged. ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_job_done_to_a_terminal_run_is_an_acknowledged_no_op() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("terminal").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 9, "worker-terminal");
    register_cijob(&mut flow, Arc::clone(&runner));

    seed_run(&pool, "acme", "no-osl", "R-term", 9).await;
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    // Terminate the parked run (a workflow cancel / timeout — the run is now terminal).
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

    // The late `job.done` (the runner finished AFTER the workflow was terminated) — a verified
    // same-tenant terminal delivery is a typed no-op, NOT an error, and mutates nothing.
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

    // DIVERGENT-REDELIVERY GUARD (not bypassed by terminality): buffer a row directly under a key
    // (simulating a signal buffered/consumed BEFORE the run went terminal), then redeliver the SAME
    // key with a DIFFERENT payload — producer corruption that MUST surface as InvalidInput even on a
    // terminal run; an IDENTICAL redelivery is the acknowledged no-op.
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
    // passed:false → canonical `[ci.stage.verdict:fail:build]` diverges from the buffered
    // `[ci.stage.verdict:pass:build]` → surfaced, NOT a no-op.
    let div_err = deliver_div(false).expect_err("a divergent terminal redelivery is surfaced");
    assert!(matches!(div_err, ExecutorError::InvalidInput(m) if m.contains("divergent")));
    // passed:true → canonical matches the buffered row exactly → acknowledged no-op.
    assert_eq!(
        deliver_div(true).expect("an identical terminal redelivery is the acknowledged no-op"),
        SignalOutcome::TerminalNoOp
    );

    // A genuinely invalid target (wrong tenant) is STILL a surfaced error, not a no-op.
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

// ── Proof 6a: a passed:false CiJobDone driven end-to-end through `read_stage_verdict`. ────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn passed_false_ci_job_done_decodes_through_read_stage_verdict_to_completion() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("fail_verdict").await;
    let runner = Arc::new(RecordingJobRunner::default());
    let mut flow = worker(&pool, "acme", "no-osl", 11, "worker-fail");
    // A body that DECODES the stage verdict with `read_stage_verdict` and records pass/fail — so a
    // typed `CiJobDone { passed: false }` is proven to round-trip through the journal into the body's
    // verdict codec (not merely stored).
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

    // Deliver a FAILING typed verdict.
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
    // The body decoded `passed: false` off the canonical verdict marker and returned `test-fail` — so
    // `read_stage_verdict` round-tripped the typed failing verdict end-to-end into the body's codec.
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

// ── Proof 6b: the race-settled `running` row is CLAIMED and DRIVEN to completion end-to-end. ──────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn race_settled_running_row_is_claimed_and_driven_to_completion() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("race_drive").await;
    // A minimal body that parks on ONE exact `job.done` wait at command 0 and returns its payload.
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

    // Reproduce the commit-side race at the store level: claim the running run, buffer a matching
    // `job.done` while it is running/leased, then commit a `waiting` park — the race fix settles it
    // RUNNING. The staged journal (a `signal_waited` at command `wf.wait:0`) is exactly what the real
    // body's first-park would write, so the settled row is a valid replay target.
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

    // END-TO-END: a normal worker pass now CLAIMS the race-settled running row and DRIVES it — replay
    // re-issues the wait, consumes the buffered signal, and the run completes.
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

// ── Proof 7: the park descriptor is IN the drive fingerprint (fail-closed re-entry, versioned). ───

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

    // A `waiting` commit whose ONLY variable is the park descriptor (identical drive_id + history).
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

    // First commit of drive_id `R-fp/d1` with park descriptor `tok-A` → committed.
    assert_eq!(
        store
            .commit_drive(&lease, commit_with("tok-A"))
            .await
            .expect("first commit ok"),
        PgDriveCommitOutcome::Committed
    );
    // IDENTICAL re-entry (same drive_id + same park) → AlreadyCommitted (deterministic re-entry still
    // works; the versioned fingerprint recomputes identically).
    assert_eq!(
        store
            .commit_drive(&lease, commit_with("tok-A"))
            .await
            .expect("identical re-entry"),
        PgDriveCommitOutcome::AlreadyCommitted
    );
    // Same drive_id but a DIFFERENT park descriptor → fingerprint mismatch → fail closed. Without the
    // park in the fingerprint this would be wrongly accepted as AlreadyCommitted.
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

// ── Proof 8: the forced-repair CADENCE recovers a stranded run under a CONTINUOUS backlog. ────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forced_repair_cadence_recovers_a_stranded_run_despite_a_continuous_backlog() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("cadence").await;
    let mut flow = worker(&pool, "acme", "no-osl", 14, "worker-cadence");
    // A trivial always-completing body (the runnable backlog) + the single-wait body (the stranded run).
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

    // Park the wf.wait run (a REAL signal_waited journal with the exact descriptor markers).
    seed_run_typed(&pool, "acme", "no-osl", "R-strand", "wf.wait", 14).await;
    flow.run_once(1000, "2026-07-20T00:00:00Z").await.unwrap();
    assert_eq!(run_state(&pool, "R-strand").await, "waiting");
    // STRAND it: buffer the matching job.done directly (no wake).
    sqlx::query(
        "INSERT INTO wf_signal (tenant_id,region,run_id,signal_name,idem_key,payload,received_at) \
         VALUES ('acme','no-osl','R-strand','job.done','tok-cadence','[]'::jsonb, clock_timestamp())",
    )
    .execute(&pool)
    .await
    .unwrap();

    // A LARGE runnable backlog (>>1 per drive over the whole loop) so a NORMAL claim ALWAYS wins and
    // the fallback repair (which only runs when nothing is runnable) NEVER fires — the stranded run
    // can therefore only be recovered by the every-64th forced-repair cadence probe.
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

    // Drive a burst of run_once passes. Every pass claims+drives a runnable noop (continuous backlog),
    // EXCEPT the cadence probe (every 64th) which sweeps + repairs the stranded run first.
    for i in 0..70 {
        flow.run_once(1001 + i, "2026-07-20T00:01:00Z").await.unwrap();
    }
    assert_eq!(
        run_state(&pool, "R-strand").await,
        "completed",
        "the forced-repair cadence recovered + completed the stranded run under a full backlog"
    );
    // The backlog was NEVER exhausted, so the fallback (nothing-runnable) repair path could not have
    // done it — proving the CADENCE probe (not the fallback) recovered the stranded run.
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

// ── Proof 9: the concurrent-index validation migration (flow_0012) RAISES on a missing/invalid index.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_repair_index_validation_raises_when_the_index_is_missing_or_invalid() {
    let _test_guard = TEST_LOCK.lock().await;
    // setup() applies the full migration set INCLUDING flow_0012, whose DO block validates the
    // freshly-built `wf_waiting_repair` index — so reaching here already proves the guard PASSES on a
    // valid index.
    let (bare, pool, schema) = setup("index_validate").await;
    // Simulate an interrupted CREATE INDEX CONCURRENTLY (which leaves a missing/invalid index that
    // `IF NOT EXISTS` would silently accept) by dropping the index, then re-run the validation DO
    // block: it MUST raise rather than record success over an unusable index.
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
