//! Stage 2 smoke integration test — the NATS JetStream `BusTransport` backing, round-tripped
//! THROUGH the trait (not the raw SDK).
//!
//! This is distinct from the Stage 1 `integration_nats.rs` (which proves the raw async-nats SDK
//! is reachable). Here we drive [`NatsJetStreamBus`] entirely through the FROZEN
//! [`BusTransport`] surface: `put` (publish + Nats-Msg-Id dedup), `consume` (durable PULL
//! consumer fetch), `ack` (explicit ack). Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-events --features integration --test smoke_nats_bus -- --nocapture
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
    Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId("acme".into()))
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

/// put (publish) + durable-pull-consume + ack, all through the BusTransport trait, plus the
/// Nats-Msg-Id dedup property (a re-put of the same event_id is Deduplicated → 0 ghost).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nats_bus_put_consume_ack() {
    let cfg = MyelinConfig::dev();
    let suffix = std::process::id();
    let stream = format!("MYELIN_SMOKE_{suffix}");
    let subject_root = format!("myelin_smoke_{suffix}");
    let consumer = format!("{stream}_pull");

    // The bus is built through the (sync) trait constructor; it ensures the durable stream +
    // durable PULL consumer exist. block_in_place needs a multi-thread runtime (flavor above).
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

    // 1. put: a fresh publish is Accepted.
    let d1 = bus.put(&subject, &env, &env.event_id).expect("put 1");
    assert_eq!(d1, Delivery::Accepted, "first publish must be Accepted");

    // 2. put again with the SAME event_id (Nats-Msg-Id dedup) → Deduplicated (0 ghost).
    let d2 = bus.put(&subject, &env, &env.event_id).expect("put 2");
    assert_eq!(
        d2,
        Delivery::Deduplicated,
        "duplicate publish (same event_id) must be Deduplicated — broker-side dedup, 0 ghost"
    );

    // 3. consume: the durable PULL consumer delivers exactly the one stored message.
    let consumed = bus.consume(&subject_root);
    assert_eq!(consumed.len(), 1, "exactly one message must be delivered (dedup suppressed the 2nd)");
    assert_eq!(consumed[0].event_id, env.event_id);
    assert_eq!(consumed[0].subject, subject);

    // 4. ack the delivered message (explicit ack). A second consume now delivers nothing (the
    // acked message is not redelivered).
    bus.ack(&consumer, &consumed[0].event_id);
    let after = bus.consume(&subject_root);
    assert!(after.is_empty(), "acked message must not be redelivered");

    // Cleanup: purge the stream's state (the frozen shape's 4th method).
    bus.purge();
}
