use std::collections::BTreeMap;

use myelin_content::InlineNode;
use myelin_events::{
    derive_envelope, AggregateKey, DataRole, EmitContext, EventDraft, EventEnvelope, EventId,
    EventType, Visibility,
};
use myelin_identity::Principal;
use myelin_notif::{Reason, SIGNAL_MENTIONS_KEY};
use myelin_query::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_tenancy::ArtifactRef;

use crate::glue::RULE_KEY_MENTIONED;

pub(crate) fn message_mention_signal(
    message: &EventEnvelope,
    event_id: EventId,
    message_id: &str,
    nodes: &[InlineNode],
) -> Result<Option<EventEnvelope>, serde_json::Error> {
    let mentions = unique_mentions(nodes);
    if mentions.is_empty() {
        return Ok(None);
    }

    let dedup_key = format!("chat-message:{message_id}");
    let signal = Signal {
        rule_id: RuleId(RULE_KEY_MENTIONED.into()),
        tenant: message.tenant.clone(),
        severity: Severity::Notice,
        dedup_key: DedupKey(dedup_key.clone()),
        subject: message.subject.clone(),
        count: 1,
        state: SignalState::Open,
        first_seen: message.occurred_at.0.clone(),
        last_seen: message.occurred_at.0.clone(),
    };
    let mut payload = serde_json::to_value(signal)?;
    payload[SIGNAL_MENTIONS_KEY] = serde_json::to_value(
        mentions
            .into_values()
            .map(InlineNode::Mention)
            .collect::<Vec<_>>(),
    )?;
    payload["notification_reason"] = serde_json::to_value(Reason::Mentioned)?;

    let aggregate_id = &blake3::hash(dedup_key.as_bytes()).to_hex()[..32];
    let draft = EventDraft {
        type_: EventType("signal.opened".into()),
        subject: ArtifactRef(format!(
            "sig.{}.notice.chat_message_mentioned",
            message.tenant.0
        )),
        aggregate: AggregateKey(format!("signal:{aggregate_id}")),
        payload,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };
    let context = EmitContext {
        event_id,
        tenant: message.tenant.clone(),
        region: message.region.clone(),
        actor: message.actor.clone(),
        schema_ver: message.schema_ver,
        occurred_at: message.occurred_at.clone(),
        recorded_at: message.recorded_at.clone(),
        caused_by: message.caused_by.clone(),
    };
    Ok(Some(derive_envelope(draft, context, Some(message))))
}

fn unique_mentions(nodes: &[InlineNode]) -> BTreeMap<String, Principal> {
    nodes
        .iter()
        .filter_map(|node| match node {
            InlineNode::Mention(principal) => {
                Some((principal.principal_id.0.clone(), principal.clone()))
            }
            InlineNode::ArtifactRefNode(_) | InlineNode::Embed(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{Actor, CorrelationId, EventId, EventType, Timestamp};
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn principal(id: &str) -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("fr-par".into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            myelin_identity::DataRole::Controller,
            myelin_identity::PrincipalStatus::Active,
        )
    }

    fn message() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("evt-message".into()),
            type_: EventType(crate::events::CHAT_MESSAGE_CREATED.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal("chat-author:pseudonym")),
            subject: ArtifactRef(
                "myelin://acme/chat/message/01J00000000000000000000000#message-01J00000000000000000000000"
                    .into(),
            ),
            aggregate: AggregateKey("channel:room-1".into()),
            causation_id: None,
            correlation_id: CorrelationId("evt-message".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-08-20T10:00:00Z".into()),
            recorded_at: Timestamp("2026-08-20T10:00:01Z".into()),
            payload: serde_json::json!({
                "message_id": "01J00000000000000000000000",
            }),
        }
    }

    #[test]
    fn an_ordinary_message_has_no_notification_side_effect() {
        assert!(message_mention_signal(
            &message(),
            EventId("evt-signal".into()),
            "01J00000000000000000000000",
            &[InlineNode::ArtifactRefNode(ArtifactRef(
                "myelin://acme/issue/issue/ENG-41".into()
            ))],
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn repeated_inline_mentions_become_one_direct_delivery_candidate() {
        let recipient = principal("reviewer");
        let signal = message_mention_signal(
            &message(),
            EventId("evt-signal".into()),
            "01J00000000000000000000000",
            &[
                InlineNode::Mention(recipient.clone()),
                InlineNode::Mention(recipient),
            ],
        )
        .unwrap()
        .unwrap();

        let mentions: Vec<InlineNode> =
            serde_json::from_value(signal.payload[SIGNAL_MENTIONS_KEY].clone()).unwrap();
        assert_eq!(mentions.len(), 1);
        assert!(matches!(
            &mentions[0],
            InlineNode::Mention(principal) if principal.principal_id.0 == "reviewer"
        ));
        assert_eq!(signal.type_.0, "signal.opened");
        assert_eq!(signal.causation_id.as_ref().unwrap().0, "evt-message");
        assert_eq!(signal.payload["notification_reason"], "mentioned");
        assert_eq!(
            signal.payload["subject"],
            "myelin://acme/chat/message/01J00000000000000000000000#message-01J00000000000000000000000"
        );
    }
}
