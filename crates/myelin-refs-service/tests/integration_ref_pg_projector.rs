#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, DedupLedger, Delivered, EventEnvelope, EventId,
    EventType, Message, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{build_pg_edge_consumer, edge_table_migrations, PgEdgeStore};
use myelin_storage::events_durable::{DurableDeadLetterBacking, DurableDedupBacking};
use myelin_storage::PgMigrator;
use myelin_tenancy::{ArtifactRef, Region, TenantId};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL").unwrap_or_else(|_| {
        app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn delivered_reference_edges_and_their_dedup_tombstone_commit_together() {
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect as migration role");
    PgMigrator::apply(&admin, &edge_table_migrations())
        .await
        .expect("apply the production edge migration");

    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&app_url())
        .await
        .expect("connect as runtime role");
    let tenant = TenantId(format!("refs-projector-{}", std::process::id()));
    let region = Region("fr-par".into());
    let source = ArtifactRef(format!("myelin://{}/chat/message/M1", tenant.0));
    let target = ArtifactRef(format!("myelin://{}/issue/issue/ENG-41", tenant.0));
    let event = EventEnvelope {
        event_id: EventId(format!("refs-projector-event-{}", std::process::id())),
        type_: EventType("refs.edge.created".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: region.clone(),
        actor: Actor(Principal::stub(
            PrincipalId("agent:release-helper".into()),
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("runtime:test".into()),
                on_behalf_of: Some(PrincipalId("human:operator".into())),
            },
            tenant.clone(),
        )),
        subject: source.clone(),
        aggregate: AggregateKey("refs-edge:M1-ENG-41".into()),
        causation_id: None,
        correlation_id: CorrelationId("release-failure".into()),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-08-10T00:00:00Z".into()),
        recorded_at: Timestamp("2026-08-10T00:00:01Z".into()),
        payload: serde_json::json!({
            "source": source.0,
            "target": target.0,
            "rel": "links",
        }),
    };
    let runtime = tokio::runtime::Handle::current();
    let dedup = DedupLedger::durable(Arc::new(DurableDedupBacking::new(
        app.clone(),
        runtime.clone(),
    )));
    let dead_letters: Arc<dyn myelin_events::DurableDeadLetter> =
        Arc::new(DurableDeadLetterBacking::new(app.clone(), runtime.clone()));
    let consumer = build_pg_edge_consumer(
        &tenant,
        &region,
        PgEdgeStore::new(app.clone()),
        dedup,
        dead_letters,
        runtime,
    )
    .expect("build the tenant-bound projector");
    let message = Message {
        subject: source.0.clone(),
        envelope: event.clone(),
    };

    assert_eq!(consumer.deliver(&message), Delivered::Acked);
    assert_eq!(
        consumer.deliver(&message),
        Delivered::Deduplicated,
        "a broker replay does not rewrite the projection"
    );

    let mut connection = app.acquire().await.unwrap();
    sqlx::query(
        "SELECT set_config('myelin.tenant_id',$1,false), set_config('myelin.region',$2,false)",
    )
    .bind(&tenant.0)
    .bind(&region.0)
    .execute(&mut *connection)
    .await
    .unwrap();
    let row: (String, String, String, String, bool) = sqlx::query_as(
        "SELECT source, target, rel, origin_actor, tombstoned
           FROM edge WHERE tenant_id = $1",
    )
    .bind(&tenant.0)
    .fetch_one(&mut *connection)
    .await
    .expect("the consumer committed one queryable edge");
    assert_eq!(row.0, source.0);
    assert_eq!(row.1, target.0);
    assert_eq!(row.2, "links");
    assert_eq!(row.3, "agent:release-helper");
    assert!(!row.4);

    sqlx::query("DELETE FROM edge WHERE tenant_id = $1")
        .bind(&tenant.0)
        .execute(&admin)
        .await
        .unwrap();
}
