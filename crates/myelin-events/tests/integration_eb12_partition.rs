//! EB-12 (P-089) — the `(tenant, region)`-partitioned stream subject, PROVEN against the LIVE
//! NATS JetStream dev stack.
//!
//! Gated behind the `integration` cargo feature so the default build stays broker-free. Run
//! against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-events --features integration --test integration_eb12_partition -- --nocapture
//!
//! What this proves on REAL hardware (not the in-process fake):
//!  1. The structured §2.2 subject `evt.<tenant>.<subsystem>.<aggregate_type>.<aggregate_id>.
//!     <event_name>` derived from the envelope is the subject the broker actually stores — a
//!     per-subject stream filter `<root>.evt.<tenant>.<subsystem>.>` captures it (the per-(tenant,
//!     subsystem) routing split, §7.1).
//!  2. Two DISTINCT tenants emitting the same subsystem/aggregate/event land under DISTINCT
//!     structured subjects on the SAME root — the bulkhead property (the tenant is the blast-radius
//!     unit): one tenant's per-(tenant, subsystem) filter never captures another tenant's message.
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

/// The structured §2.2 subject reaches the real broker, and the per-(tenant, subsystem) filter
/// isolates one tenant's events from another's on the same stream root (the bulkhead property).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn structured_subject_partitions_per_tenant_on_the_live_broker() {
    let cfg = MyelinConfig::dev();
    let client = async_nats::connect(&cfg.nats_url)
        .await
        .expect("connect to dev NATS (is the stack up with -js?)");
    let js = jetstream::new(client);

    let suffix = std::process::id();
    let root = format!("evt_eb12_{suffix}"); // a unique capture root per run
    let stream_name = format!("MYELIN_EB12_{suffix}");

    // One cell-local stream captures the whole `<root>.>` subject space (the cell's stream).
    let stream = js
        .create_stream(jetstream::stream::Config {
            name: stream_name.clone(),
            subjects: vec![format!("{root}.>")],
            ..Default::default()
        })
        .await
        .expect("create JetStream stream");

    // Build the structured §2.2 subjects for two tenants, same subsystem/aggregate/event.
    let env_acme = envelope("acme", &format!("acme-{suffix}"));
    let env_globex = envelope("globex", &format!("globex-{suffix}"));
    let subj_acme = StreamSubject::of(&env_acme).expect("acme subject");
    let subj_globex = StreamSubject::of(&env_globex).expect("globex subject");

    // The §2.2 grammar, exactly — the partition key the streams are keyed under (contract 12.1).
    assert_eq!(
        subj_acme.to_subject(),
        "evt.acme.issue.issue.PROJ-1.created"
    );
    assert_eq!(PartitionKey::of(&env_acme).tenant, TenantId("acme".into()));
    assert_eq!(PartitionKey::of(&env_acme).region, Region("fr-par".into()));

    // Publish each under the capture root (the transport's `<root>.<structured-subject>` shape).
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

    // The cell stream stored both (one per tenant).
    let info = stream.get_info().await.expect("stream info");
    assert_eq!(
        info.state.messages, 2,
        "both tenants' events landed in the cell stream"
    );

    // The bulkhead property: a per-(tenant, subsystem) filter consumer for ACME sees ONLY acme.
    let acme_filter = format!("{root}.{}", subj_acme.stream_filter()); // ...evt.acme.issue.>
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
         (the bulkhead — the tenant is the blast-radius unit, §7.1)"
    );

    // Symmetrically, globex's filter never captures acme.
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

    // Cleanup.
    js.delete_stream(&stream_name).await.expect("delete stream");
}
