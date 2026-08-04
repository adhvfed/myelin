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

fn published_signal_envelope(id: &str, draft: &PublishDraft) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: draft.signal.tenant.clone(),
        region: region(),
        actor: Actor(principal()),
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
        payload: serde_json::to_value(&draft.signal).unwrap(),
    }
}

fn see_all(_m: &myelin_query::matcher::RelMembership) -> bool {
    false
}

#[test]
fn provider_curates_signal_consumer_routes_it_to_inbox_and_emits() {
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
    assert_eq!(draft.subject, "sig.acme.error.ci_run_failed");

    let published = published_signal_envelope("evt-sig-1", draft);

    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

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

    assert_eq!(
        inbox.len(),
        1,
        "one inbox item UPSERTed from the curated Signal"
    );

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
    assert_eq!(emitted[0].correlation_id, published.correlation_id);
    assert_eq!(emitted[0].causation_id, Some(published.event_id.clone()));
    assert_eq!(emitted[0].depth, published.depth + 1);
}

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

#[test]
fn engine_collapse_count_rides_the_wire_router_consumes_it() {
    let mut engine = SignalEngine::new();
    engine.add_rule(ci_failed_rule());
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
    let round: Signal =
        serde_json::from_value(serde_json::to_value(&draft.signal).unwrap()).unwrap();
    assert_eq!(
        round.count, 3,
        "the Signal shape round-trips (the wire is stable)"
    );
}
