#![cfg(feature = "integration")]

use std::collections::HashSet;

use chrono::{Duration, Utc};
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

static DATABASE_SETUP: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

async fn delete_outbox_rows(admin: &sqlx::PgPool, tenant: &TenantId) {
    for _ in 0..5 {
        sqlx::query(
            "DELETE FROM outbox_quarantine WHERE event_id IN \
             (SELECT event_id FROM outbox WHERE envelope->>'tenant' = $1)",
        )
        .bind(&tenant.0)
        .execute(admin)
        .await
        .expect("delete probe outbox quarantine rows");
        if sqlx::query("DELETE FROM outbox WHERE envelope->>'tenant' = $1")
            .bind(&tenant.0)
            .execute(admin)
            .await
            .is_ok()
        {
            return;
        }
    }
    panic!(
        "probe outbox rows remained referenced by the publisher quarantine for tenant {}",
        tenant.0
    );
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

    {
        let _setup = DATABASE_SETUP.lock().await;
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
    }

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
    let critical_scope = InboxReadScope {
        tenant: tenant.clone(),
        region: region.clone(),
        recipient: recipient.into(),
    };
    store
        .mark_read(&critical_scope, "crit-a")
        .await
        .expect("finish the first two collapsed events");
    let mut later_critical = critical_a.clone();
    later_critical.item.origin_event = ArtifactRef(format!(
        "myelin://{}/bus/event/event-crit-a-later",
        tenant.0
    ));
    later_critical.occurred_at = "2026-07-22T12:01:00Z".into();
    assert_eq!(
        store.upsert(&later_critical).await.unwrap(),
        InboxUpsertOutcome::Collapsed { coalesce_count: 3 },
    );
    let reopened = store.get(&critical_scope, "crit-a").await.unwrap();
    assert_eq!(
        reopened.item.state, "unread",
        "new activity returns a previously finished item to the user's attention",
    );
    assert_eq!(reopened.item.origin_event, later_critical.item.origin_event);
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(&reopened.occurred_at).unwrap(),
        chrono::DateTime::parse_from_rfc3339(&later_critical.occurred_at).unwrap(),
    );

    let critical_snooze_until = Utc::now()
        .checked_add_signed(Duration::milliseconds(100))
        .unwrap();
    store
        .snooze(&critical_scope, "crit-a", critical_snooze_until)
        .await
        .expect("park the collapsed item deliberately");
    let mut while_snoozed = later_critical.clone();
    while_snoozed.item.origin_event = ArtifactRef(format!(
        "myelin://{}/bus/event/event-crit-a-snoozed",
        tenant.0
    ));
    while_snoozed.occurred_at = "2026-07-22T12:02:00Z".into();
    assert_eq!(
        store.upsert(&while_snoozed).await.unwrap(),
        InboxUpsertOutcome::Collapsed { coalesce_count: 4 },
    );
    let parked = store.get(&critical_scope, "crit-a").await.unwrap();
    assert_eq!(parked.item.state, "snoozed");
    let stored_snooze = chrono::DateTime::parse_from_rfc3339(
        parked
            .item
            .snooze_until
            .as_deref()
            .expect("the explicit snooze remains scheduled"),
    )
    .unwrap();
    assert_eq!(
        stored_snooze.timestamp_micros(),
        critical_snooze_until.timestamp_micros(),
        "fresh activity respects an explicit snooze window at PostgreSQL precision",
    );
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        store
            .get(&critical_scope, "crit-a")
            .await
            .unwrap()
            .item
            .state,
        "unread",
        "the ordinary due-snooze path brings the accumulated activity back",
    );

    let mut late_critical = while_snoozed.clone();
    late_critical.item.origin_event =
        ArtifactRef(format!("myelin://{}/bus/event/event-crit-a-late", tenant.0));
    late_critical.occurred_at = "2026-07-22T11:59:00Z".into();
    assert_eq!(
        store.upsert(&late_critical).await.unwrap(),
        InboxUpsertOutcome::Collapsed { coalesce_count: 5 },
    );
    let after_late_arrival = store.get(&critical_scope, "crit-a").await.unwrap();
    assert_eq!(
        after_late_arrival.item.origin_event,
        while_snoozed.item.origin_event
    );
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(&after_late_arrival.occurred_at).unwrap(),
        chrono::DateTime::parse_from_rfc3339(&while_snoozed.occurred_at).unwrap(),
        "late delivery counts as activity without moving the notification backward in time",
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

    let mut critical_b = upsert(
        &tenant,
        &region,
        "crit-b",
        recipient,
        Reason::ApprovalRequested,
        Class::Critical,
        "dedup-crit-b",
    );
    critical_b.occurred_at = "2026-07-22T12:00:01Z".into();
    for row in [
        critical_b,
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
        ["crit-a", "crit-b"],
        "within one priority band the newest work must be visible first"
    );
    assert_eq!(first.items[0].item.coalesce_count, 5);

    let newest_critical = store
        .list(&InboxReadRequest {
            scope: scope.clone(),
            filter: InboxFilter::all(),
            limit: 1,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(newest_critical.items[0].item.item_id, "crit-a");
    let older_critical = store
        .list(&InboxReadRequest {
            scope: scope.clone(),
            filter: InboxFilter::all(),
            limit: 1,
            cursor: newest_critical.next_cursor,
        })
        .await
        .unwrap();
    assert_eq!(
        older_critical.items[0].item.item_id, "crit-b",
        "the recency cursor must neither skip nor repeat work within one priority band"
    );

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

    assert!(store.complete_if_present(&scope, "crit-a").await.unwrap());
    assert!(store.complete_if_present(&scope, "crit-b").await.unwrap());
    assert_eq!(
        store
            .snooze(
                &scope,
                "crit-a",
                Utc::now().checked_add_signed(Duration::hours(1)).unwrap(),
            )
            .await,
        Err(PgInboxError::InvalidState),
        "completed work cannot be returned to an active state by snoozing it"
    );
    let after_approvals_finished = store
        .list(&InboxReadRequest {
            scope: scope.clone(),
            filter: InboxFilter::all(),
            limit: 4,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        after_approvals_finished
            .items
            .iter()
            .map(|item| item.item.item_id.as_str())
            .collect::<Vec<_>>(),
        ["direct-a", "fyi-a", "crit-a", "crit-b"],
        "completed critical work must not bury lower-priority unread work"
    );

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

    let addressed = store.get(&scope, "direct-a").await.unwrap();
    assert_eq!(addressed, mentioned.items[0]);
    assert_eq!(
        store
            .get(
                &InboxReadScope {
                    recipient: "psn:bob".into(),
                    ..scope.clone()
                },
                "direct-a",
            )
            .await,
        Err(PgInboxError::NotFound),
        "a recipient cannot address another recipient's inbox item"
    );

    store.mark_read(&scope, "direct-a").await.unwrap();
    let after_reading_the_mention = store
        .list(&InboxReadRequest {
            scope: scope.clone(),
            filter: InboxFilter::all(),
            limit: 4,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        after_reading_the_mention
            .items
            .iter()
            .map(|item| item.item.item_id.as_str())
            .collect::<Vec<_>>(),
        ["fyi-a", "direct-a", "crit-a", "crit-b"],
        "unread work precedes read work before reason priority is considered"
    );
    assert_eq!(
        store
            .mark_read(
                &InboxReadScope {
                    recipient: "psn:bob".into(),
                    ..scope.clone()
                },
                "direct-a",
            )
            .await,
        Err(PgInboxError::NotFound),
        "a recipient cannot mutate another recipient's inbox item"
    );

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

    assert_eq!(
        store
            .snooze(
                &scope,
                "direct-a",
                Utc::now().checked_sub_signed(Duration::seconds(1)).unwrap(),
            )
            .await,
        Err(PgInboxError::InvalidInput),
        "an expired snooze is rejected instead of immediately changing state"
    );
    store
        .snooze(
            &scope,
            "direct-a",
            Utc::now()
                .checked_add_signed(Duration::milliseconds(100))
                .unwrap(),
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let resurfaced = store.get(&scope, "direct-a").await.unwrap();
    assert_eq!(resurfaced.item.state, "unread");
    assert!(resurfaced.item.snooze_until.is_none());

    let snooze_until = Utc::now().checked_add_signed(Duration::hours(1)).unwrap();
    store
        .snooze(&scope, "direct-a", snooze_until)
        .await
        .unwrap();
    let parked = store.get(&scope, "direct-a").await.unwrap();
    assert_eq!(parked.item.state, "snoozed");
    assert!(parked.item.snooze_until.is_some());
    let active = store
        .list(&InboxReadRequest {
            scope: scope.clone(),
            filter: InboxFilter::all(),
            limit: 10,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(active.items.len(), 3);
    assert!(
        active
            .items
            .iter()
            .all(|item| item.item.item_id != "direct-a"),
        "a future snooze parks the item outside every active inbox page"
    );

    assert_eq!(
        store
            .mark_all_read(&scope, &InboxFilter::all())
            .await
            .unwrap(),
        1,
        "bulk read marks the remaining unread item but leaves completed and snoozed work alone"
    );
    assert_eq!(
        store
            .mark_all_read(&scope, &InboxFilter::all())
            .await
            .unwrap(),
        0
    );
    let all_read = store
        .list(&InboxReadRequest {
            scope: scope.clone(),
            filter: InboxFilter::all(),
            limit: 10,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(
        all_read
            .items
            .iter()
            .find(|item| item.item.item_id == "fyi-a")
            .unwrap()
            .item
            .state,
        "read"
    );
    assert!(all_read
        .items
        .iter()
        .filter(|item| item.item.item_id.starts_with("crit-"))
        .all(|item| item.item.state == "done"));
    assert_eq!(
        store
            .mark_all_read(
                &InboxReadScope {
                    recipient: "psn:bob".into(),
                    ..scope.clone()
                },
                &InboxFilter::all(),
            )
            .await
            .unwrap(),
        0,
        "bulk read state is recipient scoped"
    );

    store.mark_read(&scope, "direct-a").await.unwrap();

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
    assert_eq!(
        after_rebuild
            .items
            .iter()
            .find(|item| item.item.item_id == "direct-a")
            .unwrap()
            .item
            .state,
        "read",
        "read state survives a new store instance"
    );

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
    {
        let _setup = DATABASE_SETUP.lock().await;
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
    }

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
    delete_outbox_rows(&admin, &tenant).await;
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
    delete_outbox_rows(&admin, &tenant).await;
    sqlx::query("DELETE FROM consumer_dedup WHERE event_id = ANY($1)")
        .bind(vec![first_id, second_id])
        .execute(&admin)
        .await
        .unwrap();
}
