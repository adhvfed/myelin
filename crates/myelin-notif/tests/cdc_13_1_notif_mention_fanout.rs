//! # CDC — the Notif consumption of 13.1 (the `mention(Principal)` frozen inline node) (P-190)
//!
//! **Architecture:** `notifications.md` §3.5 (write-fanout for the bounded high-signal set —
//! materialise one `inbox_item` per mentioned recipient from the `mention(Principal)` frozen inline
//! structured node; Notif reads the STRUCTURED node, NEVER free text — AG-6), §3.2.4 (the
//! hot-subject cap bounds even the write-fanout side). **Contract:** **13.1** (the
//! `mention(Principal)` inline node in the `myelin-content` taxonomy — frozen, identical across
//! Chat/Issues/Knowledge, X-2/C10). **External insight:** `04-hard-problems.md` §5.3 (AG-6 — only a
//! structured ref re-triggers, never raw text).
//!
//! This CDC pins the 13.1 seam from BOTH sides — the PROVIDER (a content-bearing subsystem
//! Chat/Issues/Knowledge authoring a body that carries the frozen `InlineNode::Mention(Principal)`
//! node, stamped onto the curated Signal envelope by the dispatch tier) and the CONSUMER (Notif's
//! router reading the STRUCTURED node off the envelope and write-fanning one `inbox_item` per
//! mentioned recipient). The dated green artifact (2026-06-20):
//!
//! - **PROVIDER (13.1):** a producer builds the frozen `myelin_content::InlineNode::Mention(Principal)`
//!   node (the SAME node type Chat/Issues/Knowledge all author — identical, X-2/C10) and the dispatch
//!   tier serializes the structured nodes onto the Signal envelope under the frozen
//!   [`myelin_notif::SIGNAL_MENTIONS_KEY`].
//! - **CONSUMER (§3.5/AG-6):** Notif's router — bound through the ONE sanctioned consumer runtime —
//!   reads ONLY the structured node (`Vec<InlineNode>`, never a free-text scrape) and materialises
//!   exactly one `inbox_item` per mentioned recipient (`reason = Mentioned`, `class = Direct`).
//!
//! The two halves agree on the WIRE: the producer's `InlineNode::Mention(Principal)` is byte-identical
//! to what the consumer deserializes (the frozen 13.1 node), and the envelope key is the named
//! constant. A drift on either side (a changed node shape, a renamed key, a free-text fallback)
//! breaks THIS build.

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
/// The author of the content (the actor — NOT a mentioned recipient, so not self-suppressed).
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

/// **PROVIDER side:** the dispatch tier (EB-23) builds the curated-Signal envelope and STAMPS the
/// originating content's frozen `InlineNode::Mention(Principal)` nodes (13.1) onto the payload under
/// [`SIGNAL_MENTIONS_KEY`]. The SAME node type every content-bearing subsystem authors (X-2/C10).
fn provider_signal_envelope(id: &str, sig: &Signal, mentions: &[Principal]) -> EventEnvelope {
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
    // The frozen 13.1 node — the canonical write-fanout producer (identical across Chat/Issues/KN).
    let nodes: Vec<InlineNode> = mentions.iter().cloned().map(InlineNode::Mention).collect();
    let mut payload = serde_json::to_value(sig).expect("Signal serializes");
    if let serde_json::Value::Object(map) = &mut payload {
        // The structured nodes ride beside the Signal, under the frozen wire key (the CDC pins it).
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

/// **The 13.1 seam holds: the PROVIDER's frozen `mention(Principal)` node is the CONSUMER's
/// write-fanout input — one `inbox_item` per mentioned recipient, classified `Mentioned`/`Direct`,
/// read from the STRUCTURED node (0 free-text parse).**
#[test]
fn cdc_notif_consumes_13_1_mention_node_one_item_per_recipient() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // PROVIDER: a content body mentioning three principals (the frozen 13.1 node), stamped onto the
    // curated Signal envelope by the dispatch tier.
    let sig = signal("pr_review_requested", "myelin://acme/git/pr/9", "pr-9");
    let mentions = [
        mentioned("p-alice"),
        mentioned("p-bob"),
        mentioned("p-carol"),
    ];
    let env = provider_signal_envelope("evt-mention-1", &sig, &mentions);

    // CONSUMER: Notif's router reads the STRUCTURED node and write-fans one item per recipient.
    let out = consumer.deliver(&Message {
        subject: env.subject.0.clone(),
        envelope: env,
    });
    assert_eq!(
        out,
        Delivered::Acked,
        "the mention-carrying Signal routes + acks"
    );

    // EXACTLY one inbox_item per mentioned recipient (the write-fanout threshold: 1 per recipient).
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
        // references-not-payloads: the recipient is the opaque principal_id, the subject a ref.
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

/// **AG-6: the node shape is byte-identical across the boundary** — the PROVIDER's
/// `InlineNode::Mention(Principal)` round-trips through JSON to the EXACT same node the CONSUMER
/// reads. A drift in the frozen 13.1 node would break this round-trip (and the build).
#[test]
fn cdc_mention_node_is_byte_identical_across_the_boundary() {
    let p = mentioned("p-alice");
    let node = InlineNode::Mention(p.clone());
    // Serialize (provider) → deserialize (consumer): the node is identical (the frozen 13.1 shape).
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

/// **AG-6: a content-less Signal (no structured mention node) fans out NOTHING — there is NO
/// free-text fallback.** A CI-failure Signal with no `mentions` key produces 0 mention rows; the only
/// recipient source is the structured node (the agent-loop reference gate — only a structured ref
/// re-triggers).
#[test]
fn cdc_no_structured_node_no_mention_fanout() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // A Signal with NO mentions key (a plain CI failure — content-less).
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
