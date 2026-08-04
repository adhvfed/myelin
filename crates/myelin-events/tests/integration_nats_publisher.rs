#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::nats::{
    JetStreamConsumerConfig, JetStreamProvisioner, JetStreamPublisherConfig, NatsJetStreamBus,
    NatsJetStreamPublisher,
};
use myelin_events::relay::{EventConsumer, EventPublisher};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, BrokerDelivery, BrokerDeliveryBody, CausedBy, CorrelationId,
    DataRole, EventEnvelope, EventId, EventType, Timestamp, Visibility,
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

fn delivered_event(delivery: &BrokerDelivery) -> &EventEnvelope {
    match &delivery.body {
        BrokerDeliveryBody::Event(event) => event,
        other => panic!("expected event delivery, got {other:?}"),
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
        publish_ack_timeout: std::time::Duration::from_secs(2),
    };
    JetStreamProvisioner::ensure(config.clone(), tokio::runtime::Handle::current())
        .expect("provision publisher-only stream");
    let publisher =
        NatsJetStreamPublisher::connect_existing(config, tokio::runtime::Handle::current())
            .expect("connect runtime publisher to existing stream");
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

    js.delete_stream(&stream_name)
        .await
        .expect("delete test stream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_publisher_never_provisions_a_missing_stream() {
    let dev = MyelinConfig::dev();
    let suffix = format!("{}_runtime_only", std::process::id());
    let stream_name = format!("MYELIN_PUBLISHER_{suffix}");
    let config = JetStreamPublisherConfig {
        nats_url: dev.nats_url.clone(),
        stream_name: stream_name.clone(),
        subject_root: format!("myelin.publisher_{suffix}"),
        max_age: std::time::Duration::from_secs(3600),
        max_bytes: 4 * 1024 * 1024,
        max_messages: 1000,
        replicas: 1,
        duplicate_window: std::time::Duration::from_secs(60),
        publish_ack_timeout: std::time::Duration::from_millis(250),
    };
    let publisher =
        NatsJetStreamPublisher::connect_existing(config, tokio::runtime::Handle::current())
            .expect("runtime connection requires no admin request");
    let event = envelope(&format!("runtime-only-{suffix}"));
    publisher
        .publish(&event.subject, &event, &event.event_id)
        .expect_err("a missing stream cannot acknowledge publication");

    let client = async_nats::connect(&dev.nats_url)
        .await
        .expect("inspect NATS");
    let js = async_nats::jetstream::new(client);
    assert!(
        js.get_stream(&stream_name).await.is_err(),
        "runtime publish authority must not create the absent stream"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioner_refuses_existing_stream_policy_drift() {
    let dev = MyelinConfig::dev();
    let suffix = format!("{}_stream_drift", std::process::id());
    let stream_name = format!("MYELIN_PUBLISHER_{suffix}");
    let subject_root = format!("myelin.publisher_{suffix}");
    let client = async_nats::connect(&dev.nats_url)
        .await
        .expect("connect NATS admin");
    let js = async_nats::jetstream::new(client);
    js.create_stream(async_nats::jetstream::stream::Config {
        name: stream_name.clone(),
        subjects: vec![format!("{subject_root}.>")],
        max_age: std::time::Duration::from_secs(3600),
        max_bytes: 1024,
        max_messages: 1000,
        num_replicas: 1,
        duplicate_window: std::time::Duration::from_secs(60),
        storage: async_nats::jetstream::stream::StorageType::File,
        retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
        discard: async_nats::jetstream::stream::DiscardPolicy::Old,
        ..Default::default()
    })
    .await
    .expect("create drifted stream");
    let error = JetStreamProvisioner::ensure(
        JetStreamPublisherConfig {
            nats_url: dev.nats_url.clone(),
            stream_name: stream_name.clone(),
            subject_root,
            max_age: std::time::Duration::from_secs(3600),
            max_bytes: 4 * 1024 * 1024,
            max_messages: 1000,
            replicas: 1,
            duplicate_window: std::time::Duration::from_secs(60),
            publish_ack_timeout: std::time::Duration::from_secs(2),
        },
        tokio::runtime::Handle::current(),
    )
    .expect_err("drifted stream must be refused");
    assert_eq!(
        error.0,
        "existing JetStream stream configuration is incompatible"
    );
    js.delete_stream(&stream_name)
        .await
        .expect("delete drift test stream");
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
            publish_ack_timeout: std::time::Duration::from_secs(2),
        },
        tokio::runtime::Handle::current(),
    )
    .expect("provision publisher-only stream");

    let mut event = envelope(&format!("01JNR{suffix:0>21}"));
    event.type_ = EventType("git.ref.updated".into());
    event.subject = ArtifactRef("myelin://publisher-test/git/ref/web:refs%2Fheads%2Fmain".into());
    event.aggregate = AggregateKey("ref:web:refs%2Fheads%2Fmain".into());
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
    assert_eq!(delivered_event(&initial[0]).event_id, event.event_id);
    drop(first);
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;

    let rebound = NatsJetStreamBus::connect_consumer(
        consumer_config.clone(),
        tokio::runtime::Handle::current(),
    )
    .expect("rebind exact durable name after restart");
    let redelivered = rebound.consume("").expect("redelivery after restart");
    assert_eq!(
        redelivered.len(),
        1,
        "unacked event survives consumer restart"
    );
    assert_eq!(delivered_event(&redelivered[0]).event_id, event.event_id);
    assert!(redelivered[0].delivery_attempt >= Some(2));
    rebound
        .ack(redelivered[0].token)
        .expect("persist explicit ack");
    drop(rebound);
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let after_ack =
        NatsJetStreamBus::connect_consumer(consumer_config, tokio::runtime::Handle::current())
            .expect("rebind after acknowledged restart");
    assert!(after_ack.consume("").expect("pull after ack").is_empty());

    let client = async_nats::connect(&dev.nats_url)
        .await
        .expect("cleanup NATS");
    async_nats::jetstream::new(client)
        .delete_stream(&stream_name)
        .await
        .expect("delete test stream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_durable_consumer_with_semantic_drift_refuses_boot() {
    let dev = MyelinConfig::dev();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let suffix = format!("{}_drift_{nonce}", std::process::id());
    let stream_name = format!("MYELIN_CONSUMER_{suffix}");
    let subject_root = format!("myelin.consumer_{suffix}");
    let durable_name = format!("ci-dispatch-{suffix}");
    NatsJetStreamPublisher::connect(
        JetStreamPublisherConfig {
            nats_url: dev.nats_url.clone(),
            stream_name: stream_name.clone(),
            subject_root: subject_root.clone(),
            max_age: std::time::Duration::from_secs(24 * 60 * 60),
            max_bytes: 16 * 1024 * 1024,
            max_messages: 10_000,
            replicas: 1,
            duplicate_window: std::time::Duration::from_secs(120),
            publish_ack_timeout: std::time::Duration::from_secs(2),
        },
        tokio::runtime::Handle::current(),
    )
    .expect("provision publisher-only stream");

    let client = async_nats::connect(&dev.nats_url)
        .await
        .expect("connect raw NATS client");
    let js = async_nats::jetstream::new(client);
    let stream = js.get_stream(&stream_name).await.expect("get stream");
    stream
        .create_consumer(async_nats::jetstream::consumer::pull::Config {
            durable_name: Some(durable_name.clone()),
            name: Some(durable_name.clone()),
            deliver_policy: async_nats::jetstream::consumer::DeliverPolicy::All,
            ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
            ack_wait: std::time::Duration::from_secs(30),
            max_deliver: -1,
            filter_subject: format!("{subject_root}.evt.*.git.>"),
            replay_policy: async_nats::jetstream::consumer::ReplayPolicy::Instant,
            max_waiting: 9,
            max_ack_pending: 256,
            max_batch: 256,
            max_bytes: 4 * 1024 * 1024,
            max_expires: std::time::Duration::from_secs(1),
            ..Default::default()
        })
        .await
        .expect("create intentionally drifted durable consumer");

    let result = NatsJetStreamBus::connect_consumer(
        JetStreamConsumerConfig::bounded(
            &dev.nats_url,
            &stream_name,
            &subject_root,
            format!("{subject_root}.evt.*.git.>"),
            &durable_name,
        ),
        tokio::runtime::Handle::current(),
    );
    let error = match result {
        Ok(_) => panic!("consumer drift must refuse boot"),
        Err(error) => error,
    };
    assert!(
        error.0.contains("configuration drifted"),
        "unexpected refusal: {error:?}"
    );
    assert!(
        error.0.contains("max_waiting"),
        "drift field must be named: {error:?}"
    );

    js.delete_stream(&stream_name)
        .await
        .expect("delete drift test stream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nak_increments_attempt_then_term_makes_durable_rebind_empty() {
    let dev = MyelinConfig::dev();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let suffix = format!("{}_settle_{nonce}", std::process::id());
    let stream_name = format!("MYELIN_SETTLE_{suffix}");
    let subject_root = format!("myelin.settle_{suffix}");
    let durable_name = format!("settle-{suffix}");
    let publisher = NatsJetStreamPublisher::connect(
        JetStreamPublisherConfig {
            nats_url: dev.nats_url.clone(),
            stream_name: stream_name.clone(),
            subject_root: subject_root.clone(),
            max_age: std::time::Duration::from_secs(3600),
            max_bytes: 4 * 1024 * 1024,
            max_messages: 1000,
            replicas: 1,
            duplicate_window: std::time::Duration::from_secs(60),
            publish_ack_timeout: std::time::Duration::from_secs(2),
        },
        tokio::runtime::Handle::current(),
    )
    .expect("create nonce-scoped settle stream");
    let event = envelope(&format!("settle-event-{nonce}"));
    publisher
        .publish(&event.subject, &event, &event.event_id)
        .unwrap();
    let config = JetStreamConsumerConfig::bounded(
        &dev.nats_url,
        &stream_name,
        &subject_root,
        format!("{subject_root}.>"),
        &durable_name,
    );
    let consumer =
        NatsJetStreamBus::connect_consumer(config.clone(), tokio::runtime::Handle::current())
            .unwrap();
    let first = consumer.consume("").unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].delivery_attempt, Some(1));
    consumer.retry(first[0].token, 1).unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let second = loop {
        let batch = consumer.consume("").unwrap();
        if !batch.is_empty() {
            break batch;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "NAK redelivery deadline exceeded"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(second.len(), 1);
    assert!(second[0].delivery_attempt >= Some(2));
    consumer.terminate(second[0].token).unwrap();
    drop(consumer);

    let rebound =
        NatsJetStreamBus::connect_consumer(config, tokio::runtime::Handle::current()).unwrap();
    assert!(
        rebound.consume("").unwrap().is_empty(),
        "TERM survives durable rebind"
    );
    drop(rebound);
    let client = async_nats::connect(&dev.nats_url).await.unwrap();
    async_nats::jetstream::new(client)
        .delete_stream(&stream_name)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "run seed, restart NATS, then run verify"]
async fn jetstream_file_storage_survives_server_restart() {
    let dev = MyelinConfig::dev();
    let stream_name = "MYELIN_SERVER_RESTART_PROOF";
    let subject_root = "myelin.server_restart_proof";
    let durable_name = "server-restart-proof";
    let phase = std::env::var("MYELIN_NATS_RESTART_PHASE").expect("seed or verify phase");

    if phase == "seed" {
        let client = async_nats::connect(&dev.nats_url)
            .await
            .expect("connect seed cleanup");
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
                publish_ack_timeout: std::time::Duration::from_secs(2),
            },
            tokio::runtime::Handle::current(),
        )
        .expect("create file-backed stream");
        let mut event = envelope("server-restart-event");
        event.type_ = EventType("git.ref.updated".into());
        event.subject =
            ArtifactRef("myelin://publisher-test/git/ref/web:refs%2Fheads%2Fmain".into());
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
    assert_eq!(
        delivered_event(&delivery[0]).event_id.0,
        "server-restart-event"
    );
    consumer
        .ack(delivery[0].token)
        .expect("ack persisted event");

    let client = async_nats::connect(&dev.nats_url)
        .await
        .expect("connect cleanup");
    async_nats::jetstream::new(client)
        .delete_stream(stream_name)
        .await
        .expect("delete proof stream");
}
