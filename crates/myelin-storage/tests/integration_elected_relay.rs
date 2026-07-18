//! Live-PostgreSQL proof for the singleton shared-outbox publisher.
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use myelin_config::MyelinConfig;
use myelin_events::relay::{Delivery, EventPublisher, TransportError};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, InProcessBus, Timestamp, Visibility, OUTBOX_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::elected_relay::{ElectedDrainOutcome, ElectedPgRelay};
use myelin_storage::pgrelay::PgRelay;
use myelin_tenancy::{Region, TenantId};

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn envelope(aggregate: &str, seq: u32) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("01JTEST{aggregate}{seq:019}")),
        type_: EventType("issues.issue.updated".into()),
        schema_ver: 1,
        tenant: TenantId("elected-relay-tenant".into()),
        region: Region("no-osl".into()),
        actor: Actor(Principal::stub(
            PrincipalId("elected-relay-test".into()),
            PrincipalKind::Service,
            TenantId("elected-relay-tenant".into()),
        )),
        subject: ArtifactRef(format!("myelin://elected-relay/issues/issue/{aggregate}")),
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

struct SlowRecordingPublisher {
    active: AtomicUsize,
    max_active: AtomicUsize,
    published: Mutex<Vec<(String, u32)>>,
    first_publish_released: Mutex<bool>,
    first_publish_gate: Condvar,
}

impl Default for SlowRecordingPublisher {
    fn default() -> Self {
        Self {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            published: Mutex::new(Vec::new()),
            first_publish_released: Mutex::new(false),
            first_publish_gate: Condvar::new(),
        }
    }
}

impl SlowRecordingPublisher {
    fn release_first_publish(&self) {
        *self
            .first_publish_released
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = true;
        self.first_publish_gate.notify_all();
    }
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
        if self
            .published
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            let mut released = self
                .first_publish_released
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            while !*released {
                released = self
                    .first_publish_gate
                    .wait(released)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }
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
    sqlx::query("TRUNCATE outbox")
        .execute(&pool)
        .await
        .expect("clean outbox");

    let raw = PgRelay::new(pool.clone());
    for (aggregate, seq) in [("A", 0), ("B", 0), ("A", 1), ("B", 1)] {
        raw.enqueue(aggregate, seq, &envelope(aggregate, seq as u32))
            .await
            .expect("enqueue");
    }

    let first = ElectedPgRelay::new(pool.clone()).expect("first contender");
    let second = ElectedPgRelay::new(pool.clone()).expect("second contender");
    let publisher = Arc::new(SlowRecordingPublisher::default());
    let start = Arc::new(tokio::sync::Barrier::new(3));

    let mut one = {
        let publisher = Arc::clone(&publisher);
        let start = Arc::clone(&start);
        tokio::spawn(async move {
            start.wait().await;
            first.drain_once(publisher.as_ref(), 32).await
        })
    };
    let mut two = {
        let publisher = Arc::clone(&publisher);
        let start = Arc::clone(&start);
        tokio::spawn(async move {
            start.wait().await;
            second.drain_once(publisher.as_ref(), 32).await
        })
    };
    start.wait().await;
    let (standby, one_finished) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            result = &mut one => (result.expect("first task").expect("first drain"), true),
            result = &mut two => (result.expect("second task").expect("second drain"), false),
        }
    })
    .await
    .expect("standby contender must return while leader is publishing");
    assert_eq!(standby, ElectedDrainOutcome::Standby);
    publisher.release_first_publish();
    let published = if one_finished {
        two.await.expect("leader task").expect("leader drain")
    } else {
        one.await.expect("leader task").expect("leader drain")
    };
    assert_eq!(published, ElectedDrainOutcome::Published(4));
    assert_eq!(publisher.max_active.load(Ordering::SeqCst), 1);
    assert_eq!(
        *publisher
            .published
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        vec![
            ("A".into(), 0),
            ("A".into(), 1),
            ("B".into(), 0),
            ("B".into(), 1),
        ]
    );

    sqlx::query("TRUNCATE outbox")
        .execute(&pool)
        .await
        .expect("reset outbox");
    raw.enqueue("outage", 0, &envelope("outage", 0))
        .await
        .expect("enqueue outage row");
    let severed = InProcessBus::new();
    severed.sever();
    let elected = ElectedPgRelay::new(pool.clone()).expect("outage relay");
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
