use myelin_events::{
    Actor, AggregateKey, ArtifactRef, Consumer, ConsumerName, CorrelationId, DataRole, DedupLedger,
    Delivered, EventEnvelope, EventHandler, EventId, EventType, HandleOutcome, Message,
    PrefetchBound, Reason, Region, SubjectPattern, Subscription, TenantId, Timestamp,
    UpcasterRegistry, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

const SUBJECT: &str = "myelin://acme/issues/issue/PROJ-1";

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn provider_emits_old_version(schema_ver: u32) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-old".into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver,
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(SUBJECT.into()),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:00Z".into()),
        payload: serde_json::json!({ "title": "ship it" }),
    }
}

fn consumer_registry() -> UpcasterRegistry {
    let mut r = UpcasterRegistry::new();
    r.register(EventType("issues.issue.created".into()), 1, 2, |mut e| {
        e.schema_ver = 2;
        if let serde_json::Value::Object(m) = &mut e.payload {
            m.insert("priority".into(), serde_json::json!("normal"));
        }
        e
    })
    .expect("v1->v2 adjacent forward hop");
    r
}

struct CurrentShapeHandler {
    seen_ver: Arc<AtomicU32>,
    saw_priority: Arc<AtomicU32>,
}
impl EventHandler for CurrentShapeHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        &[]
    }
    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        self.seen_ver.store(ev.schema_ver, Ordering::SeqCst);
        if ev
            .payload
            .as_object()
            .map(|m| m.contains_key("priority"))
            .unwrap_or(false)
        {
            self.saw_priority.store(1, Ordering::SeqCst);
        }
        HandleOutcome::Done
    }
}

fn subscription() -> Subscription {
    Subscription::bind(
        ConsumerName("indexer".into()),
        &["myelin://acme/issues/"],
        PrefetchBound::DEFAULT,
    )
    .expect("a `*`-free whitelist binds")
}

#[test]
fn cdc_2_8_old_event_is_upcast_to_current_before_handle() {
    let seen_ver = Arc::new(AtomicU32::new(0));
    let saw_priority = Arc::new(AtomicU32::new(0));
    let handler = CurrentShapeHandler {
        seen_ver: seen_ver.clone(),
        saw_priority: saw_priority.clone(),
    };

    let c = Consumer::new(handler, subscription(), DedupLedger::new())
        .with_upcaster(consumer_registry().into_hook());

    let old = provider_emits_old_version(1);
    assert_eq!(old.schema_ver, 1, "provider emitted the old version");
    assert!(
        old.payload.as_object().unwrap().get("priority").is_none(),
        "old shape has no priority"
    );

    let out = c.deliver(&Message {
        subject: SUBJECT.into(),
        envelope: old,
    });
    assert_eq!(out, Delivered::Acked, "the upcasted event handles cleanly");
    assert_eq!(
        seen_ver.load(Ordering::SeqCst),
        2,
        "the handler saw the CURRENT schema_ver"
    );
    assert_eq!(
        saw_priority.load(Ordering::SeqCst),
        1,
        "the handler saw the forward-added field"
    );
}

#[test]
fn cdc_2_8_unbridgeable_version_is_dead_lettered_never_silently_dropped() {
    let seen_ver = Arc::new(AtomicU32::new(0));
    let handler = CurrentShapeHandler {
        seen_ver: seen_ver.clone(),
        saw_priority: Arc::new(AtomicU32::new(0)),
    };
    let c = Consumer::new(handler, subscription(), DedupLedger::new())
        .with_upcaster(consumer_registry().into_hook());

    let mut ancient = provider_emits_old_version(0);
    ancient.event_id = EventId("01J-ancient".into());

    let out = c.deliver(&Message {
        subject: SUBJECT.into(),
        envelope: ancient,
    });

    match out {
        Delivered::DeadLettered(Reason(msg)) => {
            assert!(
                msg.contains("unbridgeable schema gap"),
                "the DLQ reason names the gap: {msg}"
            );
        }
        other => panic!("an unbridgeable version must dead-letter, got {other:?}"),
    }
    assert_eq!(
        seen_ver.load(Ordering::SeqCst),
        0,
        "the handler NEVER saw the un-upcastable shape"
    );
    assert_eq!(
        c.dead_letters().len(),
        1,
        "term'd to the DLQ - surfaced, 0 silently dropped"
    );
}
