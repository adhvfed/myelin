#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;

#[tokio::test]
async fn nats_jetstream_publish_and_dedup() {
    let cfg = MyelinConfig::dev();
    let client = async_nats::connect(&cfg.nats_url)
        .await
        .expect("connect to dev NATS (is the stack up?)");
    let js = async_nats::jetstream::new(client);

    let suffix = std::process::id();
    let stream_name = format!("MYELIN_STAGE1_{suffix}");
    let subject = format!("myelin.stage1.{suffix}");

    let stream = js
        .create_stream(async_nats::jetstream::stream::Config {
            name: stream_name.clone(),
            subjects: vec![subject.clone()],
            ..Default::default()
        })
        .await
        .expect("create JetStream stream (is JetStream enabled with -js?)");

    let dedup_id = format!("event-{suffix}");

    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Nats-Msg-Id", dedup_id.as_str());
    let ack1 = js
        .publish_with_headers(subject.clone(), headers.clone(), "payload".into())
        .await
        .expect("publish 1")
        .await
        .expect("ack 1");

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

    let info = stream.get_info().await.expect("stream info");
    assert_eq!(
        info.state.messages, 1,
        "exactly one message must be stored (dedup suppressed the second)"
    );

    js.delete_stream(&stream_name).await.expect("delete stream");
}
