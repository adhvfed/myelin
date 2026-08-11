#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, Consumer, CorrelationId, DataRole, DedupLedger, Delivered, EventEnvelope,
    EventId, EventType, Message, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    build_pg_edge_consumer, edge_table_migrations, PgEdgeProjector, PgEdgeStore,
};
use myelin_storage::events_durable::{DurableDeadLetterBacking, DurableDedupBacking};
use myelin_storage::PgMigrator;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use sqlx::PgPool;
use tokio::sync::OnceCell;

static MIGRATED: OnceCell<()> = OnceCell::const_new();

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    std::env::var("DATABASE_MIGRATION_URL").unwrap_or_else(|_| {
        app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
    })
}

struct ProjectorHarness {
    admin: PgPool,
    app: PgPool,
    tenant: TenantId,
    region: Region,
    consumer: Consumer<PgEdgeProjector>,
}

impl ProjectorHarness {
    async fn start(story: &str) -> Self {
        let admin = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url())
            .await
            .expect("connect as migration role");
        let migration_pool = admin.clone();
        MIGRATED
            .get_or_init(|| async move {
                PgMigrator::apply(&migration_pool, &edge_table_migrations())
                    .await
                    .expect("apply the production edge migration");
            })
            .await;

        let app = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&app_url())
            .await
            .expect("connect as runtime role");
        let tenant = TenantId(format!("refs-{story}-{}", std::process::id()));
        let region = Region("fr-par".into());
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
        Self {
            admin,
            app,
            tenant,
            region,
            consumer,
        }
    }

    fn issue(&self, key: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{}/issue/issue/{key}", self.tenant.0))
    }

    fn event(
        &self,
        event_id: &str,
        event_type: &str,
        recorded_at: &str,
        source: &ArtifactRef,
        target: &ArtifactRef,
        rel: &str,
        rel_class: &str,
    ) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(event_id.into()),
            type_: EventType(event_type.into()),
            schema_ver: 1,
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            actor: Actor(Principal::stub(
                PrincipalId("agent:release-helper".into()),
                PrincipalKind::Agent {
                    runtime_ref: myelin_identity::RuntimeRef("runtime:test".into()),
                    on_behalf_of: Some(PrincipalId("human:operator".into())),
                },
                self.tenant.clone(),
            )),
            subject: source.clone(),
            aggregate: AggregateKey(format!("refs-edge:{}:{}", source.0, target.0)),
            causation_id: None,
            correlation_id: CorrelationId(format!("story:{event_id}")),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp(recorded_at.into()),
            recorded_at: Timestamp(recorded_at.into()),
            payload: serde_json::json!({
                "source": source.0,
                "target": target.0,
                "rel": rel,
                "rel_class": rel_class,
            }),
        }
    }

    fn deliver(&self, event: &EventEnvelope) -> Delivered {
        self.consumer.deliver(&Message {
            subject: event.subject.0.clone(),
            envelope: event.clone(),
        })
    }

    async fn edge_states(&self) -> Vec<(String, String, bool, String)> {
        let mut connection = self.app.acquire().await.expect("acquire app connection");
        sqlx::query(
            "SELECT set_config('myelin.tenant_id',$1,false), set_config('myelin.region',$2,false)",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .execute(&mut *connection)
        .await
        .expect("bind tenant scope");
        sqlx::query_as(
            "SELECT rel, rel_class, tombstoned, origin_event
               FROM edge
              WHERE tenant_id = $1
              ORDER BY rel",
        )
        .bind(&self.tenant.0)
        .fetch_all(&mut *connection)
        .await
        .expect("read the projected edges")
    }

    async fn clean_up(self) {
        sqlx::query("DELETE FROM edge WHERE tenant_id = $1")
            .bind(&self.tenant.0)
            .execute(&self.admin)
            .await
            .expect("clean up story edges");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delivered_reference_edge_and_its_dedup_record_commit_together() {
    let story = ProjectorHarness::start("reference").await;
    let source = ArtifactRef(format!("myelin://{}/chat/message/M1", story.tenant.0));
    let target = story.issue("ENG-41");
    let event = story.event(
        &format!("refs-reference-create-{}", std::process::id()),
        "refs.edge.created",
        "2026-08-10T00:00:01Z",
        &source,
        &target,
        "links",
        "reference",
    );

    assert_eq!(story.deliver(&event), Delivered::Acked);
    assert_eq!(
        story.deliver(&event),
        Delivered::Deduplicated,
        "a broker replay does not rewrite the projection"
    );
    assert_eq!(
        story.edge_states().await,
        vec![(
            "links".into(),
            "reference".into(),
            false,
            event.event_id.0.clone(),
        )]
    );

    story.clean_up().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_issue_dependency_stays_gone_when_its_creation_arrives_late() {
    let story = ProjectorHarness::start("lifecycle").await;
    let planning = story.issue("PLAN-1");
    let delivery = story.issue("SHIP-1");
    let created = story.event(
        &format!("refs-lifecycle-create-{}", std::process::id()),
        "refs.edge.created",
        "2026-08-10T00:00:01Z",
        &planning,
        &delivery,
        "blocks",
        "lifecycle",
    );
    let removed = story.event(
        &format!("refs-lifecycle-remove-{}", std::process::id()),
        "refs.edge.removed",
        "2026-08-10T00:00:02Z",
        &planning,
        &delivery,
        "blocks",
        "lifecycle",
    );

    assert_eq!(story.deliver(&removed), Delivered::Acked);
    assert_eq!(
        story.deliver(&created),
        Delivered::Acked,
        "the broker may deliver an older creation after its removal"
    );
    assert_eq!(
        story.edge_states().await,
        vec![
            (
                "blocked_by".into(),
                "lifecycle".into(),
                true,
                removed.event_id.0.clone(),
            ),
            (
                "blocks".into(),
                "lifecycle".into(),
                true,
                removed.event_id.0.clone(),
            ),
        ],
        "both navigable directions remain tombstoned by the newer fact"
    );

    story.clean_up().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hot_reference_can_be_read_forward_without_rescanning_or_duplicates() {
    let story = ProjectorHarness::start("keyset").await;
    let target = story.issue("ENG-41");
    for sequence in 1..=3 {
        let source = ArtifactRef(format!(
            "myelin://{}/chat/message/M{sequence}",
            story.tenant.0
        ));
        let event = story.event(
            &format!("refs-keyset-{sequence}-{}", std::process::id()),
            "refs.edge.created",
            &format!("2026-08-10T00:00:0{sequence}Z"),
            &source,
            &target,
            "links",
            "reference",
        );
        assert_eq!(story.deliver(&event), Delivered::Acked);
    }

    let graph = PgEdgeStore::new(story.app.clone());
    let first = graph
        .inbound_live_after(&story.tenant, &story.region, &target, None, 2)
        .await
        .expect("read the first bounded page");
    assert_eq!(first.len(), 2);
    assert!(first[0].edge_id < first[1].edge_id);

    let second = graph
        .inbound_live_after(
            &story.tenant,
            &story.region,
            &target,
            Some(&first[1].edge_id),
            2,
        )
        .await
        .expect("continue strictly after the first page");
    assert_eq!(second.len(), 1);
    assert!(second[0].edge_id > first[1].edge_id);

    story.clean_up().await;
}
