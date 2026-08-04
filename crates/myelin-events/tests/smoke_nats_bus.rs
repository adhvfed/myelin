#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::nats::NatsJetStreamBus;
use myelin_events::relay::{BusTransport, Delivery};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn envelope(id: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: Some(CausedBy("session:abc".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": "x" }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nats_bus_put_consume_ack() {
    let cfg = MyelinConfig::dev();
    let suffix = std::process::id();
    let stream = format!("MYELIN_SMOKE_{suffix}");
    let subject_root = format!("myelin_smoke_{suffix}");
    let consumer = format!("{stream}_pull");

    let bus = NatsJetStreamBus::connect(
        &cfg.nats_url,
        &stream,
        &subject_root,
        &consumer,
        tokio::runtime::Handle::current(),
    )
    .expect("connect NATS JetStream bus (is the stack up with -js?)");

    let subject = ArtifactRef("myelin://acme/issues/ISSUE-1".into());
    let env = envelope("smoke-evt-1", &subject.0);

    let d1 = bus.put(&subject, &env, &env.event_id).expect("put 1");
    assert_eq!(d1, Delivery::Accepted, "first publish must be Accepted");

    let d2 = bus.put(&subject, &env, &env.event_id).expect("put 2");
    assert_eq!(
        d2,
        Delivery::Deduplicated,
        "duplicate publish (same event_id) must be Deduplicated - broker-side dedup, 0 ghost"
    );

    let consumed = bus.consume(&subject_root);
    assert_eq!(
        consumed.len(),
        1,
        "exactly one message must be delivered (dedup suppressed the 2nd)"
    );
    assert_eq!(consumed[0].event_id, env.event_id);
    assert_eq!(consumed[0].subject, subject);

    bus.ack(&consumer, &consumed[0].event_id);
    let after = bus.consume(&subject_root);
    assert!(after.is_empty(), "acked message must not be redelivered");

    bus.purge();
}
