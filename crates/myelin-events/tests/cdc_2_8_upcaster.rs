//! # The CDC pair for contract 2.8 — schema-evolution upcasters (P-S09, forward-only)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 2.8
//! (schema evolution / upcasters — `(type, from_ver) → to_ver` pure fns at consume; forward-only).
//! Owning architecture: `00-platform-substrate.md` §2.1 (`schema_ver` gates evolution; upcasters
//! bridge versions at consume, forward-only) + `event-bus.md` §4.10.
//!
//! ## The contract this pair pins (the forward-only evolution seam)
//! Schema 2.8 is the seam between the side that EMITTED an event at an OLD `schema_ver` (the
//! PROVIDER — a producer running an earlier deploy) and the side that CONSUMES it after the type
//! has evolved (the CONSUMER — a handler written for the current shape that runs the upcaster
//! registry before `handle`). The frozen behaviour both sides agree on:
//!
//! - the producer only ever ADDS optional fields and bumps `schema_ver` (expand→migrate→contract,
//!   no rollback migrations) — so an old event is a STRICT subset shape of the new one;
//! - at consume, the registered `(type, from_ver) → to_ver` PURE chain lifts the old envelope up
//!   to the current shape BEFORE `handle` sees it (forward-only — never a down-cast);
//! - an UNBRIDGEABLE `schema_ver` (a missing upcaster) is term'd to the DLQ via a loud
//!   `NonRetryable`, **never silently dropped** and never handed to a handler at the wrong shape.
//!
//! This is the dedicated 2.8 provider+consumer pair the P-S09 TESTS field names (the chain
//! correctness + purity + un-upcastable→loud unit tests live in `upcast.rs::tests`; the
//! runtime-level dead-letter test lives in `consumer.rs::tests`). EB-10 (P-046) reconciles in
//! place against `upcast.rs` and adds its Bus-flavoured `v1→v2→v3` / DLQ assertions.

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

/// **PROVIDER side of 2.8** — a producer on an EARLIER deploy emits the event at an OLD
/// `schema_ver` with the old (subset) payload shape (here v1: only `title`, no `priority`).
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
        // The OLD subset payload — no `priority` (the field v1→v2 adds).
        payload: serde_json::json!({ "title": "ship it" }),
    }
}

/// The current-shape registry the CONSUMER runs at consume: `issues.issue.created` v1→v2 adds
/// `priority` (the expand half — a new optional field). Pure shape transform.
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

/// A handler written for the CURRENT (v2) shape: it asserts it ALWAYS sees the current
/// `schema_ver` + the forward-added `priority` field — never the old subset shape.
struct CurrentShapeHandler {
    seen_ver: Arc<AtomicU32>,
    saw_priority: Arc<AtomicU32>,
}
impl EventHandler for CurrentShapeHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        // A `*`-free whitelist (the handler-declared subjects; the runtime whitelist below uses
        // the same prefix). The runtime rejects a `*` subscription at bind time.
        &[]
    }
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
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

/// **CDC 2.8 — the old event is bridged forward to the current shape at consume.** The provider
/// emits v1 (old subset payload); the consumer runs the upcaster registry before `handle`, so the
/// handler sees v2 WITH `priority`. Forward-only: the consumer never saw the old shape.
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

    // The provider's OLD (v1) event arrives at the consumer.
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

/// **CDC 2.8 — an un-upcastable `schema_ver` is term'd to the DLQ, never silently dropped.** The
/// provider emits a version (v0) for which the consumer has NO upcaster chain to current. The
/// runtime dead-letters it loudly (`NonRetryable` → DLQ); the handler never runs; 0 silently
/// dropped (the message is surfaced on the dead-letter list, diagnosably).
#[test]
fn cdc_2_8_unbridgeable_version_is_dead_lettered_never_silently_dropped() {
    let seen_ver = Arc::new(AtomicU32::new(0));
    let handler = CurrentShapeHandler {
        seen_ver: seen_ver.clone(),
        saw_priority: Arc::new(AtomicU32::new(0)),
    };
    // The registry only knows v1->v2 (current = v2); a v0 event has no v0->v1 hop → unbridgeable.
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
        "term'd to the DLQ — surfaced, 0 silently dropped"
    );
}
