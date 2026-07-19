//! Live-NATS proof that the production publisher provisions only the bounded shared stream.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::nats::{
    JetStreamConsumerConfig, JetStreamPublisherConfig, NatsJetStreamBus, NatsJetStreamPublisher,
};
use myelin_events::relay::{EventConsumer, EventPublisher};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn envelope(id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId("publisher-test".into()),
        region: Region("no-osl".into()),
        actor: Actor(Principal::stub(
            PrincipalId("publisher-test".into()),
            PrincipalKind::Service,
            TenantId("publisher-test".into()),
        )),
        subject: ArtifactRef("myelin://publisher-test/issues/issue/ONE".into()),
        aggregate: AggregateKey("issue:ONE".into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: Some(CausedBy("integration-test".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-18T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-18T00:00:00Z".into()),
        payload: serde_json::json!({ "issue": "ONE" }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publisher_provisions_bounded_stream_without_consumer_or_purge_capability() {
    let dev = MyelinConfig::dev();
    let suffix = std::process::id();
    let stream_name = format!("MYELIN_PUBLISHER_{suffix}");
    let config = JetStreamPublisherConfig {
        nats_url: dev.nats_url.clone(),
        stream_name: stream_name.clone(),
        subject_root: format!("myelin.publisher_{suffix}"),
        max_age: std::time::Duration::from_secs(24 * 60 * 60),
        max_bytes: 16 * 1024 * 1024,
        max_messages: 10_000,
        replicas: 1,
        duplicate_window: std::time::Duration::from_secs(120),
    };
    let publisher = NatsJetStreamPublisher::connect(config, tokio::runtime::Handle::current())
        .expect("provision publisher-only stream");
    let event = envelope(&format!("01JNP{suffix:021}"));
    publisher
        .publish(&event.subject, &event, &event.event_id)
        .expect("publish through narrow seam");

    let client = async_nats::connect(&dev.nats_url)
        .await
        .expect("inspect NATS");
    let js = async_nats::jetstream::new(client);
    let mut stream = js.get_stream(&stream_name).await.expect("get stream");
    let info = stream.info().await.expect("stream info");
    assert_eq!(info.state.messages, 1);
    assert_eq!(
        info.state.consumer_count, 0,
        "publisher creates no durable consumer"
    );

    // Test infrastructure owns this uniquely named stream; production publisher API has no purge
    // or delete operation, so cleanup is deliberately performed through the raw test client.
    js.delete_stream(&stream_name)
        .await
        .expect("delete test stream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_pull_rebind_redelivers_unacked_git_event_then_persists_ack() {
    let dev = MyelinConfig::dev();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis();
    let suffix = format!("{}-{nonce}-restart", std::process::id());
    let stream_name = format!("MYELIN_PUBLISHER_{}", suffix.replace('-', "_"));
    let subject_root = format!("myelin.publisher_{}", suffix.replace('-', "_"));
    let durable_name = format!("ci-dispatch-{suffix}");
    let publisher = NatsJetStreamPublisher::connect(
        JetStreamPublisherConfig {
            nats_url: dev.nats_url.clone(),
            stream_name: stream_name.clone(),
            subject_root: subject_root.clone(),
            max_age: std::time::Duration::from_secs(24 * 60 * 60),
            max_bytes: 16 * 1024 * 1024,
            max_messages: 10_000,
            replicas: 1,
            duplicate_window: std::time::Duration::from_secs(120),
        },
        tokio::runtime::Handle::current(),
    )
    .expect("provision publisher-only stream");

    let mut event = envelope(&format!("01JNR{suffix:0>21}"));
    event.type_ = EventType("git.ref.updated".into());
    event.subject = ArtifactRef("myelin://publisher-test/git/ref/web:refs/heads/main".into());
    event.aggregate = AggregateKey("git/ref/web:refs/heads/main".into());
    publisher
        .publish(&event.subject, &event, &event.event_id)
        .expect("publish git event");

    let mut consumer_config = JetStreamConsumerConfig::bounded(
        &dev.nats_url,
        &stream_name,
        &subject_root,
        format!("{subject_root}.evt.*.git.>"),
        &durable_name,
    );
    consumer_config.ack_wait = std::time::Duration::from_secs(1);
    let first = NatsJetStreamBus::connect_consumer(
        consumer_config.clone(),
        tokio::runtime::Handle::current(),
    )
    .expect("bind durable pull consumer");
    let initial = first.consume("").expect("initial durable pull");
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].envelope.event_id, event.event_id);
    drop(first);
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;

    let rebound = NatsJetStreamBus::connect_consumer(
        consumer_config.clone(),
        tokio::runtime::Handle::current(),
    )
    .expect("rebind exact durable name after restart");
    let redelivered = rebound.consume("").expect("redelivery after restart");
    assert_eq!(redelivered.len(), 1, "unacked event survives consumer restart");
    assert_eq!(redelivered[0].envelope.event_id, event.event_id);
    assert!(redelivered[0].delivery_attempt >= 2);
    rebound
        .ack(&durable_name, &event.event_id)
        .expect("persist explicit ack");
    drop(rebound);
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let after_ack = NatsJetStreamBus::connect_consumer(
        consumer_config,
        tokio::runtime::Handle::current(),
    )
    .expect("rebind after acknowledged restart");
    assert!(after_ack.consume("").expect("pull after ack").is_empty());

    let client = async_nats::connect(&dev.nats_url).await.expect("cleanup NATS");
    async_nats::jetstream::new(client)
        .delete_stream(&stream_name)
        .await
        .expect("delete test stream");
}

/// Two-phase operator proof for the server/volume boundary. Run once with phase `seed`, restart the
/// NATS service, then run with phase `verify`. It is ignored in ordinary suites because a test
/// process must not restart shared infrastructure behind other concurrent tests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "run seed, restart NATS, then run verify"]
async fn jetstream_file_storage_survives_server_restart() {
    let dev = MyelinConfig::dev();
    let stream_name = "MYELIN_SERVER_RESTART_PROOF";
    let subject_root = "myelin.server_restart_proof";
    let durable_name = "server-restart-proof";
    let phase = std::env::var("MYELIN_NATS_RESTART_PHASE").expect("seed or verify phase");

    if phase == "seed" {
        let client = async_nats::connect(&dev.nats_url).await.expect("connect seed cleanup");
        let js = async_nats::jetstream::new(client);
        let _ = js.delete_stream(stream_name).await;
        let publisher = NatsJetStreamPublisher::connect(
            JetStreamPublisherConfig {
                nats_url: dev.nats_url.clone(),
                stream_name: stream_name.into(),
                subject_root: subject_root.into(),
                max_age: std::time::Duration::from_secs(24 * 60 * 60),
                max_bytes: 16 * 1024 * 1024,
                max_messages: 10_000,
                replicas: 1,
                duplicate_window: std::time::Duration::from_secs(120),
            },
            tokio::runtime::Handle::current(),
        )
        .expect("create file-backed stream");
        let mut event = envelope("server-restart-event");
        event.type_ = EventType("git.ref.updated".into());
        event.subject = ArtifactRef("myelin://publisher-test/git/ref/web:refs/heads/main".into());
        publisher
            .publish(&event.subject, &event, &event.event_id)
            .expect("persist event before server restart");
        return;
    }

    assert_eq!(phase, "verify", "phase must be seed or verify");
    let consumer = NatsJetStreamBus::connect_consumer(
        JetStreamConsumerConfig::bounded(
            &dev.nats_url,
            stream_name,
            subject_root,
            format!("{subject_root}.evt.*.git.>"),
            durable_name,
        ),
        tokio::runtime::Handle::current(),
    )
    .expect("stream survived server restart");
    let delivery = consumer.consume("").expect("pull persisted event");
    assert_eq!(delivery.len(), 1);
    assert_eq!(delivery[0].envelope.event_id.0, "server-restart-event");
    consumer
        .ack(durable_name, &delivery[0].envelope.event_id)
        .expect("ack persisted event");

    let client = async_nats::connect(&dev.nats_url).await.expect("connect cleanup");
    async_nats::jetstream::new(client)
        .delete_stream(stream_name)
        .await
        .expect("delete proof stream");
}
