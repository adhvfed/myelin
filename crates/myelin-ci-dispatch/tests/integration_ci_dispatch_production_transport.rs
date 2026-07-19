//! Live proof of the complete production dispatch composition over PG + NATS + Git + S3.
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_ci_controlplane::{ci_durable_migrations, ci_run_store_factory};
use myelin_ci_dispatch::{
    build_dispatch_consumers, dispatch_app_spec_with_intake, git_intake_filter,
    AuthoritativeGitRoot, EVENT_SUBJECT_ROOT,
};
use myelin_config::MyelinConfig;
use myelin_events::nats::{
    JetStreamConsumerConfig, JetStreamPublisherConfig, NatsJetStreamBus, NatsJetStreamPublisher,
};
use myelin_events::relay::{EventConsumer, EventPublisher};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, BrokerDeliveryRef, CorrelationId, DataRole, DedupLedger,
    DeliveryQuarantineReason, DurableDedup, DurableDeliveryQuarantine, EventEnvelope, EventId,
    EventType, OutboxStore, Timestamp, UlidMinter, Visibility,
    CONSUMER_DEAD_LETTER_MIGRATION, CONSUMER_DEDUP_MIGRATION,
    CONSUMER_DELIVERY_QUARANTINE_MIGRATION, OUTBOX_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::events_durable::{DurableDeadLetterBacking, DurableDedupBacking};
use myelin_storage::outbox_durable::PgOutboxBacking;
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::{BlobStore, ContentHash};
use myelin_substrate::{boot, Config};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};

const CI_TOML: &[u8] = br#"on = "push"

[[jobs]]
name = "build"
image = "registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000"
command = ["build"]
"#;

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

async fn isolated_pool(schema: &str) -> PgPool {
    let schema_owned = schema.to_string();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .after_connect(move |connection, _| {
            let schema = schema_owned.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("connect isolated live PG");
    pool.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    pool.execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    for migration in ci_durable_migrations().0.iter() {
        pool.execute(migration.ddl).await.unwrap();
    }
    for ddl in [
        OUTBOX_MIGRATION,
        CONSUMER_DEDUP_MIGRATION,
        CONSUMER_DEAD_LETTER_MIGRATION,
        CONSUMER_DELIVERY_QUARANTINE_MIGRATION,
    ] {
        pool.execute(ddl).await.unwrap();
    }
    pool
}

fn seed_git(root: &std::path::Path, tenant: &str, region: &str) -> String {
    let store = myelin_git::durable::DurableGitStore::rooted(root);
    let repo = store
        .create_repo(&myelin_git::core::RepoLoc::new(tenant, region, "web"))
        .expect("create authoritative repo");
    let git = git2::Repository::open_bare(repo.path()).unwrap();
    let config_blob = git.blob(CI_TOML).unwrap();
    let mut myelin = git.treebuilder(None).unwrap();
    myelin.insert("ci.toml", config_blob, 0o100644).unwrap();
    let myelin_tree = myelin.write().unwrap();
    let mut root_tree = git.treebuilder(None).unwrap();
    root_tree.insert(".myelin", myelin_tree, 0o040000).unwrap();
    let root_tree = root_tree.write().unwrap();
    let tree = git.find_tree(root_tree).unwrap();
    let signature = git2::Signature::now("ci", "ci@invalid").unwrap();
    git.commit(None, &signature, &signature, "seed CI", &tree, &[])
        .unwrap()
        .to_string()
}

fn push_event(tenant: &str, commit: &str, event_id: &str) -> EventEnvelope {
    let tenant = TenantId(tenant.into());
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("git.ref.updated".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("production-transport-proof".into()),
            PrincipalKind::Service,
            tenant.clone(),
        )),
        subject: ArtifactRef(format!(
            "myelin://{tenant}/git/ref/web:refs/heads/main",
            tenant = tenant.0
        )),
        aggregate: AggregateKey("web:refs/heads/main".into()),
        causation_id: None,
        correlation_id: CorrelationId(event_id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-19T00:00:00Z".into()),
        payload: serde_json::json!({
            "repo": "web", "ref": "refs/heads/main", "new_oid": commit,
            "old_oid": "0000000000000000000000000000000000000000", "forced": false
        }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_transport_runs_once_and_never_claims_foreign_outbox_rows() {
    let cfg = MyelinConfig::dev();
    let nonce = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_millis()
    );
    let schema = format!("ci_dispatch_transport_{nonce}");
    let tenant = format!("transport-{nonce}");
    let event_id = format!("transport-event-{nonce}");
    let second_event_id = format!("transport-event-second-{nonce}");
    let stream_name = format!("MYELIN_CI_DISPATCH_PROOF_{nonce}");
    let subject_root = format!("myelin.ci_dispatch_proof_{nonce}");
    let durable_name = format!("ci-dispatch-proof-{nonce}");
    assert_eq!(
        git_intake_filter(),
        format!("{EVENT_SUBJECT_ROOT}.evt.*.git.>"),
        "production intake derives its Git filter from the one production root"
    );
    let pool = isolated_pool(&schema).await;
    let rt = tokio::runtime::Handle::current();

    let git_root = std::env::temp_dir().join(format!("myelin-ci-production-transport-{nonce}"));
    std::fs::create_dir_all(&git_root).unwrap();
    let commit = seed_git(&git_root, &tenant, "fr-par");
    let git_root = AuthoritativeGitRoot::validate(&git_root).unwrap();

    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(pool.clone(), rt.clone())));
    let reserve_outbox =
        OutboxStore::durable(Arc::new(PgOutboxBacking::new(pool.clone(), rt.clone())));
    let dedup = DedupLedger::durable(
        Arc::new(DurableDedupBacking::new(pool.clone(), rt.clone())) as Arc<dyn DurableDedup>
    );
    let dead_letters: Arc<dyn myelin_events::DurableDeadLetter> =
        Arc::new(DurableDeadLetterBacking::new(pool.clone(), rt.clone()));
    let consumers = build_dispatch_consumers(
        git_root,
        &cfg.s3,
        ci_run_store_factory(pool.clone()),
        reserve_outbox,
        dedup,
        dead_letters,
        "fr-par",
        Arc::new(UlidMinter::new()),
        rt.clone(),
    )
    .unwrap();

    let envelope = push_event(&tenant, &commit, &event_id);
    let second_envelope = push_event(&tenant, &commit, &second_event_id);
    sqlx::query(
        "INSERT INTO outbox(event_id, aggregate, seq, subject, envelope) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind("foreign-outbox-row")
    .bind("foreign/aggregate")
    .bind(1_i64)
    .bind("myelin://foreign/issues/ONE")
    .bind(serde_json::to_value(push_event(&tenant, &commit, "foreign-envelope")).unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let publisher = NatsJetStreamPublisher::connect(
        JetStreamPublisherConfig {
            nats_url: cfg.nats_url.clone(),
            stream_name: stream_name.clone(),
            subject_root: subject_root.clone(),
            max_age: std::time::Duration::from_secs(24 * 60 * 60),
            max_bytes: 64 * 1024 * 1024,
            max_messages: 100_000,
            replicas: 1,
            duplicate_window: std::time::Duration::from_secs(120),
        },
        rt.clone(),
    )
    .unwrap();
    let intake_config = JetStreamConsumerConfig::bounded(
        &cfg.nats_url,
        &stream_name,
        &subject_root,
        format!("{subject_root}.evt.*.git.>"),
        &durable_name,
    );
    let intake = NatsJetStreamBus::connect_consumer(intake_config.clone(), rt.clone()).unwrap();
    let quarantine = Arc::new(
        myelin_storage::events_durable::DurableDeliveryQuarantineBacking::new(
            pool.clone(),
            rt.clone(),
        ),
    );
    let spec = dispatch_app_spec_with_intake(
        Config::default(), outbox, consumers, Box::new(intake), quarantine.clone(),
    );
    let handle = boot(spec).unwrap();
    let raw_client = async_nats::connect(&cfg.nats_url).await.unwrap();
    let raw_js = async_nats::jetstream::new(raw_client);
    raw_js
        .publish(
            format!("{subject_root}.evt.{tenant}.git.poison.malformed"),
            Vec::from(&b"ATTACKER_PII_SENTINEL malformed envelope"[..]).into(),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    publisher.publish(&envelope.subject, &envelope, &envelope.event_id).unwrap();
    raw_js
        .publish(
            format!("{subject_root}.evt.{tenant}.git.poison.route_mismatch"),
            serde_json::to_vec(&envelope).unwrap().into(),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    publisher
        .publish(&second_envelope.subject, &second_envelope, &second_envelope.event_id)
        .unwrap();
    handle.tick();

    let run_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM ci_run WHERE cause_event_id IN ($1, $2)",
    )
            .bind(&event_id)
            .bind(&second_event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(run_count, 2, "both valid siblings create their independent ci_run");
    let row = sqlx::query("SELECT definition_snapshot FROM ci_run WHERE cause_event_id=$1")
        .bind(&event_id)
        .fetch_one(&pool)
        .await
        .expect("one durable ci_run");
    let snapshot_ref: String = row.get("definition_snapshot");
    let address = ContentHash::parse(snapshot_ref.rsplit('/').next().unwrap()).unwrap();
    let s3 = S3BlobStore::connect(&cfg.s3, rt.clone());
    let bytes =
        tokio::task::block_in_place(|| s3.get(&TenantId(tenant.clone()), &address)).unwrap();
    assert_eq!(ContentHash::blake3(&bytes), address);
    let dedup_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM consumer_dedup \
         WHERE consumer=$1 AND event_id IN ($2, $3)",
    )
    .bind(myelin_ci_dispatch::TRIGGER_CONSUMER)
    .bind(&event_id)
    .bind(&second_event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(dedup_count, 2);
    let quarantine_reasons: Vec<String> = sqlx::query_scalar(
        "SELECT reason_code FROM consumer_delivery_quarantine \
         WHERE consumer=$1 ORDER BY stream_sequence",
    )
    .bind(&durable_name)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        quarantine_reasons,
        vec!["malformed_envelope".to_string(), "subject_mismatch".to_string()]
    );
    let quarantine_json: Vec<String> = sqlx::query_scalar(
        "SELECT row_to_json(q)::text FROM consumer_delivery_quarantine q WHERE consumer=$1",
    )
    .bind(&durable_name)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(quarantine_json.iter().all(|row| !row.contains("ATTACKER_PII_SENTINEL")));
    let (quarantine_stream, quarantine_sequence, quarantine_attempt): (String, i64, i64) =
        sqlx::query_as(
            "SELECT stream, stream_sequence, delivery_attempt \
             FROM consumer_delivery_quarantine WHERE consumer=$1 \
             ORDER BY stream_sequence LIMIT 1",
        )
        .bind(&durable_name)
        .fetch_one(&pool)
        .await
        .unwrap();
    quarantine
        .record(
            &durable_name,
            &BrokerDeliveryRef {
                stream: quarantine_stream,
                stream_sequence: u64::try_from(quarantine_sequence).unwrap(),
            },
            DeliveryQuarantineReason::MalformedEnvelope,
            u64::try_from(quarantine_attempt).unwrap(),
        )
        .unwrap();
    let quarantine_count: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM consumer_delivery_quarantine WHERE consumer=$1",
    )
    .bind(&durable_name)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(quarantine_count, 2, "re-record is idempotent on broker reference");
    handle.tick();
    let after_redelivery: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM ci_run WHERE cause_event_id IN ($1, $2)",
    )
            .bind(&event_id)
            .bind(&second_event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_redelivery, 2,
        "a second tick cannot duplicate either valid run"
    );
    let foreign_untouched: bool = sqlx::query_scalar(
        "SELECT published_at IS NULL AND attempts=0 FROM outbox WHERE event_id='foreign-outbox-row'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(foreign_untouched, "dispatch never claims the shared outbox");

    handle.signal_drain();
    // `drain(self)` consumes and drops the original handle/boxed transport. Dropping the returned
    // telemetry makes that ownership boundary explicit before a fresh client rebinds the durable.
    drop(handle.drain());
    let rebound = NatsJetStreamBus::connect_consumer(intake_config, rt.clone()).unwrap();
    assert!(
        rebound.consume("").unwrap().is_empty(),
        "broker ack survives rebind"
    );

    tokio::task::block_in_place(|| s3.delete(&TenantId(tenant), &address)).ok();
    let client = async_nats::connect(&cfg.nats_url).await.unwrap();
    async_nats::jetstream::new(client)
        .delete_stream(&stream_name)
        .await
        .ok();
    pool.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .ok();
    std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("myelin-ci-production-transport-{nonce}")),
    )
    .ok();
}
