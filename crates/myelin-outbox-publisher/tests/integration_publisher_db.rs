#![cfg(feature = "integration")]

use std::sync::Mutex;

use myelin_events::relay::{Delivery, EventPublisher, TransportError};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_outbox_publisher::{
    DrainPass, PassResult, PublisherConfig, PublisherDbProvider, PublisherRuntime,
};
use myelin_storage::elected_relay::{
    ElectedDrainOutcome, ElectedRelayError, SHARED_OUTBOX_PUBLISHER_LOCK_ID,
};
use myelin_storage::pg::PgError;
use myelin_storage::{foundation_migrations, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

static DB_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn isolated_url(base: &str, schema: &str) -> String {
    let separator = if base.contains('?') { '&' } else { '?' };
    format!("{base}{separator}options=-csearch_path%3D{schema}%2Cpublic")
}

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
async fn isolated_foundation_migrations_reconcile_the_global_publisher_scope() {
    let _serial = DB_TEST.lock().await;
    let admin_url = std::env::var("DATABASE_MIGRATION_URL").expect("migration authority");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .expect("admin pool");
    let schema = format!("publisher_scope_{}", std::process::id());
    sqlx::raw_sql(&format!(
        "DROP SCHEMA IF EXISTS {schema} CASCADE;
         CREATE SCHEMA {schema} AUTHORIZATION myelin_admin;"
    ))
    .execute(&admin)
    .await
    .expect("fresh isolated schema");

    let isolated = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&isolated_url(&admin_url, &schema))
        .await
        .expect("schema-pinned migration pool");
    PgMigrator::apply(&isolated, &foundation_migrations())
        .await
        .expect("foundation migrations reconcile the publisher capability");

    for relation in ["outbox", "outbox_quarantine"] {
        let qualified = format!("{schema}.{relation}");
        for privilege in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
            let admitted: bool = sqlx::query_scalar(
                "SELECT pg_catalog.has_table_privilege(
                    'myelin_outbox_publisher_fr_par', $1, $2
                 )",
            )
            .bind(&qualified)
            .bind(privilege)
            .fetch_one(&admin)
            .await
            .expect("read effective isolated-schema privilege");
            assert!(
                !admitted,
                "the global publisher retained {privilege} on disposable {qualified}"
            );
        }
    }

    let config = PublisherConfig::from_env().expect("publisher config");
    let provider = PublisherDbProvider::connect(&config)
        .await
        .expect("the exact public relay capability remains valid");
    drop(provider);

    isolated.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("drop isolated schema");
    admin.close().await;
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
        let published: bool =
            sqlx::query_scalar("SELECT published_at IS NOT NULL FROM outbox WHERE event_id=$1")
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
        assert!(
            elected,
            "publisher contender B acquires the released election lock"
        );
        probe.rollback().await.unwrap();
    }

    sqlx::query("DELETE FROM outbox WHERE event_id=$1")
        .bind(&event_id)
        .execute(&admin)
        .await
        .unwrap();
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

    let mut foreign_rows = admin.begin().await.unwrap();
    sqlx::query(
        "SELECT event_id FROM outbox
         WHERE published_at IS NULL AND event_id NOT IN ($1, $2)
         FOR UPDATE",
    )
    .bind(&valid_id)
    .bind(&invalid_id)
    .fetch_all(&mut *foreign_rows)
    .await
    .expect("isolate this publisher pass without mutating unrelated pending rows");

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
    foreign_rows.rollback().await.unwrap();

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
