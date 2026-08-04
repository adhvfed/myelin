#![cfg(feature = "integration")]

use async_nats::jetstream;
use myelin_config::MyelinConfig;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    PartitionKey, StreamSubject, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn envelope(tenant: &str, event_id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("issue.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId(tenant.into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )),
        subject: ArtifactRef(format!("myelin://{tenant}/issues/issue/PROJ-1")),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(event_id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn structured_subject_partitions_per_tenant_on_the_live_broker() {
    let cfg = MyelinConfig::dev();
    let client = async_nats::connect(&cfg.nats_url)
        .await
        .expect("connect to dev NATS (is the stack up with -js?)");
    let js = jetstream::new(client);

    let suffix = std::process::id();
    let root = format!("evt_eb12_{suffix}");
    let stream_name = format!("MYELIN_EB12_{suffix}");

    let stream = js
        .create_stream(jetstream::stream::Config {
            name: stream_name.clone(),
            subjects: vec![format!("{root}.>")],
            ..Default::default()
        })
        .await
        .expect("create JetStream stream");

    let env_acme = envelope("acme", &format!("acme-{suffix}"));
    let env_globex = envelope("globex", &format!("globex-{suffix}"));
    let subj_acme = StreamSubject::of(&env_acme).expect("acme subject");
    let subj_globex = StreamSubject::of(&env_globex).expect("globex subject");

    assert_eq!(
        subj_acme.to_subject(),
        "evt.acme.issue.issue.PROJ-1.created"
    );
    assert_eq!(PartitionKey::of(&env_acme).tenant, TenantId("acme".into()));
    assert_eq!(PartitionKey::of(&env_acme).region, Region("fr-par".into()));

    let wire_acme = format!("{root}.{}", subj_acme.to_subject());
    let wire_globex = format!("{root}.{}", subj_globex.to_subject());
    for (subject, id) in [
        (&wire_acme, env_acme.event_id.0.as_str()),
        (&wire_globex, env_globex.event_id.0.as_str()),
    ] {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Nats-Msg-Id", id);
        js.publish_with_headers(subject.clone(), headers, "p".into())
            .await
            .expect("publish")
            .await
            .expect("ack");
    }

    let info = stream.get_info().await.expect("stream info");
    assert_eq!(
        info.state.messages, 2,
        "both tenants' events landed in the cell stream"
    );

    let acme_filter = format!("{root}.{}", subj_acme.stream_filter());
    let mut acme_consumer = stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some(format!("acme_{suffix}")),
            filter_subject: acme_filter.clone(),
            ..Default::default()
        })
        .await
        .expect("acme filtered consumer");
    let acme_info = acme_consumer.info().await.expect("acme consumer info");
    assert_eq!(
        acme_info.num_pending, 1,
        "acme's per-(tenant, subsystem) filter must capture exactly acme's one event, never globex's \
         (the bulkhead - the tenant is the blast-radius unit, §7.1)"
    );

    let globex_filter = format!("{root}.{}", subj_globex.stream_filter());
    assert_ne!(
        acme_filter, globex_filter,
        "distinct tenants → distinct stream filters"
    );
    let mut globex_consumer = stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some(format!("globex_{suffix}")),
            filter_subject: globex_filter,
            ..Default::default()
        })
        .await
        .expect("globex filtered consumer");
    let globex_info = globex_consumer.info().await.expect("globex consumer info");
    assert_eq!(
        globex_info.num_pending, 1,
        "globex's filter captures exactly globex's one event"
    );

    js.delete_stream(&stream_name).await.expect("delete stream");
}
