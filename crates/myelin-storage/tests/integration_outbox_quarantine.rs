//! Live PostgreSQL proof for strict elected-relay validation and durable quarantine.
#![cfg(feature = "integration")]

use std::sync::{Arc, Mutex};

use myelin_config::MyelinConfig;
use myelin_events::relay::{Delivery, EventPublisher, TransportError};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    InProcessBus, PiiKeyRef, Timestamp, Visibility, OUTBOX_MIGRATION, OUTBOX_QUARANTINE_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::elected_relay::{ElectedDrainOutcome, ElectedPgRelay};
use myelin_storage::pgrelay::{PgRelay, RelayValidationConfig};
use myelin_tenancy::{Region, TenantId};

fn envelope(id: &str, aggregate: &str, region: &str, payload: serde_json::Value) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issue.issue.updated".into()),
        schema_ver: 1,
        tenant: TenantId("relay-quarantine".into()),
        region: Region(region.into()),
        actor: Actor(Principal::stub(
            PrincipalId("relay-quarantine-writer".into()),
            PrincipalKind::Service,
            TenantId("relay-quarantine".into()),
        )),
        subject: ArtifactRef(format!("myelin://relay-quarantine/issue/issue/{aggregate}")),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-18T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-18T00:00:00Z".into()),
        payload,
    }
}

#[derive(Default)]
struct RecordingPublisher {
    ids: Mutex<Vec<String>>,
}

impl EventPublisher for RecordingPublisher {
    fn publish(
        &self,
        _subject: &ArtifactRef,
        envelope: &EventEnvelope,
        _dedup_id: &EventId,
    ) -> Result<Delivery, TransportError> {
        self.ids
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(envelope.event_id.0.clone());
        Ok(Delivery::Accepted)
    }
}

async fn insert_raw(
    pool: &sqlx::PgPool,
    event_id: &str,
    aggregate: &str,
    seq: i64,
    subject: &str,
    envelope: serde_json::Value,
) {
    sqlx::query(
        "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(event_id)
    .bind(aggregate)
    .bind(seq)
    .bind(subject)
    .bind(envelope)
    .execute(pool)
    .await
    .expect("insert raw outbox row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strict_relay_quarantines_permanent_rows_and_preserves_transient_rows() {
    let cfg = MyelinConfig::dev();
    let schema = format!("outbox_quarantine_{}", std::process::id());
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
            .connect(&cfg.database_migration_url)
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
        .expect("migrate outbox");
    sqlx::raw_sql(OUTBOX_QUARANTINE_MIGRATION)
        .execute(&pool)
        .await
        .expect("migrate quarantine");

    let good = envelope(
        "good",
        "issue:good",
        "no-osl",
        serde_json::json!({"ok": true}),
    );
    let raw = PgRelay::new(pool.clone());
    raw.enqueue("issue:good", 0, &good)
        .await
        .expect("enqueue good row");

    let mut good_pii = envelope(
        "good-pii",
        "issue:good-pii",
        "no-osl",
        serde_json::json!({"ciphertext_ref": "blob:opaque"}),
    );
    good_pii.contains_personal_data = true;
    // Epoch zero is the KMS authority's valid initial generation, not a malformed epoch.
    good_pii.pii_key_ref = Some(PiiKeyRef("kms://relay-quarantine/0/subject:u42".into()));
    raw.enqueue("issue:good-pii", 0, &good_pii)
        .await
        .expect("enqueue valid PII row");

    insert_raw(
        &pool,
        "bad-json",
        "issue:bad-json",
        0,
        "opaque",
        serde_json::json!({"not": "an envelope"}),
    )
    .await;

    let mismatched = envelope(
        "inside-id",
        "issue:mismatch",
        "no-osl",
        serde_json::json!({}),
    );
    insert_raw(
        &pool,
        "outside-id",
        "issue:mismatch",
        0,
        &mismatched.subject.0,
        serde_json::to_value(&mismatched).expect("serialize mismatch"),
    )
    .await;

    let aggregate_mismatch = envelope(
        "aggregate-mismatch",
        "issue:inside",
        "no-osl",
        serde_json::json!({}),
    );
    insert_raw(
        &pool,
        "aggregate-mismatch",
        "issue:outside",
        0,
        &aggregate_mismatch.subject.0,
        serde_json::to_value(&aggregate_mismatch).expect("serialize aggregate mismatch"),
    )
    .await;

    let mut actor_mismatch = envelope(
        "actor-mismatch",
        "issue:actor-mismatch",
        "no-osl",
        serde_json::json!({}),
    );
    actor_mismatch.actor = Actor(Principal::stub(
        PrincipalId("foreign-writer".into()),
        PrincipalKind::Service,
        TenantId("foreign-tenant".into()),
    ));
    raw.enqueue("issue:actor-mismatch", 0, &actor_mismatch)
        .await
        .expect("enqueue actor mismatch");

    let mut invalid_taxonomy = envelope(
        "invalid-taxonomy",
        "issue:invalid-taxonomy",
        "no-osl",
        serde_json::json!({}),
    );
    invalid_taxonomy.type_ = EventType("flow.signal.delivered".into());
    raw.enqueue("issue:invalid-taxonomy", 0, &invalid_taxonomy)
        .await
        .expect("enqueue invalid taxonomy");

    let mut false_with_key = envelope(
        "false-with-key",
        "issue:false-with-key",
        "no-osl",
        serde_json::json!({}),
    );
    false_with_key.pii_key_ref = Some(PiiKeyRef("kms://relay-quarantine/0/tenant".into()));
    raw.enqueue("issue:false-with-key", 0, &false_with_key)
        .await
        .expect("enqueue false/key mismatch");

    let mut true_without_key = envelope(
        "true-without-key",
        "issue:true-without-key",
        "no-osl",
        serde_json::json!({}),
    );
    true_without_key.contains_personal_data = true;
    raw.enqueue("issue:true-without-key", 0, &true_without_key)
        .await
        .expect("enqueue true/no-key mismatch");

    let mut malformed_key = envelope(
        "malformed-key",
        "issue:malformed-key",
        "no-osl",
        serde_json::json!({}),
    );
    malformed_key.contains_personal_data = true;
    malformed_key.pii_key_ref = Some(PiiKeyRef(
        "kms://relay-quarantine/0/subject:u42/extra".into(),
    ));
    raw.enqueue("issue:malformed-key", 0, &malformed_key)
        .await
        .expect("enqueue malformed key ref");

    let mut cross_tenant_key = envelope(
        "cross-tenant-key",
        "issue:cross-tenant-key",
        "no-osl",
        serde_json::json!({}),
    );
    cross_tenant_key.contains_personal_data = true;
    cross_tenant_key.pii_key_ref = Some(PiiKeyRef("kms://foreign/0/blob".into()));
    raw.enqueue("issue:cross-tenant-key", 0, &cross_tenant_key)
        .await
        .expect("enqueue cross-tenant key ref");

    let mut malformed_subject = envelope(
        "malformed-subject",
        "issue:malformed-subject",
        "no-osl",
        serde_json::json!({}),
    );
    malformed_subject.subject = ArtifactRef("https://relay-quarantine/issue/issue/one".into());
    raw.enqueue("issue:malformed-subject", 0, &malformed_subject)
        .await
        .expect("enqueue malformed subject");

    let mut cross_tenant_subject = envelope(
        "cross-tenant-subject",
        "issue:cross-tenant-subject",
        "no-osl",
        serde_json::json!({}),
    );
    cross_tenant_subject.subject = ArtifactRef("myelin://foreign/issue/issue/one".into());
    raw.enqueue("issue:cross-tenant-subject", 0, &cross_tenant_subject)
        .await
        .expect("enqueue cross-tenant subject");

    let mut zero_schema = envelope(
        "zero-schema",
        "issue:zero-schema",
        "no-osl",
        serde_json::json!({}),
    );
    zero_schema.schema_ver = 0;
    raw.enqueue("issue:zero-schema", 0, &zero_schema)
        .await
        .expect("enqueue zero schema version");

    let wrong_region = envelope(
        "wrong-region",
        "issue:wrong-region",
        "fr-par",
        serde_json::json!({}),
    );
    raw.enqueue("issue:wrong-region", 0, &wrong_region)
        .await
        .expect("enqueue wrong-region row");

    let wildcard = envelope("wildcard", "issue:*", "no-osl", serde_json::json!({}));
    raw.enqueue("issue:*", 0, &wildcard)
        .await
        .expect("enqueue wildcard row");

    let oversized = envelope(
        "oversized",
        "issue:oversized",
        "no-osl",
        serde_json::json!({"padding": "x".repeat(4096)}),
    );
    raw.enqueue("issue:oversized", 0, &oversized)
        .await
        .expect("enqueue oversized row");

    let blocked_head = envelope(
        "blocked-head",
        "issue:blocked",
        "no-osl",
        serde_json::json!({}),
    );
    insert_raw(
        &pool,
        "blocked-head",
        "issue:blocked",
        0,
        "wrong-row-subject",
        serde_json::to_value(&blocked_head).expect("serialize blocked head"),
    )
    .await;
    let blocked_tail = envelope(
        "blocked-tail",
        "issue:blocked",
        "no-osl",
        serde_json::json!({}),
    );
    raw.enqueue("issue:blocked", 1, &blocked_tail)
        .await
        .expect("enqueue blocked tail");

    let validation =
        RelayValidationConfig::new(Region("no-osl".into()), 1024).expect("valid strict scope");
    let publisher = Arc::new(RecordingPublisher::default());
    let elected = ElectedPgRelay::new(pool.clone(), validation.clone()).expect("strict relay");
    assert_eq!(
        elected
            .drain_once(publisher.as_ref(), 64)
            .await
            .expect("strict pass"),
        ElectedDrainOutcome::Published(2)
    );
    assert_eq!(
        *publisher
            .ids
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        vec!["good".to_string(), "good-pii".to_string()]
    );

    let quarantined: Vec<(String, String)> =
        sqlx::query_as("SELECT event_id, reason_code FROM outbox_quarantine ORDER BY event_id")
            .fetch_all(&pool)
            .await
            .expect("read quarantine");
    assert_eq!(
        quarantined,
        vec![
            ("actor-mismatch".into(), "actor_tenant_mismatch".into()),
            ("aggregate-mismatch".into(), "aggregate_mismatch".into()),
            ("bad-json".into(), "invalid_envelope_json".into()),
            ("blocked-head".into(), "subject_mismatch".into()),
            ("cross-tenant-key".into(), "pii_key_tenant_mismatch".into()),
            (
                "cross-tenant-subject".into(),
                "subject_tenant_mismatch".into()
            ),
            ("false-with-key".into(), "pii_presence_mismatch".into()),
            ("invalid-taxonomy".into(), "invalid_event_taxonomy".into()),
            ("malformed-key".into(), "invalid_pii_key_ref".into()),
            ("malformed-subject".into(), "invalid_artifact_ref".into()),
            ("outside-id".into(), "event_id_mismatch".into()),
            ("oversized".into(), "envelope_too_large".into()),
            ("true-without-key".into(), "pii_presence_mismatch".into()),
            ("wildcard".into(), "invalid_stream_subject".into()),
            ("wrong-region".into(), "wrong_relay_region".into()),
            ("zero-schema".into(), "invalid_schema_version".into()),
        ]
    );
    let blocked: (bool, i32) = sqlx::query_as(
        "SELECT published_at IS NULL, attempts FROM outbox WHERE event_id = 'blocked-tail'",
    )
    .fetch_one(&pool)
    .await
    .expect("read blocked tail");
    assert_eq!(blocked, (true, 0));

    // A fresh relay process observes the persistent quarantine and neither republishes nor skips
    // past the quarantined aggregate head.
    let restarted = ElectedPgRelay::new(pool.clone(), validation).expect("restarted strict relay");
    assert_eq!(
        restarted
            .drain_once(publisher.as_ref(), 64)
            .await
            .expect("restart pass"),
        ElectedDrainOutcome::Published(0)
    );
    let quarantine_count: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox_quarantine")
        .fetch_one(&pool)
        .await
        .expect("persistent quarantine count");
    assert_eq!(quarantine_count, 16);

    // A permanent defect discovered earlier in a pass is not half-committed if a later broker
    // operation fails: both the quarantine insert and the published marks share one transaction.
    insert_raw(
        &pool,
        "rollback-invalid",
        "issue:atomic-invalid",
        0,
        "opaque",
        serde_json::json!({"not": "an envelope"}),
    )
    .await;
    let outage = envelope("outage", "issue:outage", "no-osl", serde_json::json!({}));
    raw.enqueue("issue:outage", 0, &outage)
        .await
        .expect("enqueue outage row");
    let severed = InProcessBus::new();
    severed.sever();
    assert!(restarted.drain_once(&severed, 64).await.is_err());
    let retained: (bool, i32, i64) = sqlx::query_as(
        "SELECT published_at IS NULL, attempts, \
         (SELECT count(*) FROM outbox_quarantine WHERE event_id = 'outage') \
         FROM outbox WHERE event_id = 'outage'",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect outage row");
    assert_eq!(retained, (true, 0, 0));
    let rolled_back: (bool, i32, i64) = sqlx::query_as(
        "SELECT published_at IS NULL, attempts, \
         (SELECT count(*) FROM outbox_quarantine WHERE event_id = 'rollback-invalid') \
         FROM outbox WHERE event_id = 'rollback-invalid'",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect rolled-back quarantine row");
    assert_eq!(rolled_back, (true, 0, 0));

    pool.close().await;
    let cleanup = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&cfg.database_migration_url)
        .await
        .expect("cleanup connection");
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&cleanup)
        .await
        .expect("drop isolated schema");
}
