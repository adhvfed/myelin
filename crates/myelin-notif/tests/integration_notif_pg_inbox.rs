//! Live-PostgreSQL proof for the durable notification inbox repository.
#![cfg(feature = "integration")]

use std::collections::HashSet;

use myelin_config::MyelinConfig;
use myelin_events::ArtifactRef;
use myelin_notif::list_inbox::InboxFilter;
use myelin_notif::pg_inbox::{
    InboxReadRequest, InboxReadScope, InboxUpsert, InboxUpsertOutcome, PgInboxError, PgInboxStore,
};
use myelin_notif::router::RoutedInboxItem;
use myelin_notif::{Class, Reason};
use myelin_storage::migration::HotTables;
use myelin_storage::PgMigrator;
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
