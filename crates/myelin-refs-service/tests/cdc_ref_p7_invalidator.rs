use myelin_events::{
    consume, Actor, AggregateKey, ArtifactRef, ConsumerName, ConsumerSpec, CorrelationId, DataRole,
    DedupLedger, Delivered, EventEnvelope, EventId, EventType, Message, Reason, Timestamp,
    Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{NoOpCacheShim, RefsProjectionInvalidator, INVALIDATOR_SUBJECT_PREFIXES};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn lifecycle_event(id: &str, type_: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(format!("agg:{subject}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

fn msg(subject: &str, ev: &EventEnvelope) -> Message {
    Message {
        subject: subject.into(),
        envelope: ev.clone(),
    }
}

#[test]
fn invalidator_binds_through_the_sanctioned_entrypoint_no_wildcard() {
    let shim = NoOpCacheShim::new();
    let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim));
    let spec = ConsumerSpec::new(
        ConsumerName("refs-projection-invalidator".into()),
        INVALIDATOR_SUBJECT_PREFIXES,
    );
    let consumer = consume(spec, inv, DedupLedger::new());
    assert!(
        consumer.is_ok(),
        "the invalidator binds with a *-free whitelist (one of the reviewed BUS-4 consumers)"
    );
}

#[test]
fn updated_busts_once_and_redelivery_is_deduped() {
    let shim = NoOpCacheShim::new();
    let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
    let spec = ConsumerSpec::new(
        ConsumerName("refs-projection-invalidator".into()),
        INVALIDATOR_SUBJECT_PREFIXES,
    );
    let consumer = consume(spec, inv, DedupLedger::new()).expect("bind the invalidator");

    let ref_ = "myelin://acme/knowledge/page/7c2";
    let ev = lifecycle_event("01J-u1", "knowledge.page.updated", ref_);

    assert_eq!(
        consumer.deliver(&msg("knowledge.page.updated", &ev)),
        Delivered::Acked,
        "first delivery busts the cache"
    );
    assert_eq!(
        consumer.deliver(&msg("knowledge.page.updated", &ev)),
        Delivered::Deduplicated,
        "redelivery is deduped (0 double-bust)"
    );
    let calls = shim.invalidations();
    assert_eq!(
        calls.len(),
        1,
        "exactly one invalidation call (idempotent on event_id)"
    );
    assert_eq!(calls[0].tenant, tenant(), "tenant-first");
    assert_eq!(
        calls[0].ref_.0, ref_,
        "the exact ArtifactRef the event named"
    );
}

#[test]
fn erased_busts_through_the_runtime() {
    let shim = NoOpCacheShim::new();
    let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
    let spec = ConsumerSpec::new(
        ConsumerName("refs-projection-invalidator".into()),
        INVALIDATOR_SUBJECT_PREFIXES,
    );
    let consumer = consume(spec, inv, DedupLedger::new()).expect("bind");
    let ref_ = "myelin://acme/issue/issue/ENG-1";
    let ev = lifecycle_event("01J-e1", "issue.issue.erased", ref_);
    assert_eq!(
        consumer.deliver(&msg("issue.issue.erased", &ev)),
        Delivered::Acked
    );
    assert_eq!(
        shim.call_count(),
        1,
        "the erased artifact's cache entry is busted"
    );
}

#[test]
fn created_is_acked_with_no_bust() {
    let shim = NoOpCacheShim::new();
    let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
    let spec = ConsumerSpec::new(
        ConsumerName("refs-projection-invalidator".into()),
        INVALIDATOR_SUBJECT_PREFIXES,
    );
    let consumer = consume(spec, inv, DedupLedger::new()).expect("bind");
    let ev = lifecycle_event(
        "01J-c1",
        "issue.issue.created",
        "myelin://acme/issue/issue/ENG-2",
    );
    assert_eq!(
        consumer.deliver(&msg("issue.issue.created", &ev)),
        Delivered::Acked,
        "a created event is acked (no-op for the invalidator)"
    );
    assert_eq!(shim.call_count(), 0, "no bust on create");
}

#[test]
fn malformed_updated_dead_letters() {
    let shim = NoOpCacheShim::new();
    let inv = RefsProjectionInvalidator::with_cache(Arc::new(shim.clone()));
    let spec = ConsumerSpec::new(
        ConsumerName("refs-projection-invalidator".into()),
        INVALIDATOR_SUBJECT_PREFIXES,
    );
    let consumer = consume(spec, inv, DedupLedger::new()).expect("bind");

    let mut bad = lifecycle_event("01J-bad", "knowledge.page.updated", "");
    bad.subject = ArtifactRef(String::new());
    bad.payload = serde_json::json!({ "title": "x" });
    match consumer.deliver(&msg("knowledge.page.updated", &bad)) {
        Delivered::DeadLettered(Reason(r)) => {
            assert!(
                r.contains("ArtifactRef"),
                "the poison names the missing ref: {r}"
            )
        }
        other => panic!("a malformed invalidation event must dead-letter, got {other:?}"),
    }
    assert_eq!(shim.call_count(), 0, "no bust on a malformed event");
}
