//! Live PostgreSQL proofs for the fenced workflow drive-storage transaction.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef as EventArtifactRef, CorrelationId, DataRole, EventEnvelope,
    EventId, EventType, OutboxRow, Timestamp, Visibility,
};
use myelin_flow::{
    migrations::migrations, ActivityAttemptWrite, DriveCommit, DriveStoreError, HistoryWrite,
    PgDriveCommitOutcome, PgFlowDriveStore, SignalKey, TimerArm,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::{provider::foundation_migrations, HotTables, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};
use std::sync::atomic::{AtomicU64, Ordering};

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
    let schema = format!("flow_drive_{}_{}_{}", label, std::process::id(), n);
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&bare)
        .await
        .expect("drop stale flow drive schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&bare)
        .await
        .expect("create flow drive schema");
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
        .expect("connect schema-pinned flow pool");
    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .expect("apply the shared outbox foundation");
    PgMigrator::apply_validated(&pool, &migrations(), &HotTables::declare(["workflow_run"]))
        .await
        .expect("apply flow schema and drive fencing migrations");
    (bare, pool, schema)
}

async fn cleanup(bare: PgPool, pool: PgPool, schema: String) {
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&bare)
        .await
        .expect("drop flow drive schema");
}

async fn seed_run(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    run_id: &str,
    state: &str,
    cursor: i64,
    partition: i16,
) {
    sqlx::query(
        "INSERT INTO workflow_run \
           (tenant_id, region, run_id, wf_type, wf_version, input, state, cursor, budget, \
            correlation_id, causation_id, caused_by, depth, partition, idem_key) \
         VALUES ($1, $2, $3, 'ci.pipeline', 7, '[]'::jsonb, $4, $5, NULL, \
            $3, NULL, NULL, 0, $6, $3)",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(state)
    .bind(cursor)
    .bind(partition)
    .execute(pool)
    .await
    .expect("seed workflow run");
}

fn store(pool: &PgPool, tenant: &str, region: &str) -> PgFlowDriveStore {
    PgFlowDriveStore::new(pool.clone(), TenantId(tenant.into()), Region(region.into()))
}

fn event_row(id: &str, run_id: &str) -> OutboxRow {
    let tenant = TenantId("acme".into());
    let subject = EventArtifactRef(format!("myelin://acme/ci/run/{run_id}"));
    let aggregate = AggregateKey(format!("ci-run:{run_id}"));
    let envelope = EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("ci.run.started".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("svc:flow".into()),
            PrincipalKind::Service,
            tenant,
        )),
        subject: subject.clone(),
        aggregate: aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-18T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-18T00:00:01Z".into()),
        payload: serde_json::json!({"run_ref": format!("myelin://acme/ci/run/{run_id}")}),
    };
    OutboxRow {
        event_id: envelope.event_id.clone(),
        aggregate,
        seq: 0,
        subject,
        envelope,
        published_at: None,
        attempts: 0,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claim_is_skip_locked_restart_safe_epoch_fenced_and_scope_isolated() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("claim").await;
    seed_run(&pool, "acme", "fr-par", "R-claim", "running", 0, 3).await;
    seed_run(&pool, "other", "fr-par", "R-other", "running", 0, 3).await;
    seed_run(&pool, "acme", "nl-ams", "R-region", "running", 0, 3).await;
    let acme = store(&pool, "acme", "fr-par");

    let (a, b) = tokio::join!(
        acme.claim_runnable(3, "worker-a", 30),
        acme.claim_runnable(3, "worker-b", 30)
    );
    let claims = [a.expect("claim a"), b.expect("claim b")];
    assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
    let lease = claims.into_iter().flatten().next().expect("one winner");
    assert_eq!(lease.run_id, "R-claim");

    // A new store handle models a process restart; all state comes from PostgreSQL.
    let restarted = store(&pool, "acme", "fr-par");
    assert_eq!(
        restarted
            .load_drive(&lease)
            .await
            .expect("restart load")
            .run,
        lease
    );
    let renewed = restarted
        .renew_lease(&lease, 60)
        .await
        .expect("renew live lease");
    assert!(renewed > lease.lease_expires_unix_ms);

    // Force expiry, then a successor claim increments the fencing epoch. Same owner text cannot
    // revive the old authority (the ABA case).
    sqlx::query(
        "UPDATE workflow_run SET lease_expires = clock_timestamp() - INTERVAL '1 second' \
         WHERE tenant_id = 'acme' AND region = 'fr-par' AND run_id = 'R-claim'",
    )
    .execute(&pool)
    .await
    .expect("expire first claim");
    let successor = restarted
        .claim_runnable(3, &lease.lease_owner, 30)
        .await
        .expect("re-lease after expiry")
        .expect("successor claim");
    assert!(successor.lease_epoch > lease.lease_epoch);
    assert_eq!(
        restarted.release_lease(&lease).await,
        Err(DriveStoreError::LeaseLost)
    );
    assert_eq!(
        restarted.renew_lease(&lease, 30).await,
        Err(DriveStoreError::LeaseLost)
    );

    // Explicit tenant+region predicates prevent the acme/fr-par worker seeing either neighbour.
    assert!(restarted
        .claim_runnable(3, "worker-c", 30)
        .await
        .unwrap()
        .is_none());
    let other = store(&pool, "other", "fr-par")
        .claim_runnable(3, "worker-other", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(other.run_id, "R-other");
    let other_region = store(&pool, "acme", "nl-ams")
        .claim_runnable(3, "worker-region", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(other_region.run_id, "R-region");

    restarted
        .release_lease(&successor)
        .await
        .expect("owner releases successor");
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drive_commit_is_atomic_idempotent_consumes_signal_once_and_stages_outbox() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("commit").await;
    seed_run(&pool, "acme", "fr-par", "R-signal", "running", 1, 4).await;
    sqlx::query(
        "INSERT INTO wf_history \
           (tenant_id, region, run_id, seq, kind, command_id, result) \
         VALUES ('acme', 'fr-par', 'R-signal', 0, 'signal_waited', 'ci.pipeline:0', NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let payload = vec![ArtifactRef("myelin://acme/job/result/1".into())];
    sqlx::query(
        "INSERT INTO wf_signal \
           (tenant_id, region, run_id, signal_name, idem_key, payload) \
         VALUES ('acme', 'fr-par', 'R-signal', 'job.done', 'job-1', $1)",
    )
    .bind(serde_json::to_value(&payload).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wf_activity_attempt \
           (tenant_id, region, run_id, command_id, attempt, idem_token, state, started_at, ended_at) \
         VALUES ('acme', 'fr-par', 'R-signal', 'ci.pipeline:1', 1, 'idem/activity/1', \
                 'succeeded', to_timestamp(1752796800), to_timestamp(1752796800.1))",
    )
    .execute(&pool)
    .await
    .unwrap();

    let drive_store = store(&pool, "acme", "fr-par");
    let lease = drive_store
        .claim_runnable(4, "driver-1", 60)
        .await
        .unwrap()
        .unwrap();
    let snapshot = drive_store
        .load_drive(&lease)
        .await
        .expect("load replay input");
    assert_eq!(snapshot.history.len(), 1);
    assert_eq!(snapshot.history[0].kind, "signal_waited");
    assert_eq!(snapshot.pending_signals.len(), 1);
    assert_eq!(snapshot.pending_signals[0].idem_key, "job-1");

    let mut receipt = vec![ArtifactRef("myelin://flow/signal-idem/job-1".into())];
    receipt.extend(payload);
    for (drive_id, kind, result, consume_signal) in [
        ("wrong-exact-seq", "signal_waited", None, None),
        (
            "wrong-upgrade-seq",
            "signal_received",
            Some(receipt.clone()),
            Some(SignalKey {
                signal_name: "job.done".into(),
                idem_key: "job-1".into(),
            }),
        ),
    ] {
        let wrong_seq = DriveCommit {
            drive_id: drive_id.into(),
            expected_cursor: 1,
            next_state: "running".into(),
            history: vec![HistoryWrite {
                seq: 9,
                kind: kind.into(),
                command_id: "ci.pipeline:0".into(),
                result,
                result_key_ref: None,
                consume_signal,
            }],
            attempts: vec![],
            timers: vec![],
            timer_disarms: Vec::new(),
            outbox: vec![],
            park: None,
        };
        assert!(matches!(
            drive_store.commit_drive(&lease, wrong_seq).await,
            Err(DriveStoreError::JournalConflict(_))
        ));
    }
    let still_pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_signal WHERE tenant_id='acme' AND region='fr-par' \
         AND run_id='R-signal' AND consumed_seq IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(still_pending, 1, "wrong-seq replay consumes no signal");
    let commit = DriveCommit {
        drive_id: "R-signal/cursor-1/drive-1".into(),
        expected_cursor: 1,
        next_state: "waiting".into(),
        history: vec![
            HistoryWrite {
                seq: 0,
                kind: "signal_received".into(),
                command_id: "ci.pipeline:0".into(),
                result: Some(receipt),
                result_key_ref: None,
                consume_signal: Some(SignalKey {
                    signal_name: "job.done".into(),
                    idem_key: "job-1".into(),
                }),
            },
            HistoryWrite {
                seq: 1,
                kind: "activity_completed".into(),
                command_id: "ci.pipeline:1".into(),
                result: Some(vec![ArtifactRef("myelin://acme/job/follow-up/1".into())]),
                result_key_ref: None,
                consume_signal: None,
            },
        ],
        attempts: vec![ActivityAttemptWrite {
            command_id: "ci.pipeline:1".into(),
            attempt: 1,
            idem_token: "idem/activity/1".into(),
            state: "succeeded".into(),
            error: None,
            started_unix_ms: Some(1_752_796_800_000),
            ended_unix_ms: Some(1_752_796_800_100),
        }],
        timers: vec![TimerArm {
            timer_id: "R-signal/ci.pipeline:2/timeout".into(),
            command_id: "ci.pipeline:2".into(),
            fire_at_unix_secs: 1,
            partition: 4,
        }],
        timer_disarms: Vec::new(),
        outbox: vec![event_row("01JFLOWDRIVECOMMIT00000001", "R-signal")],
        park: None,
    };
    assert_eq!(
        drive_store
            .commit_drive(&lease, commit.clone())
            .await
            .unwrap(),
        PgDriveCommitOutcome::Committed
    );
    assert_eq!(
        drive_store.commit_drive(&lease, commit).await.unwrap(),
        PgDriveCommitOutcome::AlreadyCommitted
    );

    let run = sqlx::query(
        "SELECT state, cursor, lease_owner, last_drive_id FROM workflow_run \
         WHERE tenant_id = 'acme' AND region = 'fr-par' AND run_id = 'R-signal'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(run.get::<String, _>("state"), "waiting");
    assert_eq!(
        run.get::<i64, _>("cursor"),
        2,
        "upgrade does not grow history"
    );
    assert!(run.get::<Option<String>, _>("lease_owner").is_none());
    assert_eq!(
        run.get::<String, _>("last_drive_id"),
        "R-signal/cursor-1/drive-1"
    );
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT kind FROM wf_history WHERE tenant_id = 'acme' AND region = 'fr-par' \
         AND run_id = 'R-signal' ORDER BY seq",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(kinds, ["signal_received", "activity_completed"]);
    let consumed: i64 = sqlx::query_scalar(
        "SELECT consumed_seq FROM wf_signal WHERE tenant_id = 'acme' AND region = 'fr-par' \
         AND run_id = 'R-signal' AND signal_name = 'job.done' AND idem_key = 'job-1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        consumed, 0,
        "the upgraded wait's exact journal seq consumes the signal"
    );
    let counts: (i64, i64, i64) = (
        sqlx::query_scalar("SELECT count(*) FROM wf_activity_attempt WHERE tenant_id='acme' AND region='fr-par' AND run_id='R-signal'").fetch_one(&pool).await.unwrap(),
        sqlx::query_scalar("SELECT count(*) FROM wf_timer WHERE tenant_id='acme' AND region='fr-par' AND run_id='R-signal'").fetch_one(&pool).await.unwrap(),
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE event_id='01JFLOWDRIVECOMMIT00000001'").fetch_one(&pool).await.unwrap(),
    );
    assert_eq!(counts, (1, 1, 1));

    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn journal_and_outbox_roll_back_together_and_stale_owner_cannot_commit() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("rollback").await;
    seed_run(&pool, "acme", "fr-par", "R-rollback", "running", 0, 5).await;
    sqlx::query(
        "CREATE FUNCTION reject_rollback_drive() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN \
           IF NEW.last_drive_id = 'rollback-drive' THEN \
             RAISE EXCEPTION 'injected settlement failure after journal and outbox inserts'; \
           END IF; \
           RETURN NEW; \
         END $$",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER reject_rollback_drive BEFORE UPDATE ON workflow_run \
         FOR EACH ROW EXECUTE FUNCTION reject_rollback_drive()",
    )
    .execute(&pool)
    .await
    .unwrap();
    let drive_store = store(&pool, "acme", "fr-par");
    let stale = drive_store
        .claim_runnable(5, "driver-stale", 60)
        .await
        .unwrap()
        .unwrap();

    sqlx::query(
        "INSERT INTO wf_activity_attempt \
           (tenant_id, region, run_id, command_id, attempt, idem_token, state) \
         VALUES ('acme','fr-par','R-rollback','ci.pipeline:existing',1,'idem/existing','scheduled')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let divergent_attempt = DriveCommit {
        drive_id: "divergent-attempt".into(),
        expected_cursor: 0,
        next_state: "running".into(),
        history: vec![],
        attempts: vec![ActivityAttemptWrite {
            command_id: "ci.pipeline:existing".into(),
            attempt: 1,
            idem_token: "idem/existing".into(),
            state: "succeeded".into(),
            error: None,
            started_unix_ms: None,
            ended_unix_ms: None,
        }],
        timers: vec![],
        timer_disarms: Vec::new(),
        outbox: vec![],
        park: None,
    };
    assert_eq!(
        drive_store.commit_drive(&stale, divergent_attempt).await,
        Err(DriveStoreError::AttemptConflict(
            "ci.pipeline:existing".into()
        ))
    );

    let gap = DriveCommit {
        drive_id: "gap-drive".into(),
        expected_cursor: 0,
        next_state: "running".into(),
        history: vec![HistoryWrite {
            seq: 5,
            kind: "side_marker".into(),
            command_id: "ci.pipeline:gap".into(),
            result: None,
            result_key_ref: None,
            consume_signal: None,
        }],
        attempts: vec![],
        timers: vec![],
        timer_disarms: Vec::new(),
        outbox: vec![event_row("01JFLOWGAPROLLBACK000000001", "R-rollback")],
        park: None,
    };
    assert!(matches!(
        drive_store.commit_drive(&stale, gap).await,
        Err(DriveStoreError::JournalConflict(_))
    ));
    let gap_event: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE event_id='01JFLOWGAPROLLBACK000000001'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(gap_event, 0, "a journal gap aborts before any outbox write");

    // The settlement trigger fails only after history, timer, and shared outbox inserts have all
    // executed. The typed tenant transaction rolls every one of them back.
    let failed = DriveCommit {
        drive_id: "rollback-drive".into(),
        expected_cursor: 0,
        next_state: "waiting".into(),
        history: vec![HistoryWrite {
            seq: 0,
            kind: "wf_completed".into(),
            command_id: "ci.pipeline:0".into(),
            result: None,
            result_key_ref: None,
            consume_signal: None,
        }],
        attempts: vec![],
        timers: vec![TimerArm {
            timer_id: "timer-rollback".into(),
            command_id: "new-command".into(),
            fire_at_unix_secs: 600,
            partition: 5,
        }],
        timer_disarms: Vec::new(),
        outbox: vec![event_row("01JFLOWROLLBACK000000000001", "R-rollback")],
        park: None,
    };
    assert!(matches!(
        drive_store.commit_drive(&stale, failed).await,
        Err(DriveStoreError::Storage(_))
    ));
    let history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='fr-par' AND run_id='R-rollback'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let outbox: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE event_id='01JFLOWROLLBACK000000000001'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let timer: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_timer WHERE tenant_id='acme' AND region='fr-par' AND timer_id='timer-rollback'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (history, timer, outbox),
        (0, 0, 0),
        "journal, timer, and outbox all rolled back after the late settlement failure"
    );
    sqlx::query("DROP TRIGGER reject_rollback_drive ON workflow_run")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "UPDATE workflow_run SET lease_expires = clock_timestamp() - INTERVAL '1 second' \
         WHERE tenant_id='acme' AND region='fr-par' AND run_id='R-rollback'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let successor = drive_store
        .claim_runnable(5, "driver-next", 60)
        .await
        .unwrap()
        .unwrap();
    let stale_commit = DriveCommit {
        drive_id: "stale-drive".into(),
        expected_cursor: 0,
        next_state: "completed".into(),
        history: vec![],
        attempts: vec![],
        timers: vec![],
        timer_disarms: Vec::new(),
        outbox: vec![],
        park: None,
    };
    assert_eq!(
        drive_store.commit_drive(&stale, stale_commit).await,
        Err(DriveStoreError::LeaseLost)
    );
    drive_store.release_lease(&successor).await.unwrap();
    cleanup(bare, pool, schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timer_fire_wakes_and_journals_once_without_crossing_tenant_or_region() {
    let _test_guard = TEST_LOCK.lock().await;
    let (bare, pool, schema) = setup("timer").await;
    seed_run(&pool, "acme", "fr-par", "R-timer", "waiting", 1, 6).await;
    sqlx::query(
        "INSERT INTO wf_history \
           (tenant_id, region, run_id, seq, kind, command_id) \
         VALUES ('acme','fr-par','R-timer',0,'timer_set','ci.pipeline:0')",
    )
    .execute(&pool)
    .await
    .unwrap();
    seed_run(&pool, "other", "fr-par", "R-other-timer", "waiting", 0, 6).await;
    for (tenant, run_id, timer_id) in [
        ("acme", "R-timer", "timer-acme"),
        ("other", "R-other-timer", "timer-other"),
    ] {
        sqlx::query(
            "INSERT INTO wf_timer \
               (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
             VALUES ($1, 'fr-par', $2, $3, 'ci.pipeline:0', to_timestamp(1), 0, false, 6)",
        )
        .bind(tenant)
        .bind(timer_id)
        .bind(run_id)
        .execute(&pool)
        .await
        .unwrap();
    }
    let drive_store = store(&pool, "acme", "fr-par");
    let first = drive_store.fire_due_timer(6, 2).await.unwrap().unwrap();
    assert_eq!(first.timer_id, "timer-acme");
    assert!(drive_store.fire_due_timer(6, 2).await.unwrap().is_none());
    let run = sqlx::query(
        "SELECT state, cursor FROM workflow_run \
         WHERE tenant_id='acme' AND region='fr-par' AND run_id='R-timer'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(run.get::<String, _>("state"), "running");
    assert_eq!(run.get::<i64, _>("cursor"), 2);
    let fired_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='fr-par' \
         AND run_id='R-timer' AND kind='timer_fired'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(fired_rows, 1);
    let other_fired: bool = sqlx::query_scalar(
        "SELECT fired FROM wf_timer WHERE tenant_id='other' AND region='fr-par' AND timer_id='timer-other'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !other_fired,
        "acme timer worker cannot fire another tenant's row"
    );

    let lease = drive_store
        .claim_runnable(6, "after-timer", 30)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lease.run_id, "R-timer",
        "the one fire woke the run exactly once"
    );

    // A terminal run's outstanding timeout is cancelled without journal/cursor/state mutation.
    seed_run(&pool, "acme", "fr-par", "R-terminal", "completed", 0, 7).await;
    sqlx::query(
        "INSERT INTO wf_timer \
           (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
         VALUES ('acme','fr-par','timer-terminal','R-terminal','ci.pipeline:0', \
                 to_timestamp(1),0,false,7)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        drive_store
            .fire_due_timer(7, 2)
            .await
            .unwrap()
            .unwrap()
            .timer_id,
        "timer-terminal"
    );
    let terminal: (String, i64) = sqlx::query_as(
        "SELECT state, cursor FROM workflow_run \
         WHERE tenant_id='acme' AND region='fr-par' AND run_id='R-terminal'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let terminal_history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='fr-par' AND run_id='R-terminal'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(terminal, ("completed".into(), 0));
    assert_eq!(terminal_history, 0);

    // A due timer may not append beneath a live drive. Once the lease is released, the already-
    // running run makes the timeout obsolete, so it is disarmed without a journal row.
    seed_run(&pool, "acme", "fr-par", "R-live-timer", "running", 0, 8).await;
    sqlx::query(
        "INSERT INTO wf_timer \
           (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
         VALUES ('acme','fr-par','timer-live','R-live-timer','ci.pipeline:0', \
                 to_timestamp(1),0,false,8)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let live_lease = drive_store
        .claim_runnable(8, "live-driver", 30)
        .await
        .unwrap()
        .unwrap();
    seed_run(&pool, "acme", "fr-par", "R-eligible-timer", "waiting", 0, 8).await;
    sqlx::query(
        "INSERT INTO wf_timer \
           (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
         VALUES ('acme','fr-par','timer-eligible','R-eligible-timer','ci.pipeline:0', \
                 to_timestamp(2),0,false,8)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let eligible = drive_store.fire_due_timer(8, 3).await.unwrap().unwrap();
    assert_eq!(
        eligible.timer_id, "timer-eligible",
        "the older live-leased timer is skipped rather than starving eligible due work"
    );
    let still_due: bool = sqlx::query_scalar(
        "SELECT NOT fired FROM wf_timer WHERE tenant_id='acme' AND region='fr-par' AND timer_id='timer-live'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(still_due);
    let live_history_before_release: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='fr-par' AND run_id='R-live-timer'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live_history_before_release, 0);
    let terminal_arm = DriveCommit {
        drive_id: "terminal-with-timer".into(),
        expected_cursor: 0,
        next_state: "completed".into(),
        history: vec![],
        attempts: vec![],
        timers: vec![TimerArm {
            timer_id: "new-terminal-timer".into(),
            command_id: "ci.pipeline:1".into(),
            fire_at_unix_secs: 10,
            partition: 8,
        }],
        timer_disarms: Vec::new(),
        outbox: vec![],
        park: None,
    };
    assert!(matches!(
        drive_store.commit_drive(&live_lease, terminal_arm).await,
        Err(DriveStoreError::InvalidInput(_))
    ));
    drive_store.release_lease(&live_lease).await.unwrap();
    assert_eq!(
        drive_store
            .fire_due_timer(8, 3)
            .await
            .unwrap()
            .unwrap()
            .timer_id,
        "timer-live"
    );
    let live_history: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_history WHERE tenant_id='acme' AND region='fr-par' AND run_id='R-live-timer'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live_history, 0);
    cleanup(bare, pool, schema).await;
}
