#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::nats::{JetStreamConsumerConfig, NatsJetStreamBus};
use myelin_events::DurableWorkerAdmission;

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

#[tokio::test(flavor = "multi_thread")]
async fn durable_consumer_reconciles_changed_admission_limits_on_restart() {
    let cfg = MyelinConfig::dev();
    let client = async_nats::connect(&cfg.nats_url)
        .await
        .expect("connect to dev NATS (is the stack up?)");
    let js = async_nats::jetstream::new(client);

    let suffix = std::process::id();
    let stream_name = format!("MYELIN_ADMISSION_RECONCILE_{suffix}");
    let subject_root = format!("myelin.admission.{suffix}");
    let filter_subject = format!("{subject_root}.>");
    let consumer_name = format!("admission-reconcile-{suffix}");
    let stream = js
        .create_stream(async_nats::jetstream::stream::Config {
            name: stream_name.clone(),
            subjects: vec![filter_subject.clone()],
            ..Default::default()
        })
        .await
        .expect("create admission-reconciliation stream");

    let handle = tokio::runtime::Handle::current();
    let initial = JetStreamConsumerConfig::bounded(
        &cfg.nats_url,
        &stream_name,
        &subject_root,
        &filter_subject,
        &consumer_name,
    )
    .with_admission(DurableWorkerAdmission::new(96, 32, 24).unwrap());
    drop(NatsJetStreamBus::connect_consumer(initial, handle.clone()).expect("first startup"));

    let tightened = JetStreamConsumerConfig::bounded(
        &cfg.nats_url,
        &stream_name,
        &subject_root,
        &filter_subject,
        &consumer_name,
    )
    .with_admission(DurableWorkerAdmission::new(24, 8, 6).unwrap());
    drop(NatsJetStreamBus::connect_consumer(tightened, handle).expect("restart"));

    let info = stream
        .consumer_info(&consumer_name)
        .await
        .expect("inspect reconciled consumer");
    assert_eq!(info.config.max_ack_pending, 24);
    assert_eq!(info.config.max_batch, 8);

    js.delete_stream(&stream_name).await.expect("delete stream");
}
