#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, Consumer, CorrelationId, DataRole, DedupLedger, Delivered, EventEnvelope,
    EventId, EventType, Message, Reason, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{
    build_pg_cell_edge_consumer, build_pg_edge_consumer, edge_id, edge_table_migrations,
    PgEdgeProjector, PgEdgeStore,
};
use myelin_storage::events_durable::{DurableDeadLetterBacking, DurableDedupBacking};
use myelin_storage::placement_durable::placement_durable_migrations;
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

fn legacy_edge_id(tenant: &TenantId, source: &str, target: &str, rel: &str) -> String {
    let mut hash: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013b;
    for field in [
        tenant.0.as_bytes(),
        source.as_bytes(),
        target.as_bytes(),
        rel.as_bytes(),
    ] {
        for byte in field {
            hash ^= u128::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:032x}")
}

async fn migrated_admin_pool() -> PgPool {
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect as migration role");
    let migration_pool = admin.clone();
    MIGRATED
        .get_or_init(|| async move {
            PgMigrator::apply(&migration_pool, &placement_durable_migrations())
                .await
                .expect("apply the production placement migrations");
            PgMigrator::apply(&migration_pool, &edge_table_migrations())
                .await
                .expect("apply the production edge migration");
        })
        .await;
    admin
}

#[allow(clippy::too_many_arguments)]
fn edge_event(
    tenant: &TenantId,
    region: &Region,
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

struct ProjectorHarness {
    admin: PgPool,
    app: PgPool,
    tenant: TenantId,
    region: Region,
    consumer: Consumer<PgEdgeProjector>,
}

impl ProjectorHarness {
    async fn start(story: &str) -> Self {
        let admin = migrated_admin_pool().await;

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
        edge_event(
            &self.tenant,
            &self.region,
            event_id,
            event_type,
            recorded_at,
            source,
            target,
            rel,
            rel_class,
        )
    }

    fn deliver(&self, event: &EventEnvelope) -> Delivered {
        self.consumer.deliver(&Message {
            subject: event.subject.0.clone(),
            envelope: event.clone(),
        })
    }

    fn consumer_for(&self, region: &Region) -> Consumer<PgEdgeProjector> {
        let runtime = tokio::runtime::Handle::current();
        build_pg_edge_consumer(
            &self.tenant,
            region,
            PgEdgeStore::new(self.app.clone()),
            DedupLedger::durable(Arc::new(DurableDedupBacking::new(
                self.app.clone(),
                runtime.clone(),
            ))),
            Arc::new(DurableDeadLetterBacking::new(
                self.app.clone(),
                runtime.clone(),
            )),
            runtime,
        )
        .expect("build another region-bound projector")
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
async fn an_existing_reference_keeps_its_legacy_handle_as_new_events_converge() {
    let story = ProjectorHarness::start("legacy-identity").await;
    let source = ArtifactRef(format!("myelin://{}/chat/message/M1", story.tenant.0));
    let target = story.issue("ENG-41");
    let created = story.event(
        &format!("refs-legacy-create-{}", std::process::id()),
        "refs.edge.created",
        "2026-08-11T00:00:01Z",
        &source,
        &target,
        "links",
        "reference",
    );
    assert_eq!(story.deliver(&created), Delivered::Acked);

    let strong_id = edge_id(&story.tenant, &source.0, &target.0, "links");
    let legacy_id = legacy_edge_id(&story.tenant, &source.0, &target.0, "links");
    assert_ne!(strong_id, legacy_id);
    sqlx::query(
        "UPDATE edge
            SET edge_id = $4
          WHERE tenant_id = $1 AND region = $2 AND edge_id = $3",
    )
    .bind(&story.tenant.0)
    .bind(&story.region.0)
    .bind(&strong_id)
    .bind(&legacy_id)
    .execute(&story.admin)
    .await
    .expect("stand in for a reference persisted by the previous identity scheme");

    let refreshed = story.event(
        &format!("refs-legacy-refresh-{}", std::process::id()),
        "refs.edge.created",
        "2026-08-11T00:00:02Z",
        &source,
        &target,
        "links",
        "reference",
    );
    assert_eq!(story.deliver(&refreshed), Delivered::Acked);
    let graph = PgEdgeStore::new(story.app.clone());
    let backlinks = graph
        .inbound_live(&story.tenant, &story.region, &target, 10)
        .await
        .expect("read the converged reference");
    assert_eq!(backlinks.len(), 1, "replay never forks the old reference");
    assert_eq!(
        backlinks[0].edge_id, legacy_id,
        "opaque handles already handed to clients remain stable"
    );

    let mut removed = story.event(
        &format!("refs-legacy-remove-{}", std::process::id()),
        "refs.edge.removed",
        "2026-08-11T00:00:03Z",
        &source,
        &target,
        "links",
        "reference",
    );
    removed.payload = serde_json::json!({ "edge_id": legacy_id });
    assert_eq!(story.deliver(&removed), Delivered::Acked);
    assert!(graph
        .inbound_live(&story.tenant, &story.region, &target, 10)
        .await
        .expect("read after removing through the legacy handle")
        .is_empty());

    story.clean_up().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_identity_collision_is_parked_without_overwriting_either_reference() {
    let story = ProjectorHarness::start("identity-collision").await;
    let protected_source = ArtifactRef(format!(
        "myelin://{}/chat/message/protected",
        story.tenant.0
    ));
    let protected_target = story.issue("SAFE-1");
    let protected = story.event(
        &format!("refs-collision-protected-{}", std::process::id()),
        "refs.edge.created",
        "2026-08-11T00:00:01Z",
        &protected_source,
        &protected_target,
        "links",
        "reference",
    );
    assert_eq!(story.deliver(&protected), Delivered::Acked);

    let incoming_source = ArtifactRef(format!("myelin://{}/chat/message/incoming", story.tenant.0));
    let incoming_target = story.issue("NEW-1");
    let colliding_id = edge_id(
        &story.tenant,
        &incoming_source.0,
        &incoming_target.0,
        "links",
    );
    sqlx::query(
        "UPDATE edge
            SET edge_id = $3
          WHERE tenant_id = $1 AND region = $2",
    )
    .bind(&story.tenant.0)
    .bind(&story.region.0)
    .bind(&colliding_id)
    .execute(&story.admin)
    .await
    .expect("simulate a digest collision without needing to break BLAKE3");

    let incoming = story.event(
        &format!("refs-collision-incoming-{}", std::process::id()),
        "refs.edge.created",
        "2026-08-11T00:00:02Z",
        &incoming_source,
        &incoming_target,
        "links",
        "reference",
    );
    match story.deliver(&incoming) {
        Delivered::DeadLettered(Reason(reason)) => assert!(
            reason.contains("identity collision"),
            "the parked event explains the permanent conflict: {reason}"
        ),
        other => panic!("a durable identity collision must be parked, got {other:?}"),
    }

    let graph = PgEdgeStore::new(story.app.clone());
    assert!(graph
        .inbound_live(&story.tenant, &story.region, &incoming_target, 10)
        .await
        .expect("look for the rejected reference")
        .is_empty());
    let protected_backlinks = graph
        .inbound_live(&story.tenant, &story.region, &protected_target, 10)
        .await
        .expect("read the protected reference");
    assert_eq!(protected_backlinks.len(), 1);
    assert_eq!(protected_backlinks[0].source, protected_source.0);

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

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_can_page_the_links_it_makes_without_rescanning() {
    let story = ProjectorHarness::start("outbound-keyset").await;
    let source = ArtifactRef(format!("myelin://{}/git/pr/platform:42", story.tenant.0));
    for sequence in 1..=3 {
        let target = story.issue(&format!("ENG-{sequence}"));
        let event = story.event(
            &format!("refs-outbound-{sequence}-{}", std::process::id()),
            "refs.edge.created",
            &format!("2026-08-10T00:00:0{sequence}Z"),
            &source,
            &target,
            "closes",
            "lifecycle",
        );
        assert_eq!(story.deliver(&event), Delivered::Acked);
    }

    let graph = PgEdgeStore::new(story.app.clone());
    let first = graph
        .outbound_live_after(&story.tenant, &story.region, &source, None, 2)
        .await
        .expect("read the first bounded page of links");
    assert_eq!(first.len(), 2);
    assert!(first[0].edge_id < first[1].edge_id);

    let second = graph
        .outbound_live_after(
            &story.tenant,
            &story.region,
            &source,
            Some(&first[1].edge_id),
            2,
        )
        .await
        .expect("continue strictly after the first page of links");
    assert_eq!(second.len(), 1);
    assert!(second[0].edge_id > first[1].edge_id);

    story.clean_up().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_same_link_can_live_in_two_regions_and_disappear_from_only_one() {
    let story = ProjectorHarness::start("regional-identity").await;
    let second_region = Region("us-east".into());
    let second_consumer = story.consumer_for(&second_region);
    let source = ArtifactRef(format!("myelin://{}/chat/message/M1", story.tenant.0));
    let target = story.issue("ENG-41");
    let first_created = story.event(
        &format!("refs-region-first-create-{}", std::process::id()),
        "refs.edge.created",
        "2026-08-11T00:00:01Z",
        &source,
        &target,
        "links",
        "reference",
    );
    let second_created = edge_event(
        &story.tenant,
        &second_region,
        &format!("refs-region-second-create-{}", std::process::id()),
        "refs.edge.created",
        "2026-08-11T00:00:02Z",
        &source,
        &target,
        "links",
        "reference",
    );

    assert_eq!(story.deliver(&first_created), Delivered::Acked);
    assert_eq!(
        second_consumer.deliver(&Message {
            subject: source.0.clone(),
            envelope: second_created,
        }),
        Delivered::Acked,
        "the same semantic link is independent work in another tenant region"
    );

    let graph = PgEdgeStore::new(story.app.clone());
    assert_eq!(
        graph
            .inbound_live(&story.tenant, &story.region, &target, 10)
            .await
            .expect("read the first region")
            .len(),
        1
    );
    assert_eq!(
        graph
            .inbound_live(&story.tenant, &second_region, &target, 10)
            .await
            .expect("read the second region")
            .len(),
        1
    );

    let first_removed = story.event(
        &format!("refs-region-first-remove-{}", std::process::id()),
        "refs.edge.removed",
        "2026-08-11T00:00:03Z",
        &source,
        &target,
        "links",
        "reference",
    );
    assert_eq!(story.deliver(&first_removed), Delivered::Acked);
    assert!(graph
        .inbound_live(&story.tenant, &story.region, &target, 10)
        .await
        .expect("the removal is visible in its own region")
        .is_empty());
    assert_eq!(
        graph
            .inbound_live(&story.tenant, &second_region, &target, 10)
            .await
            .expect("the other region remains intact")
            .len(),
        1,
        "regional deletion must not erase a separate regional projection"
    );

    story.clean_up().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_tenant_can_start_linking_work_without_restarting_refs() {
    let admin = migrated_admin_pool().await;
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&app_url())
        .await
        .expect("connect as runtime role");
    let suffix = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time follows the Unix epoch")
            .as_nanos()
    );
    let cell_id = format!("refs-live-cell-{suffix}");
    let tenant = TenantId(format!("refs-live-tenant-{suffix}"));
    let region = Region("fr-par".into());
    let runtime = tokio::runtime::Handle::current();
    let consumer = build_pg_cell_edge_consumer(
        &cell_id,
        &region,
        PgEdgeStore::new(app.clone()),
        DedupLedger::durable(Arc::new(DurableDedupBacking::new(
            app.clone(),
            runtime.clone(),
        ))),
        Arc::new(DurableDeadLetterBacking::new(app.clone(), runtime.clone())),
        runtime,
    )
    .expect("build one cell-bound projector before the tenant exists");
    let source = ArtifactRef(format!("myelin://{}/chat/message/M1", tenant.0));
    let target = ArtifactRef(format!("myelin://{}/issue/issue/ENG-41", tenant.0));
    let event = edge_event(
        &tenant,
        &region,
        &format!("refs-live-tenant-{suffix}"),
        "refs.edge.created",
        "2026-08-11T00:00:01Z",
        &source,
        &target,
        "links",
        "reference",
    );
    let message = Message {
        subject: source.0.clone(),
        envelope: event,
    };

    assert_eq!(
        consumer.deliver(&message),
        Delivered::Retried(2),
        "work for a tenant still converging into the cell waits instead of being discarded"
    );

    sqlx::query(
        "INSERT INTO local_tenant (cell_id, tenant_id, isolation_tier, active)
         VALUES ($1, $2, 'Pool', true)",
    )
    .bind(&cell_id)
    .bind(&tenant.0)
    .execute(&admin)
    .await
    .expect("finish placing the new tenant while the Refs worker stays alive");

    assert_eq!(consumer.deliver(&message), Delivered::Acked);
    let backlinks = PgEdgeStore::new(app)
        .inbound_live(&tenant, &region, &target, 10)
        .await
        .expect("the newly placed tenant can immediately read its projected backlink");
    assert_eq!(backlinks.len(), 1);
    assert_eq!(backlinks[0].source, source.0);
    assert_eq!(backlinks[0].target, target.0);

    sqlx::query("DELETE FROM edge WHERE tenant_id = $1")
        .bind(&tenant.0)
        .execute(&admin)
        .await
        .expect("clean up the tenant's projected edge");
    sqlx::query("DELETE FROM local_tenant WHERE cell_id = $1 AND tenant_id = $2")
        .bind(&cell_id)
        .bind(&tenant.0)
        .execute(&admin)
        .await
        .expect("clean up the late placement");
}
