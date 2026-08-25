use myelin_content::InlineNode;
use myelin_events::{
    derive_envelope, AggregateKey, DataRole, EmitContext, EventDraft, EventEnvelope, EventId,
    EventType, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{Reason, SIGNAL_MENTIONS_KEY};
use myelin_query::{DedupKey, RuleId, Severity, Signal, SignalState};

use crate::glue::RULE_KEY_REPLIED;
use crate::store::{MessageId, StoreError};

pub(super) struct ThreadReplyEvents {
    pub(super) replied: EventEnvelope,
    pub(super) notification: Option<EventEnvelope>,
}

pub(super) fn thread_reply_events(
    message: &EventEnvelope,
    root: &MessageId,
    reply: &MessageId,
    recipient: Option<&PrincipalId>,
    replying_principal: &PrincipalId,
) -> Result<ThreadReplyEvents, StoreError> {
    let thread = crate::subs::mint_thread(message.tenant.as_str(), root.as_str())
        .map_err(|error| StoreError::Cold(format!("mint reply thread reference: {error}")))?;
    let replied = derive_envelope(
        EventDraft {
            type_: EventType(crate::events::CHAT_THREAD_REPLIED.into()),
            subject: thread.clone(),
            aggregate: message.aggregate.clone(),
            payload: serde_json::json!({
                "conversation_id": message.payload["conversation_id"],
                "thread_root_id": root.as_str(),
                "reply_message_id": reply.as_str(),
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        derived_context(message, "thread-replied"),
        Some(message),
    );
    let notification = recipient
        .filter(|recipient| *recipient != replying_principal)
        .map(|recipient| reply_signal(&replied, &thread, root, recipient))
        .transpose()?;
    Ok(ThreadReplyEvents {
        replied,
        notification,
    })
}

fn reply_signal(
    replied: &EventEnvelope,
    thread: &myelin_tenancy::ArtifactRef,
    root: &MessageId,
    recipient: &PrincipalId,
) -> Result<EventEnvelope, StoreError> {
    let dedup_key = format!("chat-thread:{}", root.as_str());
    let signal = Signal {
        rule_id: RuleId(RULE_KEY_REPLIED.into()),
        tenant: replied.tenant.clone(),
        severity: Severity::Notice,
        dedup_key: DedupKey(dedup_key.clone()),
        subject: thread.clone(),
        count: 1,
        state: SignalState::Open,
        first_seen: replied.occurred_at.0.clone(),
        last_seen: replied.occurred_at.0.clone(),
    };
    let mut payload = serde_json::to_value(signal).map_err(payload_error)?;
    payload[SIGNAL_MENTIONS_KEY] = serde_json::to_value([InlineNode::Mention(Principal::stub(
        recipient.clone(),
        PrincipalKind::Human,
        replied.tenant.clone(),
    ))])
    .map_err(payload_error)?;
    payload["notification_reason"] =
        serde_json::to_value(Reason::Replied).map_err(payload_error)?;

    let aggregate_id = &blake3::hash(dedup_key.as_bytes()).to_hex()[..32];
    Ok(derive_envelope(
        EventDraft {
            type_: EventType("signal.opened".into()),
            subject: myelin_tenancy::ArtifactRef(format!(
                "sig.{}.notice.chat_thread_replied",
                replied.tenant.0
            )),
            aggregate: AggregateKey(format!("signal:{aggregate_id}")),
            payload,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        derived_context(replied, "reply-signal"),
        Some(replied),
    ))
}

fn payload_error(error: serde_json::Error) -> StoreError {
    StoreError::Cold(format!("encode Chat reply notification: {error}"))
}

fn derived_context(parent: &EventEnvelope, purpose: &str) -> EmitContext {
    EmitContext {
        event_id: derived_event_id(&parent.event_id, purpose),
        tenant: parent.tenant.clone(),
        region: parent.region.clone(),
        actor: parent.actor.clone(),
        schema_ver: parent.schema_ver,
        occurred_at: parent.occurred_at.clone(),
        recorded_at: parent.recorded_at.clone(),
        caused_by: parent.caused_by.clone(),
    }
}

fn derived_event_id(parent: &EventId, purpose: &str) -> EventId {
    let mut digest = blake3::Hasher::new();
    digest.update(b"myelin.chat.derived-event.v1\0");
    digest.update(purpose.as_bytes());
    digest.update(b"\0");
    digest.update(parent.0.as_bytes());
    EventId(format!("chat-{}", &digest.finalize().to_hex()[..32]))
}

#[cfg(test)]
mod tests {
    use myelin_events::{Actor, CorrelationId, Timestamp};
    use myelin_tenancy::{Region, TenantId};

    use super::*;

    fn message() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("evt-message".into()),
            type_: EventType(crate::events::CHAT_MESSAGE_CREATED.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("chat-author:pseudonym".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            subject: myelin_tenancy::ArtifactRef(
                "myelin://acme/chat/message/01J00000000000000000000001".into(),
            ),
            aggregate: AggregateKey("channel:room".into()),
            causation_id: None,
            correlation_id: CorrelationId("evt-message".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-08-25T10:00:00Z".into()),
            recorded_at: Timestamp("2026-08-25T10:00:01Z".into()),
            payload: serde_json::json!({ "conversation_id": "room" }),
        }
    }

    #[test]
    fn a_reply_addresses_the_root_author_without_copying_message_content() {
        let root = MessageId("01J00000000000000000000000".into());
        let reply = MessageId("01J00000000000000000000001".into());
        let events = thread_reply_events(
            &message(),
            &root,
            &reply,
            Some(&PrincipalId("alice".into())),
            &PrincipalId("bob".into()),
        )
        .unwrap();
        assert_eq!(events.replied.type_.0, crate::events::CHAT_THREAD_REPLIED);
        assert_eq!(
            events.replied.subject.0,
            format!(
                "myelin://acme/chat/thread/{}#thread-{}",
                root.as_str(),
                root.as_str(),
            )
        );
        let notification = events.notification.unwrap();
        assert_eq!(notification.payload["notification_reason"], "replied");
        assert_eq!(notification.payload["subject"], events.replied.subject.0);
        assert!(!notification.payload.to_string().contains("message content"));
    }

    #[test]
    fn replying_to_your_own_root_emits_no_self_notification() {
        let root = MessageId("01J00000000000000000000000".into());
        let events = thread_reply_events(
            &message(),
            &root,
            &MessageId("01J00000000000000000000001".into()),
            Some(&PrincipalId("alice".into())),
            &PrincipalId("alice".into()),
        )
        .unwrap();
        assert!(events.notification.is_none());
    }
}
