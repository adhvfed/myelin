use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, DedupLedger, Delivered,
    EventEnvelope, EventId, EventType, Message, OutboxStore, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{build_router, Class, InboxProjection, Reason, SIGNAL_MENTIONS_KEY};
use myelin_query::signals::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn author() -> Principal {
    Principal::stub(
        PrincipalId("p-author".into()),
        PrincipalKind::Human,
        tenant(),
    )
}
fn mentioned(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

fn signal(rule: &str, subject: &str, dedup: &str) -> Signal {
    Signal {
        rule_id: RuleId(rule.into()),
        tenant: tenant(),
        severity: Severity::Info,
        dedup_key: DedupKey(dedup.into()),
        subject: ArtifactRef(subject.into()),
        count: 1,
        state: SignalState::Open,
        first_seen: "2026-06-20T00:00:00Z".into(),
        last_seen: "2026-06-20T00:00:00Z".into(),
    }
}

fn provider_signal_envelope(id: &str, sig: &Signal, mentions: &[Principal]) -> EventEnvelope {
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
    let nodes: Vec<InlineNode> = mentions.iter().cloned().map(InlineNode::Mention).collect();
    let mut payload = serde_json::to_value(sig).expect("Signal serializes");
    if let serde_json::Value::Object(map) = &mut payload {
        map.insert(
            SIGNAL_MENTIONS_KEY.into(),
            serde_json::to_value(&nodes).unwrap(),
        );
    }
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(author()),
        subject: ArtifactRef(subject),
        aggregate: AggregateKey(format!("signal:{}", sig.dedup_key.0)),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload,
    }
}

#[test]
fn cdc_notif_consumes_13_1_mention_node_one_item_per_recipient() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    let sig = signal("pr_review_requested", "myelin://acme/git/pr/9", "pr-9");
    let mentions = [
        mentioned("p-alice"),
        mentioned("p-bob"),
        mentioned("p-carol"),
    ];
    let env = provider_signal_envelope("evt-mention-1", &sig, &mentions);

    let out = consumer.deliver(&Message {
        subject: env.subject.0.clone(),
        envelope: env,
    });
    assert_eq!(
        out,
        Delivered::Acked,
        "the mention-carrying Signal routes + acks"
    );

    for p in &mentions {
        let mention_rows = inbox
            .snapshot_for_tenant(&tenant())
            .into_iter()
            .filter(|r| r.recipient == p.principal_id.0 && r.reason == Reason::Mentioned)
            .collect::<Vec<_>>();
        assert_eq!(
            mention_rows.len(),
            1,
            "exactly ONE mention inbox_item for {}",
            p.principal_id.0
        );
        let row = &mention_rows[0];
        assert_eq!(
            row.class,
            Class::Direct,
            "a mention is directly addressed → Direct"
        );
        assert_eq!(
            row.recipient, p.principal_id.0,
            "the recipient is the mentioned principal id"
        );
        assert_eq!(
            row.subject.0, "myelin://acme/git/pr/9",
            "the subject is a ref (no payload)"
        );
    }
}

#[test]
fn cdc_mention_node_is_byte_identical_across_the_boundary() {
    let p = mentioned("p-alice");
    let node = InlineNode::Mention(p.clone());
    let json = serde_json::to_value(&node).expect("the frozen node serializes");
    let back: InlineNode = serde_json::from_value(json).expect("the consumer reads the SAME node");
    assert_eq!(
        node, back,
        "the 13.1 mention node is byte-identical across the boundary (X-2/C10)"
    );
    match back {
        InlineNode::Mention(got) => assert_eq!(got.principal_id, p.principal_id),
        _ => panic!("the node is a Mention (the frozen write-fanout producer)"),
    }
}

#[test]
fn cdc_no_structured_node_no_mention_fanout() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    let sig = signal("ci_run_failed", "myelin://acme/ci/run/42", "run-42");
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
    let env = EventEnvelope {
        event_id: EventId("evt-plain".into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(author()),
        subject: ArtifactRef(subject.clone()),
        aggregate: AggregateKey(format!("signal:{}", sig.dedup_key.0)),
        causation_id: None,
        correlation_id: CorrelationId("evt-plain".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::to_value(&sig).unwrap(),
    };
    consumer.deliver(&Message {
        subject,
        envelope: env,
    });

    let mention_rows = inbox
        .snapshot_for_tenant(&tenant())
        .into_iter()
        .filter(|r| r.reason == Reason::Mentioned)
        .count();
    assert_eq!(
        mention_rows, 0,
        "no structured node → 0 mention fanout (no free-text fallback, AG-6)"
    );
}
