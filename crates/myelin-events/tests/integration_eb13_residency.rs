#![cfg(feature = "integration")]

use async_nats::jetstream;
use myelin_config::MyelinConfig;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, BusStreamResidency, CorrelationId, DataRole, EventEnvelope,
    EventId, EventType, PartitionKey, ResidencyError, StreamSubject, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn envelope(tenant: &str, region: &str, event_id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("issue.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId(tenant.into()),
        region: Region(region.into()),
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
async fn a_region_pinned_stream_has_no_cross_region_read_path_on_the_live_broker() {
    let cfg = MyelinConfig::dev();
    let cell_region = Region::new(&cfg.region);
    let other_region = Region::new("eu-north");

    let client = async_nats::connect(&cfg.nats_url)
        .await
        .expect("connect to dev NATS (is the stack up with -js?)");
    let js = jetstream::new(client);

    let suffix = std::process::id();
    let tenant = TenantId("acme".into());
    let partition = PartitionKey::new(tenant.clone(), cell_region.clone());

    let stream_residency = BusStreamResidency::provision(&partition, "issue", &cell_region)
        .expect("a partition matching the cell region provisions a residency-pinned stream");
    assert_eq!(stream_residency.region(), &cell_region);
    assert_eq!(stream_residency.region_report().region, cell_region);
    assert!(stream_residency
        .region_report()
        .matches_region_of_record(&cell_region));

    let wrong = PartitionKey::new(tenant.clone(), other_region.clone());
    assert!(matches!(
        BusStreamResidency::provision(&wrong, "issue", &cell_region),
        Err(ResidencyError::WrongCellRegion { .. })
    ));

    let root = format!("evt_eb13_{suffix}");
    let stream_name = format!("MYELIN_EB13_{suffix}");
    let cell_filter = format!("{root}.{}.>", cell_region.as_str());
    let stream = js
        .create_stream(jetstream::stream::Config {
            name: stream_name.clone(),
            subjects: vec![cell_filter.clone()],
            ..Default::default()
        })
        .await
        .expect("create the region-pinned JetStream stream");

    let env = envelope("acme", cell_region.as_str(), &format!("acme-{suffix}"));
    let subj = StreamSubject::of(&env).expect("subject");
    let in_region_wire = format!("{root}.{}.{}", cell_region.as_str(), subj.to_subject());
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Nats-Msg-Id", env.event_id.0.as_str());
    js.publish_with_headers(in_region_wire.clone(), headers, "p".into())
        .await
        .expect("publish in-region")
        .await
        .expect("ack");

    let info = stream.get_info().await.expect("stream info");
    assert_eq!(
        info.state.messages, 1,
        "the in-region event landed in the cell stream"
    );

    let cross_region_filter = format!("{root}.{}.>", other_region.as_str());
    assert_ne!(
        cell_filter, cross_region_filter,
        "the region is the partition boundary"
    );
    let mut cross_consumer = stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some(format!("xregion_{suffix}")),
            filter_subject: cross_region_filter,
            ..Default::default()
        })
        .await
        .expect("a cross-region-scoped consumer (it will read nothing)");
    let cross_info = cross_consumer.info().await.expect("cross consumer info");
    assert_eq!(
        cross_info.num_pending, 0,
        "THE GATE: a read scoped to a DIFFERENT region captures 0 messages - no cross-region \
         stream read path (CP-D3 Bus slice; 0 cross-region reads)"
    );

    assert!(matches!(
        stream_residency.authorize_read(&other_region),
        Err(ResidencyError::CrossRegionRead { .. })
    ));
    stream_residency
        .authorize_read(&cell_region)
        .expect("an in-region read is authorized");

    let mut in_consumer = stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some(format!("inregion_{suffix}")),
            filter_subject: format!("{root}.{}.>", cell_region.as_str()),
            ..Default::default()
        })
        .await
        .expect("in-region consumer");
    let in_info = in_consumer.info().await.expect("in consumer info");
    assert_eq!(
        in_info.num_pending, 1,
        "the in-region read sees the event (the pin allows in-region)"
    );

    js.delete_stream(&stream_name).await.expect("delete stream");
}
