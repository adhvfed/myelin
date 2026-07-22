//! Live-PostgreSQL proof for the durable notification inbox repository.
#![cfg(feature = "integration")]

use std::collections::HashSet;

use myelin_config::MyelinConfig;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, DedupLedger, Delivered,
    EventEnvelope, EventId, EventType, Message, OutboxStore, Timestamp, UlidMinter, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::list_inbox::InboxFilter;
use myelin_notif::pg_inbox::{
    InboxReadRequest, InboxReadScope, InboxUpsert, InboxUpsertOutcome, PgInboxError, PgInboxStore,
};
use myelin_notif::router::RoutedInboxItem;
use myelin_notif::{Class, Reason};
use myelin_query::signals::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_storage::migration::HotTables;
use myelin_storage::{PgMigrator, PgOutboxBacking};
use myelin_tenancy::{Region, TenantId};

fn upsert(
    tenant: &TenantId,
    region: &Region,
    item_id: &str,
    recipient: &str,
    reason: Reason,
    class: Class,
    dedup_key: &str,
) -> InboxUpsert {
    let subject = ArtifactRef(format!("myelin://{}/git/pr/{item_id}", tenant.0));
    InboxUpsert {
        item: RoutedInboxItem {
            tenant: tenant.clone(),
            region: region.clone(),
            item_id: item_id.into(),
            recipient: recipient.into(),
            subject: subject.clone(),
            reason,
            class,
            origin_event: ArtifactRef(format!("myelin://{}/bus/event/event-{item_id}", tenant.0)),
            dedup_key: dedup_key.into(),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
        },
        subject_root: subject.clone(),
        template_key: "git.pr.activity".into(),
        template_args: vec![subject],
        occurred_at: "2026-07-22T12:00:00Z".into(),
        dek_ref: format!("kms://{}/notif/inbox", tenant.0),
    }
}

async fn delete_probe_rows(admin: &sqlx::PgPool, tenant: &TenantId, region: &Region) {
    let mut conn = admin.acquire().await.expect("acquire cleanup connection");
    sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
        .bind(&tenant.0)
        .execute(&mut *conn)
        .await
        .expect("scope cleanup tenant");
    sqlx::query("SELECT set_config('myelin.region', $1, false)")
        .bind(&region.0)
        .execute(&mut *conn)
        .await
        .expect("scope cleanup region");
    sqlx::query("DELETE FROM notif_inbox_item WHERE tenant_id = $1 AND region = $2")
        .bind(&tenant.0)
        .bind(&region.0)
        .execute(&mut *conn)
        .await
        .expect("delete durable-inbox probe rows");
}

fn signal_envelope(tenant: &TenantId, region: &Region, event_id: &str) -> EventEnvelope {
    let signal = Signal {
        rule_id: RuleId("durable_router_probe".into()),
        tenant: tenant.clone(),
        severity: Severity::Warning,
        dedup_key: DedupKey("one-durable-effect".into()),
        subject: ArtifactRef(format!("myelin://{}/git/pr/router-probe", tenant.0)),
        count: 1,
        state: SignalState::Open,
        first_seen: "2026-07-22T12:00:00Z".into(),
        last_seen: "2026-07-22T12:00:00Z".into(),
    };
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: region.clone(),
        actor: Actor(Principal::stub(
            PrincipalId("psn:actor".into()),
            PrincipalKind::Human,
            tenant.clone(),
        )),
        subject: ArtifactRef(format!("sig.{}.warning.durable_router_probe", tenant.0)),
        aggregate: AggregateKey("signal:one-durable-effect".into()),
        causation_id: None,
        correlation_id: CorrelationId(event_id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-22T12:00:00Z".into()),
        recorded_at: Timestamp("2026-07-22T12:00:01Z".into()),
        payload: serde_json::to_value(signal).expect("serialize signal"),
    }
}

#[tokio::test]
async fn durable_inbox_collapses_pages_and_survives_a_new_store_instance() {
    let config = MyelinConfig::dev();
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&config.database_url)
        .await
        .expect("connect to dev Postgres as the RLS-bound app role (is the stack up?)");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&config.database_migration_url)
        .await
        .expect("connect to dev Postgres as the migration role");

    PgMigrator::apply_validated(
        &admin,
        &myelin_notif::migrations::migrations(),
        &HotTables::none(),
    )
    .await
    .expect("apply the notification schema and online keyset index");
    sqlx::query("GRANT SELECT, INSERT, UPDATE, DELETE ON notif_inbox_item TO myelin_app")
        .execute(&admin)
        .await
        .expect("grant the runtime role access to the migrated inbox table");

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let tenant = TenantId(format!("notif-pg-{}-{nonce}", std::process::id()));
    let region = Region("fr-par".into());
    let recipient = "psn:alice";
    delete_probe_rows(&admin, &tenant, &region).await;

    let store = PgInboxStore::new(app.clone());
    let critical_a = upsert(
        &tenant,
        &region,
        "crit-a",
        recipient,
        Reason::Sla,
        Class::Critical,
        "dedup-crit-a",
    );
    assert_eq!(
        store.upsert(&critical_a).await.unwrap(),
        InboxUpsertOutcome::Inserted
    );
    assert_eq!(
        store.upsert(&critical_a).await.unwrap(),
        InboxUpsertOutcome::Collapsed { coalesce_count: 2 }
    );

    let mut incompatible = critical_a.clone();
    incompatible.template_key = "git.pr.a-different-template".into();
    assert_eq!(
        store.upsert(&incompatible).await,
        Err(PgInboxError::WriteConflict),
        "a collapse key cannot silently rewrite immutable notification identity"
    );

    let other_region = Region("nl-ams".into());
    let region_collision = upsert(
        &tenant,
        &other_region,
        "crit-a",
        recipient,
        Reason::Sla,
        Class::Critical,
        "dedup-crit-a",
    );
    assert!(
        store.upsert(&region_collision).await.is_err(),
        "the tenant-wide collapse key cannot move an inbox row across residency regions"
    );

    for row in [
        upsert(
            &tenant,
            &region,
            "crit-b",
            recipient,
            Reason::ApprovalRequested,
            Class::Critical,
            "dedup-crit-b",
        ),
        upsert(
            &tenant,
            &region,
            "direct-a",
            recipient,
            Reason::Mentioned,
            Class::Direct,
            "dedup-direct-a",
        ),
        upsert(
            &tenant,
            &region,
            "fyi-a",
            recipient,
            Reason::Fyi,
            Class::Fyi,
            "dedup-fyi-a",
        ),
    ] {
        assert_eq!(
            store.upsert(&row).await.unwrap(),
            InboxUpsertOutcome::Inserted
        );
    }

    let scope = InboxReadScope {
        tenant: tenant.clone(),
        region: region.clone(),
        recipient: recipient.into(),
    };
    let first = store
        .list(&InboxReadRequest {
            scope: scope.clone(),
            filter: InboxFilter::all(),
            limit: 2,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.item.item_id.as_str())
            .collect::<Vec<_>>(),
        ["crit-a", "crit-b"]
    );
    assert_eq!(first.items[0].item.coalesce_count, 2);
    let second = store
        .list(&InboxReadRequest {
            scope: scope.clone(),
            filter: InboxFilter::all(),
            limit: 2,
            cursor: first.next_cursor,
        })
        .await
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|item| item.item.item_id.as_str())
            .collect::<Vec<_>>(),
        ["direct-a", "fyi-a"]
    );
    assert!(second.next_cursor.is_none());

    let mention_filter = InboxFilter {
        subsystems: None,
        reasons: Some(HashSet::from([Reason::Mentioned])),
    };
    let mentioned = store
        .list(&InboxReadRequest {
            scope: scope.clone(),
            filter: mention_filter,
            limit: 10,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(mentioned.items.len(), 1);
    assert_eq!(mentioned.items[0].item.item_id, "direct-a");

    let foreign = PgInboxStore::new(app.clone())
        .list(&InboxReadRequest {
            scope: InboxReadScope {
                recipient: "psn:bob".into(),
                ..scope.clone()
            },
            filter: InboxFilter::all(),
            limit: 10,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(foreign.items.is_empty(), "recipient scope is exact");

    // A fresh repository value over the same pool sees the committed rows: the inbox truth is in
    // PostgreSQL, not in process memory, and remains readable after composition is rebuilt.
    let after_rebuild = PgInboxStore::new(app)
        .list(&InboxReadRequest {
            scope,
            filter: InboxFilter::all(),
            limit: 10,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(after_rebuild.items.len(), 4);

    delete_probe_rows(&admin, &tenant, &region).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn durable_router_co_commits_dedup_inbox_and_outbox() {
    let config = MyelinConfig::dev();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&config.database_migration_url)
        .await
        .expect("connect migration role");
    PgMigrator::apply_validated(
        &admin,
        &myelin_storage::foundation_migrations(),
        &HotTables::none(),
    )
    .await
    .expect("apply foundation tables");
    PgMigrator::apply_validated(
        &admin,
        &myelin_notif::migrations::migrations(),
        &HotTables::none(),
    )
    .await
    .expect("apply notification tables");
    sqlx::query(
        "GRANT SELECT, INSERT, UPDATE, DELETE ON \
         notif_inbox_item, outbox, consumer_dedup, consumer_dead_letter TO myelin_app",
    )
    .execute(&admin)
    .await
    .expect("grant runtime tables");

    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&config.database_url)
        .await
        .expect("connect application role");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let tenant = TenantId(format!("notif-router-{}-{nonce}", std::process::id()));
    let region = Region("fr-par".into());
    let first_id = format!("router-first-{nonce}");
    let second_id = format!("router-second-{nonce}");
    delete_probe_rows(&admin, &tenant, &region).await;
    sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant' = $1")
        .bind(&tenant.0)
        .execute(&admin)
        .await
        .expect("clean probe outbox rows");
    sqlx::query("DELETE FROM consumer_dedup WHERE event_id = ANY($1)")
        .bind(vec![first_id.clone(), second_id.clone()])
        .execute(&admin)
        .await
        .expect("clean probe dedup rows");

    let runtime = tokio::runtime::Handle::current();
    let outbox = OutboxStore::durable(std::sync::Arc::new(PgOutboxBacking::new(
        app.clone(),
        runtime.clone(),
    )));
    let dedup = DedupLedger::durable(std::sync::Arc::new(
        myelin_storage::events_durable::DurableDedupBacking::new(app.clone(), runtime.clone()),
    ));
    let dead_letters: std::sync::Arc<dyn myelin_events::DurableDeadLetter> = std::sync::Arc::new(
        myelin_storage::events_durable::DurableDeadLetterBacking::new(app.clone(), runtime.clone()),
    );
    let consumer = myelin_notif::build_durable_router(
        &tenant,
        region.0.clone(),
        PgInboxStore::new(app.clone()),
        outbox.clone(),
        dedup,
        dead_letters.clone(),
        std::sync::Arc::new(UlidMinter::new()),
        runtime,
    )
    .expect("build durable router");

    let first = signal_envelope(&tenant, &region, &first_id);
    assert_eq!(
        consumer.deliver(&Message {
            subject: first.subject.0.clone(),
            envelope: first.clone(),
        }),
        Delivered::Acked
    );
    assert_eq!(
        consumer.deliver(&Message {
            subject: first.subject.0.clone(),
            envelope: first,
        }),
        Delivered::Deduplicated,
        "the committed consumer mark suppresses a redelivery"
    );

    let second = signal_envelope(&tenant, &region, &second_id);
    assert_eq!(
        consumer.deliver(&Message {
            subject: second.subject.0.clone(),
            envelope: second,
        }),
        Delivered::Acked,
        "a distinct event with the same inbox collapse key still commits its dedup mark"
    );

    let inbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notif_inbox_item WHERE tenant_id = $1 AND region = $2",
    )
    .bind(&tenant.0)
    .bind(&region.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    let coalesce_count: i32 = sqlx::query_scalar(
        "SELECT coalesce_count FROM notif_inbox_item WHERE tenant_id = $1 AND region = $2",
    )
    .bind(&tenant.0)
    .bind(&region.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM outbox \
         WHERE envelope->>'tenant' = $1 AND envelope->>'type_' = 'notif.item.created'",
    )
    .bind(&tenant.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    let dedup_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM consumer_dedup WHERE event_id = ANY($1)")
            .bind(vec![first_id.clone(), second_id.clone()])
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!((inbox_count, coalesce_count), (1, 2));
    assert_eq!(
        outbox_count, 1,
        "a collapse updates the row without re-pushing"
    );
    assert_eq!(dedup_count, 2);

    let no_tx_consumer = myelin_notif::build_durable_router(
        &tenant,
        region.0.clone(),
        PgInboxStore::new(app),
        outbox,
        DedupLedger::new(),
        dead_letters,
        std::sync::Arc::new(UlidMinter::new()),
        tokio::runtime::Handle::current(),
    )
    .expect("build missing-transaction probe router");
    let no_tx = signal_envelope(&tenant, &region, &format!("router-no-tx-{nonce}"));
    assert_eq!(
        no_tx_consumer.deliver(&Message {
            subject: no_tx.subject.0.clone(),
            envelope: no_tx,
        }),
        Delivered::Retried(2),
        "durable ingestion never falls back to a separate pool write when co-commit authority is absent"
    );
    let after_no_tx: (i64, i64) = (
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM notif_inbox_item WHERE tenant_id = $1 AND region = $2",
        )
        .bind(&tenant.0)
        .bind(&region.0)
        .fetch_one(&admin)
        .await
        .unwrap(),
        sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE envelope->>'tenant' = $1")
            .bind(&tenant.0)
            .fetch_one(&admin)
            .await
            .unwrap(),
    );
    assert_eq!(after_no_tx, (1, 1));

    delete_probe_rows(&admin, &tenant, &region).await;
    sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant' = $1")
        .bind(&tenant.0)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM consumer_dedup WHERE event_id = ANY($1)")
        .bind(vec![first_id, second_id])
        .execute(&admin)
        .await
        .unwrap();
}
