#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use myelin_config::MyelinConfig;
use myelin_events::relay::{Delivery, EventPublisher, TransportError};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, InProcessBus, Timestamp, Visibility, OUTBOX_MIGRATION, OUTBOX_QUARANTINE_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::elected_relay::{
    ElectedDrainOutcome, ElectedPgRelay, SHARED_OUTBOX_PUBLISHER_LOCK_ID,
};
use myelin_storage::pgrelay::{PgRelay, RelayValidationConfig};
use myelin_tenancy::{Region, TenantId};

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn envelope(aggregate: &str, seq: u32) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("01JTEST{aggregate}{seq:019}")),
        type_: EventType("issue.issue.updated".into()),
        schema_ver: 1,
        tenant: TenantId("elected-relay-tenant".into()),
        region: Region("no-osl".into()),
        actor: Actor(Principal::stub(
            PrincipalId("elected-relay-test".into()),
            PrincipalKind::Service,
            TenantId("elected-relay-tenant".into()),
        )),
        subject: ArtifactRef(format!(
            "myelin://elected-relay-tenant/issue/issue/{aggregate}"
        )),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{aggregate}-{seq}")),
        caused_by: Some(CausedBy("integration-test".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-18T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-18T00:00:00Z".into()),
        payload: serde_json::json!({ "aggregate": aggregate, "seq": seq }),
    }
}

fn validation() -> RelayValidationConfig {
    RelayValidationConfig::new(Region("no-osl".into()), 256 * 1024).expect("valid relay scope")
}

fn isolated_lock_id() -> i64 {
    SHARED_OUTBOX_PUBLISHER_LOCK_ID ^ i64::from(std::process::id())
}

#[derive(Default)]
struct SlowRecordingPublisher {
    active: AtomicUsize,
    max_active: AtomicUsize,
    published: Mutex<Vec<(String, u32)>>,
}

impl EventPublisher for SlowRecordingPublisher {
    fn publish(
        &self,
        _subject: &ArtifactRef,
        envelope: &EventEnvelope,
        _dedup_id: &EventId,
    ) -> Result<Delivery, TransportError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(75));
        self.published
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((
                envelope.aggregate.0.clone(),
                envelope.payload["seq"].as_u64().expect("seq") as u32,
            ));
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(Delivery::Accepted)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn elected_relay_serializes_contenders_preserves_order_and_retains_outage_rows() {
    let cfg = MyelinConfig::dev();
    let schema = format!("elected_relay_{}", std::process::id());
    let pool = {
        let schema = schema.clone();
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
            .after_connect(move |connection, _| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::Executor::execute(
                        connection,
                        format!("SET search_path TO {schema}, public").as_str(),
                    )
                    .await?;
                    Ok(())
                })
            })
            .connect(&admin_url(&cfg))
            .await
            .expect("connect live PostgreSQL")
    };
    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {schema}"))
        .execute(&pool)
        .await
        .expect("create isolated schema");
    sqlx::raw_sql(OUTBOX_MIGRATION)
        .execute(&pool)
        .await
        .expect("migrate isolated outbox");
    sqlx::raw_sql(OUTBOX_QUARANTINE_MIGRATION)
        .execute(&pool)
        .await
        .expect("migrate isolated quarantine");
    sqlx::query("TRUNCATE outbox_quarantine, outbox")
        .execute(&pool)
        .await
        .expect("clean outbox");

    let raw = PgRelay::new(pool.clone());
    let lock_id = isolated_lock_id();
    for (aggregate, seq) in [
        ("issue:A", 0),
        ("issue:B", 0),
        ("issue:A", 1),
        ("issue:B", 1),
    ] {
        raw.enqueue(aggregate, seq, &envelope(aggregate, seq as u32))
            .await
            .expect("enqueue");
    }

    let first =
        ElectedPgRelay::with_lock_id(pool.clone(), validation(), lock_id).expect("first contender");
    let second = ElectedPgRelay::with_lock_id(pool.clone(), validation(), lock_id)
        .expect("second contender");
    let publisher = Arc::new(SlowRecordingPublisher::default());
    let start = Arc::new(tokio::sync::Barrier::new(3));

    let one = {
        let publisher = Arc::clone(&publisher);
        let start = Arc::clone(&start);
        tokio::spawn(async move {
            start.wait().await;
            first.drain_once(publisher.as_ref(), 32).await
        })
    };
    let two = {
        let publisher = Arc::clone(&publisher);
        let start = Arc::clone(&start);
        tokio::spawn(async move {
            start.wait().await;
            second.drain_once(publisher.as_ref(), 32).await
        })
    };
    start.wait().await;
    let outcomes = tokio::time::timeout(Duration::from_secs(5), async {
        let (one, two) = tokio::join!(one, two);
        [
            one.expect("first task").expect("first drain"),
            two.expect("second task").expect("second drain"),
        ]
    })
    .await
    .expect("bounded publishers make both contenders terminate");
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, ElectedDrainOutcome::Published(4)))
            .count(),
        1
    );
    assert!(outcomes.iter().any(|outcome| matches!(
        outcome,
        ElectedDrainOutcome::Standby | ElectedDrainOutcome::Published(0)
    )));
    assert_eq!(publisher.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(
        *publisher
            .published
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        vec![
            ("issue:A".into(), 0),
            ("issue:A".into(), 1),
            ("issue:B".into(), 0),
            ("issue:B".into(), 1),
        ]
    );

    let mut lock_holder = pool.acquire().await.expect("lock-holder connection");
    lock_holder.close_on_drop();
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(lock_id)
        .fetch_one(&mut *lock_holder)
        .await
        .expect("hold election lock");
    assert!(locked);
    let standby = ElectedPgRelay::with_lock_id(pool.clone(), validation(), lock_id)
        .expect("standby relay")
        .drain_once(publisher.as_ref(), 32)
        .await
        .expect("standby outcome");
    assert_eq!(standby, ElectedDrainOutcome::Standby);
    let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
        .bind(lock_id)
        .fetch_one(&mut *lock_holder)
        .await
        .expect("release election lock");
    assert!(unlocked);
    drop(lock_holder);

    sqlx::query("TRUNCATE outbox_quarantine, outbox")
        .execute(&pool)
        .await
        .expect("reset outbox");
    raw.enqueue("issue:outage", 0, &envelope("issue:outage", 0))
        .await
        .expect("enqueue outage row");
    let severed = InProcessBus::new();
    severed.sever();
    let elected =
        ElectedPgRelay::with_lock_id(pool.clone(), validation(), lock_id).expect("outage relay");
    let error = elected
        .drain_once(&severed, 32)
        .await
        .expect_err("broker outage must surface");
    assert!(error.to_string().contains("publish failed"));
    let retained: (i64, i32) = sqlx::query_as(
        "SELECT count(*), COALESCE(max(attempts), -1) FROM outbox WHERE published_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect retained row");
    assert_eq!(
        retained,
        (1, 0),
        "outage retains row without spending DLQ attempts"
    );

    let one_connection_pool = {
        let schema = schema.clone();
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(move |connection, _| {
                let schema = schema.clone();
                Box::pin(async move {
                    sqlx::Executor::execute(
                        connection,
                        format!("SET search_path TO {schema}, public").as_str(),
                    )
                    .await?;
                    Ok(())
                })
            })
            .connect(&admin_url(&cfg))
            .await
            .expect("connect one-session pool")
    };
    let recovered =
        ElectedPgRelay::with_lock_id(one_connection_pool.clone(), validation(), lock_id)
            .expect("one connection is sufficient")
            .drain_once(publisher.as_ref(), 32)
            .await
            .expect("one-connection elected pass");
    assert_eq!(recovered, ElectedDrainOutcome::Published(1));
    one_connection_pool.close().await;

    pool.close().await;
    let cleanup = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url(&cfg))
        .await
        .expect("cleanup connection");
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&cleanup)
        .await
        .expect("drop isolated schema");
}
