#![cfg(feature = "integration")]

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_events::relay::InProcessBus;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility, OUTBOX_MIGRATION, OUTBOX_QUARANTINE_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{foundation_migrations, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

mod common;

fn unique_schema() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "outbox_values_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn envelope(event_id: &str) -> EventEnvelope {
    let tenant = TenantId("acme".into());
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("issue.issue.updated".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("outbox-invariant-test".into()),
            PrincipalKind::Service,
            tenant,
        )),
        subject: ArtifactRef("myelin://acme/issue/issue/OUTBOX-1".into()),
        aggregate: AggregateKey("issue:OUTBOX-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(event_id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-08-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-08-20T00:00:01Z".into()),
        payload: serde_json::json!({ "use_case": "a user's event survives durable relay" }),
    }
}

async fn isolated_pool(schema: &str) -> (PgPool, PgPool) {
    let admin_url = common::admin_database_config().database_url;
    let bootstrap = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect to the required admin Postgres backend");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&bootstrap)
        .await
        .expect("create the isolated outbox schema");
    let options = PgConnectOptions::from_str(&admin_url)
        .expect("parse the integration database URL")
        .options([("search_path", format!("{schema},public").as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("connect to the isolated outbox schema");
    (bootstrap, pool)
}

async fn insert_raw(pool: &PgPool, envelope: &EventEnvelope, seq: i64, attempts: i32) {
    sqlx::query(
        "INSERT INTO outbox (event_id, aggregate, seq, subject, envelope, attempts) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&envelope.event_id.0)
    .bind(&envelope.aggregate.0)
    .bind(seq)
    .bind(&envelope.subject.0)
    .bind(serde_json::to_value(envelope).expect("serialize the event envelope"))
    .bind(attempts)
    .execute(pool)
    .await
    .expect("seed the pre-invariant outbox row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_outbox_refuses_corrupt_counters_and_split_identity() {
    let schema = unique_schema();
    let (bootstrap, pool) = isolated_pool(&schema).await;
    sqlx::raw_sql(OUTBOX_MIGRATION)
        .execute(&pool)
        .await
        .expect("install the frozen pre-invariant outbox foundation");
    sqlx::raw_sql(OUTBOX_QUARANTINE_MIGRATION)
        .execute(&pool)
        .await
        .expect("install the frozen pre-invariant quarantine foundation");

    let first = envelope("outbox-invariant-1");
    insert_raw(&pool, &first, -1, -1).await;
    let relay = PgRelay::new(pool.clone());
    assert!(relay
        .retained_rows()
        .await
        .expect_err("negative sequence must fail closed")
        .to_string()
        .contains("negative durable outbox seq"));

    sqlx::query("UPDATE outbox SET seq = 0 WHERE event_id = $1")
        .bind(&first.event_id.0)
        .execute(&pool)
        .await
        .expect("isolate the attempts decoder");
    assert!(relay
        .retained_rows()
        .await
        .expect_err("negative attempts must fail closed")
        .to_string()
        .contains("negative durable outbox attempts"));

    sqlx::query("UPDATE outbox SET attempts = 0, subject = 'myelin://acme/issue/issue/OTHER'")
        .execute(&pool)
        .await
        .expect("isolate the split identity decoder");
    assert!(relay
        .retained_rows()
        .await
        .expect_err("column and envelope identity must remain bound")
        .to_string()
        .contains("disagree with the serialized envelope identity"));

    sqlx::query("UPDATE outbox SET seq = -1, subject = $1")
        .bind(&first.subject.0)
        .execute(&pool)
        .await
        .expect("restore identity while retaining the legacy corrupt sequence");

    let mut legacy = envelope("outbox-legacy-aggregate");
    legacy.aggregate = AggregateKey("issue:OUTBOX-2".into());
    legacy.subject = ArtifactRef("myelin://acme/issue/issue/OUTBOX-2".into());
    insert_raw(&pool, &legacy, 0, 0).await;
    sqlx::query("UPDATE outbox SET aggregate = 'raw-object-id' WHERE event_id = $1")
        .bind(&legacy.event_id.0)
        .execute(&pool)
        .await
        .expect("reproduce the legacy split ordering identity");
    sqlx::query(
        "INSERT INTO outbox_quarantine \
         (event_id, aggregate, seq, reason_code, reason_detail) \
         VALUES ($1, 'raw-object-id', 0, 'aggregate_mismatch', 'legacy producer mismatch')",
    )
    .bind(&legacy.event_id.0)
    .execute(&pool)
    .await
    .expect("reproduce the quarantine metadata created by the old publisher");

    let mut legacy_chat = envelope("outbox-legacy-chat-aggregate");
    legacy_chat.type_ = EventType("chat.channel.created".into());
    legacy_chat.aggregate = AggregateKey("01JLEGACYCHAT0000000000000".into());
    legacy_chat.subject =
        ArtifactRef("myelin://acme/chat/channel/01JLEGACYCHAT0000000000000".into());
    insert_raw(&pool, &legacy_chat, 0, 0).await;
    sqlx::query(
        "INSERT INTO outbox_quarantine \
         (event_id, aggregate, seq, reason_code, reason_detail) \
         VALUES ($1, $2, 0, 'invalid_stream_subject', 'legacy aggregate omitted its type')",
    )
    .bind(&legacy_chat.event_id.0)
    .bind(&legacy_chat.aggregate.0)
    .execute(&pool)
    .await
    .expect("reproduce the legacy Chat stream-subject barrier");

    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .expect_err("validation refuses to bless a legacy corrupt row");
    assert!(
        PgMigrator::is_applied(&pool, "0007_outbox_value_invariants_expand")
            .await
            .expect("read expand migration state")
    );
    assert!(
        PgMigrator::is_applied(&pool, "0008_outbox_identity_backfill")
            .await
            .expect("read identity repair migration state")
    );
    assert!(
        !PgMigrator::is_applied(&pool, "0009_outbox_value_invariants_validate")
            .await
            .expect("read validation migration state")
    );
    sqlx::query("UPDATE outbox SET seq = 0 WHERE event_id = $1")
        .bind(&first.event_id.0)
        .execute(&pool)
        .await
        .expect("repair the legacy row explicitly");
    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .expect("the repaired foundation validates and becomes durable");
    let repaired_aggregates: (String, String, String) = sqlx::query_as(
        "SELECT source.aggregate, resolution.aggregate, resolution.resolution_code \
           FROM outbox source JOIN outbox_quarantine_resolution resolution USING (event_id) \
          WHERE source.event_id = $1",
    )
    .bind(&legacy.event_id.0)
    .fetch_one(&pool)
    .await
    .expect("read the forward-repaired event and quarantine identities");
    assert_eq!(
        repaired_aggregates,
        (
            legacy.aggregate.0.clone(),
            legacy.aggregate.0.clone(),
            "canonical_aggregate_backfill".into(),
        ),
        "the repaired quarantine remains auditable under its canonical event identity"
    );
    let released: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox_quarantine WHERE event_id = $1")
            .bind(&legacy.event_id.0)
            .fetch_one(&pool)
            .await
            .expect("verify the resolved barrier was released");
    assert_eq!(
        released, 0,
        "the repaired event can re-enter ordered delivery"
    );
    let repaired_chat: (String, String, String, i64) = sqlx::query_as(
        "SELECT source.aggregate,
                source.envelope ->> 'aggregate',
                resolution.resolution_code,
                (SELECT count(*) FROM outbox_quarantine WHERE event_id = source.event_id)
           FROM outbox source JOIN outbox_quarantine_resolution resolution USING (event_id)
          WHERE source.event_id = $1",
    )
    .bind(&legacy_chat.event_id.0)
    .fetch_one(&pool)
    .await
    .expect("read the canonicalized Chat event and its resolution");
    assert_eq!(
        repaired_chat,
        (
            "channel:01JLEGACYCHAT0000000000000".into(),
            "channel:01JLEGACYCHAT0000000000000".into(),
            "canonical_chat_aggregate_backfill".into(),
            0,
        ),
        "legacy Chat becomes publishable without losing its repair record"
    );

    for corrupting_write in [
        "UPDATE outbox SET seq = -1 WHERE event_id = 'outbox-invariant-1'",
        "UPDATE outbox SET attempts = -1 WHERE event_id = 'outbox-invariant-1'",
        "UPDATE outbox SET subject = 'myelin://acme/issue/issue/OTHER' \
         WHERE event_id = 'outbox-invariant-1'",
    ] {
        assert!(
            sqlx::query(corrupting_write).execute(&pool).await.is_err(),
            "Postgres rejects {corrupting_write}"
        );
    }
    assert!(
        relay.enqueue(&first.aggregate.0, -1, &first).await.is_err(),
        "the typed writer rejects a negative sequence before SQL"
    );
    assert!(
        relay.enqueue("raw-object-id", 2, &first).await.is_err(),
        "the typed writer cannot split ordering from event identity"
    );
    let retry_bus = InProcessBus::new();
    assert!(relay
        .drain_once_dead_letter(&retry_bus, 1, 0)
        .await
        .is_err());
    assert!(relay
        .drain_once_dead_letter(&retry_bus, 1, i32::MAX as u32 + 1)
        .await
        .is_err());

    let second = envelope("outbox-invariant-2");
    relay
        .enqueue(&second.aggregate.0, 1, &second)
        .await
        .expect("an ordinary event remains easy to enqueue");
    let retained = relay
        .retained_rows()
        .await
        .expect("validated durable rows remain readable");
    assert_eq!(
        retained
            .iter()
            .map(|row| (&row.event_id.0, row.seq, row.attempts))
            .collect::<Vec<_>>(),
        vec![
            (&legacy_chat.event_id.0, 0, 0),
            (&first.event_id.0, 0, 0),
            (&second.event_id.0, 1, 0),
            (&legacy.event_id.0, 0, 0),
        ]
    );

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&bootstrap)
        .await
        .expect("remove the isolated outbox schema");
    eprintln!("OK: durable outbox counters and event identity stay exact across upgrade and use");
}
