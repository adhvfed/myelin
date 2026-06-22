//! # CDC — the Notif consumption of 2.4 (EventHandler template) + 3.1 (the Signal stream) (P-181)
//!
//! **Architecture:** `notifications.md` §3.4 (the router loop — a stateless `myelin-events`
//! consumer of curated Signals; idempotent on `origin_event`; emit-only-via-outbox), §5.1.
//! **Contracts:** **2.4** (the `EventHandler` consumer template — `subjects()` whitelist NEVER `*`,
//! ack-after-enqueue, dedup ledger, bounded prefetch, lag metric), **2.5** (`consumer_dedup`),
//! **2.2** (`OutboxTx::emit` — the ONLY emit path), **3.1** (`define_signal_rule` + the
//! `sig.<tenant>.>` Signal stream the engine, P-138, publishes). **ADR-19:** Notif consumes
//! Signals, NOT `evt.*`.
//!
//! This CDC pins the seam from BOTH sides — the PROVIDER (the Signal-curation engine emitting a
//! curated Signal to `sig.<tenant>.<severity>.<rule>`) and the CONSUMER (Notif's router consuming
//! it through the ONE sanctioned consumer runtime). The dated green artifact (2026-06-20):
//!
//! - **PROVIDER (3.1):** the [`SignalEngine`](myelin_query::SignalEngine) ingests a domain event,
//!   curates a Signal (`sig.acme.error.ci_run_failed`), and yields a [`PublishDraft`] carrying the
//!   [`Signal`]. (The dispatch tier, EB-23, turns the draft into the `OutboxTx::emit` of a
//!   `signal.*` event on that subject; here we build the published envelope directly from the draft
//!   — the SAME subject + the SAME `Signal` payload the router consumes.)
//! - **CONSUMER (2.4/2.5/2.2):** Notif's router — bound through [`myelin_events::consume`] (rule 3:
//!   the `sig.<tenant>.` whitelist, NEVER `*`) — consumes the published Signal, UPSERTs an inbox
//!   item idempotently (2.5: a redelivery dedups; the inner `(tenant, recipient, dedup_key)` UPSERT
//!   collapses), and emits `notif.item.created` via `OutboxTx::emit` ONLY (2.2: no `publish_now`).
//!
//! The two halves agree on the WIRE: the engine's `sig.<tenant>.<severity>.<rule>` subject is the
//! router's whitelist prefix, and the engine's [`Signal`] is exactly what the router deserializes.
//! A drift on either side (a renamed subject, a changed Signal shape) breaks THIS build.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, BusTransport, ConsumerName, CorrelationId, DataRole,
    DedupLedger, Delivered, EventEnvelope, EventHandler, EventId, EventType, InProcessBus, Message,
    OutboxStore, Relay, SubjectPattern, Timestamp, Visibility,
};
use myelin_identity::{Literal, ObjectType, Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_notif::{build_router, InboxProjection, NOTIF_ITEM_CREATED, ROUTER_CONSUMER_NAME};
use myelin_query::signals::{
    define_signal_rule, DedupKeyTpl, DedupWindow, PublishDraft, PublishKind, RuleId, Severity,
    Signal, SignalEngine,
};
use myelin_query::{CmpOp, EventMatcher, Expr, Predicate};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-1".into()),
        PrincipalKind::Human,
        tenant(),
    )
}

/// An `event.type == <type>` matcher (the selector a signal rule binds — contract 4.5/3.1).
fn type_matcher(type_: &str) -> EventMatcher {
    EventMatcher::compile(
        ObjectType("run".into()),
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("event.type".into()),
            rhs: Expr::Lit(Literal::Str(type_.into())),
        },
    )
    .unwrap()
}

/// The PROVIDER's rule (3.1 — `define_signal_rule`): collapse all `ci.run.failed` of one run into
/// one `error` Signal, auto-resolved by `ci.run.passed`.
fn ci_failed_rule() -> myelin_query::signals::SignalRule {
    define_signal_rule(
        RuleId("ci_run_failed".into()),
        type_matcher("ci.run.failed"),
        Severity::Error,
        DedupKeyTpl("ci.run.failed:{event.subject}".into()),
        DedupWindow { seconds: 0 },
        Some(type_matcher("ci.run.passed")),
    )
}

/// A raw domain event the Signal engine ingests (the upstream `ci.run.failed`).
fn domain_event(id: &str, run: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("ci.run.failed".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        subject: ArtifactRef(format!("myelin://acme/ci/run/{run}")),
        aggregate: AggregateKey(format!("ci:{run}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

/// Turn a PROVIDER [`PublishDraft`] into the published `sig.<tenant>.<severity>.<rule>` envelope the
/// dispatch tier (EB-23) emits — the SAME subject + the SAME [`Signal`] payload the router consumes.
/// This is the wire the CDC pins: the provider's subject == the consumer's whitelist; the provider's
/// Signal == the consumer's deserialized payload.
fn published_signal_envelope(id: &str, draft: &PublishDraft) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        // The dispatch tier maps Opened/Collapsed/Resolved → signal.opened/collapsed/resolved.
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: draft.signal.tenant.clone(),
        region: region(),
        actor: Actor(principal()),
        // THE WIRE: the `sig.<tenant>.<severity>.<rule>` subject (3.1 / Bus §4.4).
        subject: ArtifactRef(draft.subject.clone()),
        aggregate: AggregateKey(format!("signal:{}", draft.signal.dedup_key.0)),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:02Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:03Z".into()),
        // THE WIRE: the curated `Signal` is the payload (what the router deserializes).
        payload: serde_json::to_value(&draft.signal).unwrap(),
    }
}

fn see_all(_m: &myelin_query::matcher::RelMembership) -> bool {
    false
}

/// **The full provider → consumer CDC: a domain event → a curated Signal → an inbox item + a
/// `notif.item.created` emit.** The two halves agree on the wire (subject + Signal shape).
#[test]
fn provider_curates_signal_consumer_routes_it_to_inbox_and_emits() {
    // --- PROVIDER (3.1): curate a Signal from a domain event ---
    let mut engine = SignalEngine::new();
    engine.add_rule(ci_failed_rule());
    let drafts = engine.ingest(&domain_event("evt-dom-1", "42"), &SetExpr::All, &see_all);
    assert_eq!(drafts.len(), 1, "the rule curated one Signal");
    let draft = &drafts[0];
    assert_eq!(
        draft.kind,
        PublishKind::Opened,
        "the first failure opened the Signal"
    );
    // THE WIRE (provider side): the publish subject is sig.<tenant>.<severity>.<rule>.
    assert_eq!(draft.subject, "sig.acme.error.ci_run_failed");

    // The dispatch tier publishes it to the bus on that subject (here built from the draft).
    let published = published_signal_envelope("evt-sig-1", draft);

    // --- CONSUMER (2.4/2.5/2.2): the router consumes it ---
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // THE WIRE (consumer side): the published subject is on the router's `sig.<tenant>.` whitelist.
    assert_eq!(
        consumer.handler().subjects(),
        &[SubjectPattern("sig.acme.".into())],
        "the router whitelist is the sig.<tenant>. prefix (rule 3: never `*`)"
    );
    assert!(
        published.subject.0.starts_with("sig.acme."),
        "the engine's publish subject is on the router's whitelist (the seam agrees)"
    );

    let msg = Message {
        subject: published.subject.0.clone(),
        envelope: published.clone(),
    };
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Acked,
        "the router routed the curated Signal"
    );

    // The router UPSERTed exactly one inbox item (refs-not-payloads) ...
    assert_eq!(
        inbox.len(),
        1,
        "one inbox item UPSERTed from the curated Signal"
    );

    // ... and emitted exactly one notif.item.created via the outbox (2.2 — the ONLY emit path).
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || {
        Timestamp("2026-06-20T00:00:04Z".into())
    });
    relay.drain_to_empty();
    let emitted = bus.consume("");
    assert_eq!(
        emitted.len(),
        1,
        "exactly one notif.item.created emitted via OutboxTx::emit"
    );
    assert_eq!(emitted[0].type_.0, NOTIF_ITEM_CREATED);
    assert!(
        !emitted[0].contains_personal_data,
        "references-not-payloads: no inline PII"
    );
    // Causality correct-by-construction: caused by the Signal (root carries, depth+1).
    assert_eq!(emitted[0].correlation_id, published.correlation_id);
    assert_eq!(emitted[0].causation_id, Some(published.event_id.clone()));
    assert_eq!(emitted[0].depth, published.depth + 1);
}

/// **2.4/2.5 (the consumer template + dedup): a redelivered curated Signal is deduped — ONE inbox
/// row, ONE emit (0 dup).** The consumer-side idempotency the seam guarantees.
#[test]
fn redelivered_curated_signal_is_deduped() {
    let mut engine = SignalEngine::new();
    engine.add_rule(ci_failed_rule());
    let draft = engine.ingest(&domain_event("evt-dom-2", "7"), &SetExpr::All, &see_all)[0].clone();
    let published = published_signal_envelope("evt-sig-2", &draft);

    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();
    let msg = Message {
        subject: published.subject.0.clone(),
        envelope: published,
    };

    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Acked,
        "first delivery routes"
    );
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Deduplicated,
        "redelivery dedups (2.5)"
    );
    assert_eq!(inbox.len(), 1, "0 dup: exactly one inbox row");
    assert_eq!(outbox.committed_count(), 1, "0 dup: exactly one emit");
    assert_eq!(consumer.name(), &ConsumerName(ROUTER_CONSUMER_NAME.into()));
}

/// **3.1 (the Signal stream wire): the engine's collapse `count` rides on the SAME Signal payload
/// the router consumes.** N domain failures → one Signal `count=N` (the storm-control primitive),
/// and the router consumes the curated Signal at whatever count it carries (the skeleton routes the
/// representative item; the +N coalesce body is NOTIF-P11). The wire (Signal shape) is stable.
#[test]
fn engine_collapse_count_rides_the_wire_router_consumes_it() {
    let mut engine = SignalEngine::new();
    engine.add_rule(ci_failed_rule());
    // Three failures of the SAME run collapse into one Signal count=3 (provider side, §4.4).
    let mut last: Option<PublishDraft> = None;
    for i in 0..3 {
        let drafts = engine.ingest(
            &domain_event(&format!("evt-dom-3-{i}"), "99"),
            &SetExpr::All,
            &see_all,
        );
        last = Some(drafts[0].clone());
    }
    let draft = last.unwrap();
    assert_eq!(
        draft.signal.count, 3,
        "N=3 failures → one Signal count=3 (the wire carries it)"
    );

    // The router consumes the curated Signal (the skeleton routes it; the wire/Signal is stable).
    let published = published_signal_envelope("evt-sig-3", &draft);
    let consumer = build_router(
        &tenant(),
        InboxProjection::new(),
        OutboxStore::new(),
        DedupLedger::new(),
    )
    .unwrap();
    let msg = Message {
        subject: published.subject.0.clone(),
        envelope: published,
    };
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Acked,
        "the router consumed the count=3 Signal"
    );
    // Round-trip the Signal shape (the wire contract): the count survives serde.
    let round: Signal =
        serde_json::from_value(serde_json::to_value(&draft.signal).unwrap()).unwrap();
    assert_eq!(
        round.count, 3,
        "the Signal shape round-trips (the wire is stable)"
    );
}
