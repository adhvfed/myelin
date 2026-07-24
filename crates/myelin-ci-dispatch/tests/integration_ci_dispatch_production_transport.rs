//! Live proof of the complete production dispatch composition over PG + NATS + Git + S3.
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_ci_controlplane::{ci_durable_migrations, ci_run_store_factory};
use myelin_ci_dispatch::{
    build_dispatch_consumers, dispatch_app_spec_with_intake, git_intake_filter,
    AuthoritativeGitRoot, RecoveringIntake, EVENT_SUBJECT_ROOT,
};
use myelin_config::MyelinConfig;
use myelin_events::nats::{
    JetStreamConsumerConfig, JetStreamProvisioner, JetStreamPublisherConfig, NatsJetStreamBus,
    NatsJetStreamPublisher,
};
use myelin_events::relay::EventConsumer;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, BrokerDeliveryRef, CorrelationId, DataRole, DedupLedger,
    DeliveryQuarantineReason, DurableDedup, DurableDeliveryQuarantine, EventEnvelope, EventId,
    EventType, OutboxStore, Timestamp, UlidMinter, Visibility, CONSUMER_DEAD_LETTER_MIGRATION,
    CONSUMER_DEDUP_MIGRATION, CONSUMER_DELIVERY_QUARANTINE_MIGRATION, OUTBOX_MIGRATION,
    OUTBOX_QUARANTINE_MIGRATION,
};
use myelin_git::check_status::{CheckContext, CheckState, CheckStatusConsumer, GitOid};
use myelin_git::check_status_store::PgCheckStatusProjection;
use myelin_git::merge_gate::{MergeGateOutcome, MergeGatePolicy};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::elected_relay::{ElectedDrainOutcome, ElectedPgRelay};
use myelin_storage::events_durable::{DurableDeadLetterBacking, DurableDedupBacking};
use myelin_storage::outbox_durable::PgOutboxBacking;
use myelin_storage::pgrelay::{PgRelay, RelayValidationConfig};
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::{BlobStore, ContentHash};
use myelin_substrate::{boot, Config};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};

const CI_TOML: &[u8] = br#"on = "pull_request"

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
        OUTBOX_QUARANTINE_MIGRATION,
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

fn trigger_event(tenant: &str, commit: &str, event_id: &str) -> EventEnvelope {
    let tenant = TenantId(tenant.into());
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("git.pr.opened".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("production-transport-proof".into()),
            PrincipalKind::Service,
            tenant.clone(),
        )),
        subject: ArtifactRef(format!("myelin://{}/git/pr/web:42", tenant.0)),
        aggregate: AggregateKey("git/pr/web:42".into()),
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
            "repo": "web", "number": 42, "head_oid": commit, "is_fork": false
        }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn elected_publisher_delivers_trigger_then_dispatch_leaves_new_rows_for_next_pass() {
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
    let dedup = DedupLedger::durable(
        Arc::new(DurableDedupBacking::new(pool.clone(), rt.clone())) as Arc<dyn DurableDedup>
    );
    let dead_letters: Arc<dyn myelin_events::DurableDeadLetter> =
        Arc::new(DurableDeadLetterBacking::new(pool.clone(), rt.clone()));
    let s3 = Arc::new(S3BlobStore::connect(&cfg.s3, rt.clone()));
    s3.preflight().expect("S3 read/write authority preflight");
    let consumers = build_dispatch_consumers(
        git_root,
        s3.clone(),
        ci_run_store_factory(pool.clone()),
        dedup,
        dead_letters,
        "fr-par",
        Arc::new(UlidMinter::new()),
        rt.clone(),
    )
    .unwrap();

    let envelope = trigger_event(&tenant, &commit, &event_id);
    let second_envelope = trigger_event(&tenant, &commit, &second_event_id);
    sqlx::query(
        "INSERT INTO outbox(event_id, aggregate, seq, subject, envelope) VALUES ($1,$2,$3,$4,$5)",
    )
    .bind("foreign-outbox-row")
    .bind("foreign/aggregate")
    .bind(1_i64)
    .bind("myelin://foreign/issues/ONE")
    .bind(serde_json::to_value(trigger_event(&tenant, &commit, "foreign-envelope")).unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let publisher_config = JetStreamPublisherConfig {
        nats_url: cfg.nats_url.clone(),
        stream_name: stream_name.clone(),
        subject_root: subject_root.clone(),
        max_age: std::time::Duration::from_secs(24 * 60 * 60),
        max_bytes: 64 * 1024 * 1024,
        max_messages: 100_000,
        replicas: 1,
        duplicate_window: std::time::Duration::from_secs(120),
        publish_ack_timeout: std::time::Duration::from_secs(2),
    };
    JetStreamProvisioner::ensure(publisher_config.clone(), rt.clone()).unwrap();
    let publisher = NatsJetStreamPublisher::connect_existing(publisher_config, rt.clone()).unwrap();
    let elected = ElectedPgRelay::new(
        pool.clone(),
        RelayValidationConfig::new(Region("fr-par".into()), 256 * 1024).unwrap(),
    )
    .unwrap();
    let intake_config = JetStreamConsumerConfig::bounded(
        &cfg.nats_url,
        &stream_name,
        &subject_root,
        format!("{subject_root}.evt.*.git.>"),
        &durable_name,
    );
    let intake = RecoveringIntake::new(intake_config.clone(), s3.clone(), rt.clone());
    let quarantine = Arc::new(
        myelin_storage::events_durable::DurableDeliveryQuarantineBacking::new(
            pool.clone(),
            rt.clone(),
        ),
    );
    let spec = dispatch_app_spec_with_intake(
        Config::default(),
        outbox,
        consumers,
        Box::new(intake),
        quarantine.clone(),
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
    let relay_store = PgRelay::new(pool.clone());
    relay_store
        .enqueue(&envelope.aggregate.0, 0, &envelope)
        .await
        .unwrap();
    raw_js
        .publish(
            format!("{subject_root}.evt.{tenant}.git.poison.route_mismatch"),
            serde_json::to_vec(&envelope).unwrap().into(),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    relay_store
        .enqueue(&second_envelope.aggregate.0, 1, &second_envelope)
        .await
        .unwrap();
    assert_eq!(
        elected.drain_once(&publisher, 64).await.unwrap(),
        ElectedDrainOutcome::Published(2),
        "the elected relay publishes both committed trigger rows and quarantines foreign poison"
    );
    handle.tick();

    let source_rows_marked: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM outbox
          WHERE event_id IN ($1, $2) AND published_at IS NOT NULL",
    )
    .bind(&event_id)
    .bind(&second_event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source_rows_marked, 2);

    let run_count: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM ci_run WHERE cause_event_id IN ($1, $2)")
            .bind(&event_id)
            .bind(&second_event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        run_count, 2,
        "both valid siblings create their independent ci_run"
    );
    let queued_envelopes: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT envelope FROM outbox \
          WHERE envelope->>'type_'='ci.check.updated' \
            AND envelope->>'causation_id' IN ($1,$2) \
          ORDER BY (envelope->'payload'->>'run_attempt')::integer",
    )
    .bind(&event_id)
    .bind(&second_event_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    let queued_attempts = queued_envelopes
        .iter()
        .map(|envelope| envelope["payload"]["run_attempt"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        queued_attempts,
        vec![1, 2],
        "the production dispatch transaction allocates a new monotonic attempt before each queued fact"
    );

    // Regression for the audit's gate-green rerun window: make attempt 1 a settled success, prove
    // it admits, then consume the second production-dispatch queued fact. Its allocated attempt 2
    // must supersede immediately and block before PgPipelineStarter emits InProgress.
    let projection = PgCheckStatusProjection::connect(
        pool.clone(),
        "dispatch_check_status",
        "dispatch_check_dedup",
        "dispatch-check-proof",
    )
    .await
    .unwrap();
    let first_envelope: EventEnvelope =
        serde_json::from_value(queued_envelopes[0].clone()).unwrap();
    let second_envelope: EventEnvelope =
        serde_json::from_value(queued_envelopes[1].clone()).unwrap();
    let mut settled_success = CheckStatusConsumer::decode(&first_envelope.payload).unwrap();
    settled_success.state = CheckState::Success;
    settled_success.cost_settled = true;
    projection
        .apply("dispatch-terminal-attempt-1", "fr-par", &settled_success)
        .await
        .unwrap();
    let policy = MergeGatePolicy {
        required: vec![CheckContext::ci("build")],
    };
    assert_eq!(
        projection
            .merge_gate(
                &tenant,
                "fr-par",
                &settled_success.repo.0,
                &GitOid(commit.clone()),
                &policy,
                &[],
            )
            .await
            .unwrap(),
        MergeGateOutcome::Admitted
    );
    let queued_rerun = CheckStatusConsumer::decode(&second_envelope.payload).unwrap();
    projection
        .apply(&second_envelope.event_id.0, "fr-par", &queued_rerun)
        .await
        .unwrap();
    assert!(matches!(
        projection
            .merge_gate(
                &tenant,
                "fr-par",
                &queued_rerun.repo.0,
                &GitOid(commit.clone()),
                &policy,
                &[],
            )
            .await
            .unwrap(),
        MergeGateOutcome::Blocked { .. }
    ));
    let row = sqlx::query("SELECT definition_snapshot FROM ci_run WHERE cause_event_id=$1")
        .bind(&event_id)
        .fetch_one(&pool)
        .await
        .expect("one durable ci_run");
    let snapshot_ref: String = row.get("definition_snapshot");
    let address = ContentHash::parse(snapshot_ref.rsplit('/').next().unwrap()).unwrap();
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
    let newly_emitted: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM outbox
          WHERE published_at IS NULL
            AND event_id NOT IN ($1, $2, 'foreign-outbox-row')",
    )
    .bind(&event_id)
    .bind(&second_event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        newly_emitted > 0,
        "dispatch never embeds a relay; its emitted rows await election"
    );
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
        vec![
            "malformed_envelope".to_string(),
            "subject_mismatch".to_string()
        ]
    );
    let quarantine_json: Vec<String> = sqlx::query_scalar(
        "SELECT row_to_json(q)::text FROM consumer_delivery_quarantine q WHERE consumer=$1",
    )
    .bind(&durable_name)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(quarantine_json
        .iter()
        .all(|row| !row.contains("ATTACKER_PII_SENTINEL")));
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
    assert_eq!(
        quarantine_count, 2,
        "re-record is idempotent on broker reference"
    );
    handle.tick();
    let after_redelivery: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM ci_run WHERE cause_event_id IN ($1, $2)")
            .bind(&event_id)
            .bind(&second_event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after_redelivery, 2,
        "a second tick cannot duplicate either valid run"
    );
    let foreign_owned_by_shared_publisher: bool = sqlx::query_scalar(
        "SELECT o.published_at IS NULL AND o.attempts=0
                AND EXISTS (SELECT 1 FROM outbox_quarantine q WHERE q.event_id=o.event_id)
           FROM outbox o WHERE o.event_id='foreign-outbox-row'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        foreign_owned_by_shared_publisher,
        "dispatch never claims the shared outbox"
    );

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
