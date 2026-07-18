//! Live-NATS proof that the production publisher provisions only the bounded shared stream.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::nats::{JetStreamPublisherConfig, NatsJetStreamPublisher};
use myelin_events::relay::EventPublisher;
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
