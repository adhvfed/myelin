//! Live-Postgres proof for the durable workflow control surface.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::{ConsumerName, DedupLedger, DurableDedup, EventId, HandlerTx, MonotonicMinter};
use myelin_flow::{
    migrations::migrations, DurableExecutor, ExecutorError, PgFlowExecutor, RunId, SignalOutcome,
    SignalSpec, StartSpec,
};
use myelin_refs::ArtifactRef;
use myelin_storage::{
    events_durable::DurableDedupBacking, provider::foundation_migrations, HotTables, PgMigrator,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};
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
    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .expect("apply shared durable consumer foundation");
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

    // The transaction-aware entry point writes on the caller's exact connection. Rolling the
    // caller back removes the workflow start too; no nested transaction escaped the co-commit.
    let mut caller_tx = pool.begin().await.expect("begin caller co-commit tx");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind("acme")
    .bind("fr-par")
    .execute(&mut *caller_tx)
    .await
    .expect("scope caller co-commit tx");
    let rolled_back_run = tokio::task::block_in_place(|| {
        let mut handler_tx = HandlerTx::with_connection(&mut *caller_tx);
        let first = first_process
            .start_with_id_on_conn(
                &mut handler_tx,
                start_spec("trigger:rolled-back"),
                Some(RunId("run-rolled-back".into())),
            )
            .expect("start on caller transaction");
        let replay = first_process
            .start_with_id_on_conn(
                &mut handler_tx,
                start_spec("trigger:rolled-back"),
                Some(RunId("different-requested-id".into())),
            )
            .expect("idempotency anchor wins within caller transaction");
        assert_eq!(replay, first);

        let invalid = first_process.start_with_id_on_conn(
            &mut handler_tx,
            StartSpec {
                wf_type: "ci.pipeline".into(),
                input: vec![ArtifactRef("myelin://other/ci/run/cross-tenant".into())],
                budget: None,
                idem_key: "trigger:invalid".into(),
            },
            None,
        );
        assert!(matches!(invalid, Err(ExecutorError::InvalidInput(_))));

        let unknown = first_process.start_with_id_on_conn(
            &mut handler_tx,
            StartSpec {
                wf_type: "unknown.workflow".into(),
                input: Vec::new(),
                budget: None,
                idem_key: "trigger:unknown".into(),
            },
            None,
        );
        assert_eq!(
            unknown,
            Err(ExecutorError::UnknownWorkflow("unknown.workflow".into()))
        );
        first
    });
    assert_eq!(rolled_back_run.0, "run-rolled-back");
    let persisted = sqlx::query(
        "SELECT wf_version, partition, input FROM workflow_run \
         WHERE tenant_id = 'acme' AND region = 'fr-par' AND run_id = 'run-rolled-back'",
    )
    .fetch_one(&mut *caller_tx)
    .await
    .expect("read caller-owned start before rollback");
    assert_eq!(persisted.get::<i32, _>("wf_version"), 1);
    use std::hash::{Hash, Hasher};
    let mut partition_hasher = std::collections::hash_map::DefaultHasher::new();
    "run-rolled-back".hash(&mut partition_hasher);
    let expected_partition =
        (partition_hasher.finish() % u64::from(myelin_flow::PARTITION_COUNT)) as i16;
    assert_eq!(persisted.get::<i16, _>("partition"), expected_partition);
    assert_eq!(
        persisted.get::<serde_json::Value, _>("input"),
        serde_json::json!([])
    );
    caller_tx.rollback().await.expect("roll back caller tx");
    let rolled_back_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_run WHERE tenant_id = 'acme' AND region = 'fr-par' \
         AND idem_key = 'trigger:rolled-back'",
    )
    .fetch_one(&pool)
    .await
    .expect("verify caller rollback");
    assert_eq!(
        rolled_back_count, 0,
        "caller rollback removes workflow start"
    );

    // The production consumer path owns the transaction: its durable dedup mark and the workflow
    // start land together. A committed redelivery is deduplicated before it can create another run.
    let ledger = DedupLedger::durable(Arc::new(DurableDedupBacking::new(
        pool.clone(),
        tokio::runtime::Handle::current(),
    )) as Arc<dyn DurableDedup>);
    let consumer = ConsumerName("flow.trigger".into());
    let event_id = EventId("event-committed-start".into());
    let committed_run = tokio::task::block_in_place(|| {
        let (mut co_tx, fresh) = ledger.begin_co_commit(
            &consumer,
            &event_id,
            &TenantId("acme".into()),
            &Region("fr-par".into()),
        );
        assert!(fresh);
        let run = {
            let conn = co_tx.connection().expect("durable co-commit connection");
            let mut handler_tx = HandlerTx::with_connection(conn);
            let run = first_process
                .start_with_id_on_conn(
                    &mut handler_tx,
                    start_spec("trigger:committed"),
                    Some(RunId("run-committed".into())),
                )
                .expect("start on durable consumer transaction");
            assert_eq!(
                first_process
                    .start_with_id_on_conn(
                        &mut handler_tx,
                        start_spec("trigger:committed"),
                        Some(RunId("ignored-redelivery-id".into())),
                    )
                    .expect("idempotency key wins in the caller transaction"),
                run
            );
            assert_eq!(
                first_process.start_with_id_on_conn(
                    &mut handler_tx,
                    start_spec("trigger:colliding"),
                    Some(run.clone()),
                ),
                Err(ExecutorError::RunIdConflict(run.0.clone())),
                "caller-tx and standalone starts share run-id collision semantics"
            );
            run
        };
        co_tx
            .commit()
            .expect("commit dedup mark and workflow start");
        run
    });
    assert_eq!(committed_run.0, "run-committed");
    let committed_counts: (i64, i64) = (
        sqlx::query_scalar(
            "SELECT count(*) FROM workflow_run WHERE tenant_id='acme' AND region='fr-par' \
             AND idem_key='trigger:committed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        sqlx::query_scalar(
            "SELECT count(*) FROM consumer_dedup WHERE consumer='flow.trigger' \
             AND event_id='event-committed-start'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
    );
    assert_eq!(
        committed_counts,
        (1, 1),
        "start and dedup mark co-committed"
    );
    tokio::task::block_in_place(|| {
        let (co_tx, fresh) = ledger.begin_co_commit(
            &consumer,
            &event_id,
            &TenantId("acme".into()),
            &Region("fr-par".into()),
        );
        assert!(!fresh, "committed delivery is durably deduplicated");
        co_tx.rollback();
    });
    let after_redelivery: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_run WHERE tenant_id='acme' AND region='fr-par' \
         AND idem_key='trigger:committed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after_redelivery, 1);

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

    let mut divergent_payload = signal();
    divergent_payload.payload = vec![ArtifactRef("myelin://acme/ci/artifact/divergent".into())];
    let divergent_payload_result =
        tokio::task::block_in_place(|| restarted.signal(divergent_payload));
    assert!(
        matches!(
            &divergent_payload_result,
            Err(ExecutorError::InvalidInput(message))
                if message.contains("divergent payload")
        ),
        "unexpected divergent payload result: {divergent_payload_result:?}"
    );
    let mut divergent_key_ref = signal();
    divergent_key_ref.payload_key_ref = Some("kms://acme/0/subject:signal-owner".into());
    assert!(matches!(
        tokio::task::block_in_place(|| restarted.signal(divergent_key_ref)),
        Err(ExecutorError::InvalidInput(message))
            if message.contains("divergent payload")
    ));

    // Once history has consumed this exact signal, its at-least-once replay remains a successful
    // Duplicate but cannot wake the run again.
    sqlx::query(
        "UPDATE wf_signal SET consumed_seq = 5 WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
         AND signal_name = 'job.done' AND idem_key = 'job-token-1'",
    )
    .bind("acme")
    .bind("fr-par")
    .bind(&run.0)
    .execute(&pool)
    .await
    .expect("mark the signal consumed");
    sqlx::query(
        "UPDATE workflow_run SET state = 'waiting' WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind("acme")
    .bind("fr-par")
    .bind(&run.0)
    .execute(&pool)
    .await
    .expect("park after consuming the signal");
    assert_eq!(
        tokio::task::block_in_place(|| restarted.signal(signal()))
            .expect("consumed exact duplicate remains idempotent"),
        SignalOutcome::Duplicate
    );
    assert_eq!(
        tokio::task::block_in_place(|| restarted.describe(&run))
            .expect("describe after consumed duplicate")
            .state,
        "waiting",
        "a consumed duplicate must not resurrect a parked run"
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
    assert!(matches!(
        tokio::task::block_in_place(|| after_second_restart.signal(signal())),
        Err(ExecutorError::InvalidInput(message))
            if message.contains("terminal workflow run")
    ));
    let signal_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wf_signal WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind("acme")
    .bind("fr-par")
    .bind(&run.0)
    .fetch_one(&pool)
    .await
    .expect("count immutable terminal signal rows");
    assert_eq!(signal_count, 1, "terminal delivery mutates no signal rows");

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
