//! Live NATS JetStream integration test (Stage 1 / infra).
//!
//! Gated behind the `integration` cargo feature so the default build stays DB/broker-free.
//! Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-events --features integration --test integration_nats -- --nocapture
//!
//! Proves the durable bus (the BusTransport backing) is reachable through NATS_URL and that
//! JetStream is enabled: create a stream, publish with a Nats-Msg-Id dedup header (the stable
//! event_id → 0-ghost property the BusTransport contract names), and confirm a duplicate
//! publish is suppressed.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;

#[tokio::test]
async fn nats_jetstream_publish_and_dedup() {
    let cfg = MyelinConfig::dev();
    let client = async_nats::connect(&cfg.nats_url)
        .await
        .expect("connect to dev NATS (is the stack up?)");
    let js = async_nats::jetstream::new(client);

    // A unique stream/subject per run so concurrent runs don't collide.
    let suffix = std::process::id();
    let stream_name = format!("MYELIN_STAGE1_{suffix}");
    let subject = format!("myelin.stage1.{suffix}");

    // JetStream must be enabled (-js). Creating a durable stream proves it.
    let stream = js
        .create_stream(async_nats::jetstream::stream::Config {
            name: stream_name.clone(),
            subjects: vec![subject.clone()],
            ..Default::default()
        })
        .await
        .expect("create JetStream stream (is JetStream enabled with -js?)");

    let dedup_id = format!("event-{suffix}");

    // First publish with the stable dedup id header (Nats-Msg-Id = event_id).
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Nats-Msg-Id", dedup_id.as_str());
    let ack1 = js
        .publish_with_headers(subject.clone(), headers.clone(), "payload".into())
        .await
        .expect("publish 1")
        .await
        .expect("ack 1");

    // Second publish with the SAME dedup id — the broker suppresses it (0 ghost). The ack
    // carries the same sequence and the duplicate flag.
    let ack2 = js
        .publish_with_headers(subject.clone(), headers, "payload".into())
        .await
        .expect("publish 2")
        .await
        .expect("ack 2");

    assert_eq!(
        ack1.sequence, ack2.sequence,
        "duplicate publish must map to the same stream sequence (broker-side dedup → 0 ghost)"
    );
    assert!(ack2.duplicate, "second publish must be flagged a duplicate");

    // Exactly one message landed in the stream despite two publishes.
    let info = stream.get_info().await.expect("stream info");
    assert_eq!(
        info.state.messages, 1,
        "exactly one message must be stored (dedup suppressed the second)"
    );

    // Cleanup.
    js.delete_stream(&stream_name).await.expect("delete stream");
}
