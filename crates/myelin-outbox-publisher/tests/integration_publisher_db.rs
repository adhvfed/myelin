#![cfg(feature = "integration")]

use std::sync::Mutex;

use myelin_events::relay::{Delivery, EventPublisher, TransportError};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_outbox_publisher::{PublisherConfig, PublisherDbProvider};
use myelin_storage::elected_relay::ElectedDrainOutcome;
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

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

#[tokio::test]
async fn dedicated_capability_publishes_and_quarantines_but_cannot_mutate_outbox_shape() {
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
