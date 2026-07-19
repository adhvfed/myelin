#![cfg(feature = "integration")]

use std::sync::Mutex;

use myelin_events::nats::{
    JetStreamConsumerConfig, JetStreamProvisioner, NatsJetStreamBus, NatsJetStreamPublisher,
};
use myelin_events::relay::{Delivery, EventConsumer, EventPublisher, TransportError};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_outbox_publisher::{
    DrainPass, GitRefV2CutoverError, GitRefV2OperatorFence, PassResult, PublisherConfig,
    PublisherDbProvider, PublisherRuntime, EVENT_STREAM_NAME, EVENT_SUBJECT_ROOT,
};
use myelin_storage::elected_relay::{
    ElectedDrainOutcome, ElectedRelayError, SHARED_OUTBOX_PUBLISHER_LOCK_ID,
};
use myelin_storage::pg::PgError;
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

static DB_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn envelope(event_id: &str, aggregate: &str) -> EventEnvelope {
    let tenant = TenantId("publisher-live".into());
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("issue.issue.updated".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("publisher-live".into()),
            PrincipalKind::Service,
            tenant,
        )),
        subject: ArtifactRef(format!("myelin://publisher-live/issue/issue/{aggregate}")),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{event_id}")),
        caused_by: Some(CausedBy("publisher-live".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-19T00:00:00Z".into()),
        payload: serde_json::json!({"proof": true}),
    }
}

#[derive(Default)]
struct RecordingPublisher(Mutex<Vec<String>>);

impl EventPublisher for RecordingPublisher {
    fn publish(
        &self,
        _subject: &ArtifactRef,
        envelope: &EventEnvelope,
        _dedup_id: &EventId,
    ) -> Result<Delivery, TransportError> {
        self.0.lock().unwrap().push(envelope.event_id.0.clone());
        Ok(Delivery::Accepted)
    }
}

struct UnresponsiveDbPass {
    pool: sqlx::PgPool,
    event_id: String,
}

impl DrainPass for UnresponsiveDbPass {
    async fn drain_once(&self) -> Result<ElectedDrainOutcome, ElectedRelayError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| ElectedRelayError::Election(PgError::Query(error.to_string())))?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(SHARED_OUTBOX_PUBLISHER_LOCK_ID)
            .execute(&mut *tx)
            .await
            .map_err(|error| ElectedRelayError::Election(PgError::Query(error.to_string())))?;
        sqlx::query("SELECT event_id FROM outbox WHERE event_id=$1 FOR UPDATE")
            .bind(&self.event_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| ElectedRelayError::Relay(PgError::Query(error.to_string())))?;
        std::future::pending::<()>().await;
        drop(tx);
        unreachable!("an unresponsive pass never completes without its whole-pass timeout")
    }
}

#[tokio::test]
async fn repeated_unresponsive_passes_time_out_with_rows_unsent_and_locks_released() {
    let _serial = DB_TEST.lock().await;
    let admin_url = std::env::var("DATABASE_MIGRATION_URL").expect("migration authority");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .expect("admin pool");
    sqlx::raw_sql(myelin_events::OUTBOX_MIGRATION)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::raw_sql(myelin_events::OUTBOX_PUBLISHER_GRANTS_MIGRATION)
        .execute(&admin)
        .await
        .unwrap();

    let event_id = format!("publisher-timeout-{}", std::process::id());
    sqlx::query("DELETE FROM outbox WHERE event_id=$1")
        .bind(&event_id)
        .execute(&admin)
        .await
        .unwrap();
    let event = envelope(&event_id, &format!("issue:timeout:{}", std::process::id()));
    sqlx::query(
        "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) VALUES ($1, $2, 0, $3, $4)",
    )
    .bind(&event_id)
    .bind(&event.aggregate.0)
    .bind(&event.subject.0)
    .bind(serde_json::to_value(&event).unwrap())
    .execute(&admin)
    .await
    .unwrap();

    let publisher_url = std::env::var("MYELIN_OUTBOX_PUBLISHER_DATABASE_URL").unwrap();
    let publisher_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&publisher_url)
        .await
        .unwrap();
    let contender_pool = publisher_pool.clone();
    let runtime = PublisherRuntime::new(
        UnresponsiveDbPass {
            pool: publisher_pool,
            event_id: event_id.clone(),
        },
        std::time::Duration::from_secs(60),
        std::time::Duration::from_secs(60),
        std::time::Duration::from_millis(25),
    );

    for _ in 0..3 {
        assert_eq!(runtime.run_pass().await, PassResult::Unavailable);
        let published: bool = sqlx::query_scalar(
            "SELECT published_at IS NOT NULL FROM outbox WHERE event_id=$1",
        )
        .bind(&event_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(!published, "a timed-out pass rolls its row state back");

        let mut probe = contender_pool.begin().await.unwrap();
        let elected: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
            .bind(SHARED_OUTBOX_PUBLISHER_LOCK_ID)
            .fetch_one(&mut *probe)
            .await
            .unwrap();
        assert!(elected, "publisher contender B acquires the released election lock");
        probe.rollback().await.unwrap();
    }

    sqlx::query("DELETE FROM outbox WHERE event_id=$1")
        .bind(&event_id)
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn git_ref_v2_cutover_barrier_reads_live_pg_and_nats_without_mutating_them() {
    let _serial = DB_TEST.lock().await;
    let admin_url = std::env::var("DATABASE_MIGRATION_URL").expect("migration authority");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .expect("admin pool");
    for migration in [
        myelin_events::OUTBOX_MIGRATION,
        myelin_events::OUTBOX_QUARANTINE_MIGRATION,
        myelin_events::OUTBOX_PUBLISHER_GRANTS_MIGRATION,
    ] {
        sqlx::raw_sql(migration).execute(&admin).await.unwrap();
    }
    let suffix = std::process::id();
    let legacy_id = format!("cutover-legacy-{suffix}");
    let v2_id = format!("cutover-v2-{suffix}");
    let broker_id = format!("cutover-broker-{suffix}");
    sqlx::query("DELETE FROM outbox_quarantine WHERE event_id=$1")
        .bind(&legacy_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE event_id=$1")
        .bind(&legacy_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE event_id=$1")
        .bind(&v2_id)
        .execute(&admin)
        .await
        .unwrap();

    let config = PublisherConfig::from_env().expect("publisher config");
    let rt = tokio::runtime::Handle::current();
    JetStreamProvisioner::ensure(config.provision_nats_config(), rt.clone()).unwrap();
    let publish_config = config.publish_nats_config();
    let consumer_name = format!("git-ref-cutover-{suffix}");
    let tenant = format!("cutover{suffix}");
    let filter = format!("{EVENT_SUBJECT_ROOT}.evt.{tenant}.>");
    let consumer = NatsJetStreamBus::connect_consumer(
        JetStreamConsumerConfig::bounded(
            publish_config.nats_url.clone(),
            EVENT_STREAM_NAME,
            EVENT_SUBJECT_ROOT,
            filter,
            consumer_name.clone(),
        ),
        rt.clone(),
    )
    .unwrap();
    let provider = PublisherDbProvider::connect(&config).await.unwrap();
    let fence = GitRefV2OperatorFence {
        consumer_upcaster_active: true,
        writer_quiesced: true,
    };
    assert_eq!(
        provider
            .preflight_git_ref_v2(
                &config,
                &consumer_name,
                GitRefV2OperatorFence {
                    consumer_upcaster_active: false,
                    writer_quiesced: true,
                },
                rt.clone(),
            )
            .await,
        Err(GitRefV2CutoverError::ConsumerUpcasterNotAcknowledged)
    );
    assert_eq!(
        provider
            .preflight_git_ref_v2(
                &config,
                &consumer_name,
                GitRefV2OperatorFence {
                    consumer_upcaster_active: true,
                    writer_quiesced: false,
                },
                rt.clone(),
            )
            .await,
        Err(GitRefV2CutoverError::WriterNotQuiesced)
    );

    let mut legacy = envelope(&legacy_id, &format!("core{suffix}:refs/heads/main"));
    legacy.type_ = EventType("git.ref.updated".into());
    legacy.schema_ver = 1;
    legacy.payload = serde_json::json!({
        "repo": format!("core{suffix}"),
        "ref": "refs/heads/main",
        "update_seq": 1,
    });
    sqlx::query(
        "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) VALUES ($1, $2, 0, $3, $4)",
    )
    .bind(&legacy_id)
    .bind(&legacy.aggregate.0)
    .bind(&legacy.subject.0)
    .bind(serde_json::to_value(&legacy).unwrap())
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        provider
            .preflight_git_ref_v2(&config, &consumer_name, fence, rt.clone())
            .await,
        Err(GitRefV2CutoverError::LegacyOutboxPending)
    );

    sqlx::query("UPDATE outbox SET published_at=now() WHERE event_id=$1")
        .bind(&legacy_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO outbox_quarantine (event_id, aggregate, seq, reason_code, reason_detail)
         VALUES ($1, $2, 0, 'invalid_artifact_ref', 'legacy Git ref identity')",
    )
    .bind(&legacy_id)
    .bind(&legacy.aggregate.0)
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        provider
            .preflight_git_ref_v2(&config, &consumer_name, fence, rt.clone())
            .await,
        Err(GitRefV2CutoverError::LegacyQuarantinePending)
    );
    sqlx::query("UPDATE outbox_quarantine SET acknowledged_at=now() WHERE event_id=$1")
        .bind(&legacy_id)
        .execute(&admin)
        .await
        .unwrap();

    let repo = format!("core{suffix}");
    let mut v2 = legacy.clone();
    v2.event_id = EventId(v2_id.clone());
    v2.schema_ver = 2;
    v2.aggregate = AggregateKey(format!("ref:{repo}:refs%2Fheads%2Fmain"));
    v2.subject = ArtifactRef(format!(
        "myelin://publisher-live/git/ref/{repo}:refs%2Fheads%2Fmain"
    ));
    v2.payload["update_seq"] = serde_json::json!(1);
    sqlx::query(
        "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope, published_at)
         VALUES ($1, $2, 0, $3, $4, now())",
    )
    .bind(&v2_id)
    .bind(&v2.aggregate.0)
    .bind(&v2.subject.0)
    .bind(serde_json::to_value(&v2).unwrap())
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        provider
            .preflight_git_ref_v2(&config, &consumer_name, fence, rt.clone())
            .await,
        Err(GitRefV2CutoverError::UpdateSequenceOverlap)
    );
    v2.payload["update_seq"] = serde_json::json!(2);
    sqlx::query("UPDATE outbox SET envelope=$2 WHERE event_id=$1")
        .bind(&v2_id)
        .bind(serde_json::to_value(&v2).unwrap())
        .execute(&admin)
        .await
        .unwrap();

    let mut broker_event = envelope(&broker_id, &format!("issue:cutover:{suffix}"));
    broker_event.tenant = TenantId(tenant.clone());
    broker_event.actor = Actor(Principal::stub(
        PrincipalId("cutover".into()),
        PrincipalKind::Service,
        TenantId(tenant),
    ));
    broker_event.subject = ArtifactRef(format!(
        "myelin://{}/issue/issue/cutover-{suffix}",
        broker_event.tenant.as_str()
    ));
    let publisher = NatsJetStreamPublisher::connect_existing(publish_config, rt.clone()).unwrap();
    publisher
        .publish(&broker_event.subject, &broker_event, &broker_event.event_id)
        .unwrap();
    assert_eq!(
        provider
            .preflight_git_ref_v2(&config, &consumer_name, fence, rt.clone())
            .await,
        Err(GitRefV2CutoverError::DurableMessagesPending)
    );
    let delivery = consumer.consume(EVENT_SUBJECT_ROOT).unwrap().remove(0);
    assert_eq!(
        provider
            .preflight_git_ref_v2(&config, &consumer_name, fence, rt.clone())
            .await,
        Err(GitRefV2CutoverError::DurableAcksPending)
    );
    consumer.ack(delivery.token).unwrap();
    provider
        .preflight_git_ref_v2(&config, &consumer_name, fence, rt)
        .await
        .expect("all five barriers are zero/acknowledged");

    sqlx::query("DELETE FROM outbox_quarantine WHERE event_id=$1")
        .bind(&legacy_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE event_id=$1")
        .bind(&legacy_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE event_id=$1")
        .bind(&v2_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

#[tokio::test]
async fn dedicated_capability_publishes_and_quarantines_but_cannot_mutate_outbox_shape() {
    let _serial = DB_TEST.lock().await;
    let admin_url = std::env::var("DATABASE_MIGRATION_URL").expect("migration authority");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .expect("admin pool");
    sqlx::raw_sql(myelin_events::OUTBOX_MIGRATION)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::raw_sql(myelin_events::OUTBOX_QUARANTINE_MIGRATION)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::raw_sql(myelin_events::OUTBOX_PUBLISHER_GRANTS_MIGRATION)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE IF NOT EXISTS publisher_forbidden_probe (id int primary key)")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox_quarantine WHERE event_id LIKE 'publisher-live-%' OR event_id LIKE 'publisher-invalid-%'")
        .execute(&admin).await.unwrap();
    sqlx::query("DELETE FROM outbox WHERE event_id LIKE 'publisher-live-%' OR event_id LIKE 'publisher-invalid-%'")
        .execute(&admin).await.unwrap();
    let suffix = std::process::id();
    let valid_id = format!("publisher-live-{suffix}");
    let invalid_id = format!("publisher-invalid-{suffix}");
    sqlx::query("DELETE FROM outbox_quarantine WHERE event_id IN ($1, $2)")
        .bind(&valid_id)
        .bind(&invalid_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE event_id IN ($1, $2)")
        .bind(&valid_id)
        .bind(&invalid_id)
        .execute(&admin)
        .await
        .unwrap();
    let valid = envelope(&valid_id, &format!("issue:publisher:{suffix}"));
    sqlx::query(
        "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) VALUES ($1, $2, 0, $3, $4)",
    )
    .bind(&valid_id).bind(&valid.aggregate.0).bind(&valid.subject.0)
    .bind(serde_json::to_value(&valid).unwrap()).execute(&admin).await.unwrap();
    sqlx::query(
        "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) VALUES ($1, $2, 0, $3, '{}'::jsonb)",
    )
    .bind(&invalid_id).bind(format!("invalid:publisher:{suffix}"))
    .bind("myelin://publisher-live/issue/issue/invalid").execute(&admin).await.unwrap();

    let config = PublisherConfig::from_env().expect("publisher config");
    let provider = PublisherDbProvider::connect(&config)
        .await
        .expect("least-privilege provider");
    let relay = provider
        .elected_relay(config.region(), config.max_envelope_bytes())
        .unwrap();
    let publisher = RecordingPublisher::default();
    assert_eq!(
        relay.drain_once(&publisher, config.batch()).await.unwrap(),
        ElectedDrainOutcome::Published(1)
    );
    assert_eq!(*publisher.0.lock().unwrap(), vec![valid_id.clone()]);
    let row = sqlx::query(
        "SELECT published_at IS NOT NULL AS published,
                EXISTS (SELECT 1 FROM outbox_quarantine WHERE event_id = $2) AS quarantined
           FROM outbox WHERE event_id = $1",
    )
    .bind(&valid_id)
    .bind(&invalid_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(row.get::<bool, _>("published"));
    assert!(row.get::<bool, _>("quarantined"));

    let publisher_url = std::env::var("MYELIN_OUTBOX_PUBLISHER_DATABASE_URL").unwrap();
    let direct = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&publisher_url)
        .await
        .unwrap();
    assert!(sqlx::query("INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) VALUES ('forbidden', 'forbidden', 0, 'forbidden', '{}'::jsonb)")
        .execute(&direct).await.is_err());
    assert!(
        sqlx::query("UPDATE outbox SET attempts = attempts + 1 WHERE event_id = $1")
            .bind(&valid_id)
            .execute(&direct)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM outbox WHERE event_id = $1")
        .bind(&valid_id)
        .execute(&direct)
        .await
        .is_err());
    assert!(sqlx::query("SELECT * FROM publisher_forbidden_probe")
        .fetch_all(&direct)
        .await
        .is_err());
    direct.close().await;

    sqlx::query("DELETE FROM outbox_quarantine WHERE event_id IN ($1, $2)")
        .bind(&valid_id)
        .bind(&invalid_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox WHERE event_id IN ($1, $2)")
        .bind(&valid_id)
        .bind(&invalid_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DROP TABLE publisher_forbidden_probe")
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
