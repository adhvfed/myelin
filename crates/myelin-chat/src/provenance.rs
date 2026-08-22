use myelin_events::{Actor, CausedBy, CorrelationId, EventEnvelope, EventId};
use myelin_identity::{PrincipalId, PrincipalKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProvenance {
    pub agent: PrincipalId,
    pub runtime_ref: Option<String>,
    pub on_behalf_of: Option<PrincipalId>,
    pub triggered_by: Option<EventId>,
    pub correlation_id: CorrelationId,
    pub human_action: Option<CausedBy>,
    pub agent_badge: bool,
}

pub const PROVENANCE_AUDIT_LINK_KIND: &str = "audit-log:correlation";

pub fn agent_provenance(message: &EventEnvelope) -> Option<AgentProvenance> {
    let Actor(principal) = &message.actor;
    let PrincipalKind::Agent {
        runtime_ref,
        on_behalf_of,
    } = &principal.kind
    else {
        return None;
    };

    Some(AgentProvenance {
        agent: principal.principal_id.clone(),
        runtime_ref: Some(runtime_ref.0.clone()),
        on_behalf_of: on_behalf_of.clone(),
        triggered_by: message.causation_id.clone(),
        correlation_id: message.correlation_id.clone(),
        human_action: message.caused_by.clone(),
        agent_badge: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventType, Timestamp, Visibility};
    use myelin_identity::{Principal, PrincipalStatus, RuntimeRef};
    use myelin_tenancy::{Region, TenantId};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn message_from(actor: Principal) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("evt:post".into()),
            type_: EventType("chat.message.created".into()),
            schema_ver: 1,
            tenant: tenant(),
            region: Region("fr-par".into()),
            actor: Actor(actor),
            subject: ArtifactRef("myelin://acme/chat/message/M1".into()),
            aggregate: AggregateKey("agg:channel".into()),
            causation_id: Some(EventId("evt:explicit-action".into())),
            correlation_id: CorrelationId("root-flow-1".into()),
            caused_by: Some(CausedBy("session:alice".into())),
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
            pii_key_ref: None,
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn an_agent_message_explains_who_acted_and_on_whose_authority() {
        let actor = Principal::new(
            tenant(),
            Region("fr-par".into()),
            PrincipalId("agent:assistant".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("external:codex".into()),
                on_behalf_of: Some(PrincipalId("psn:alice".into())),
            },
            myelin_identity::DataRole::Controller,
            PrincipalStatus::Active,
        );

        let provenance = agent_provenance(&message_from(actor))
            .expect("an agent-authored message has structured provenance");

        assert_eq!(provenance.agent, PrincipalId("agent:assistant".into()));
        assert_eq!(provenance.runtime_ref.as_deref(), Some("external:codex"));
        assert_eq!(
            provenance.on_behalf_of,
            Some(PrincipalId("psn:alice".into()))
        );
        assert_eq!(
            provenance.triggered_by,
            Some(EventId("evt:explicit-action".into()))
        );
        assert_eq!(
            provenance.correlation_id,
            CorrelationId("root-flow-1".into())
        );
        assert_eq!(
            provenance.human_action,
            Some(CausedBy("session:alice".into()))
        );
        assert!(provenance.agent_badge);
        assert_eq!(PROVENANCE_AUDIT_LINK_KIND, "audit-log:correlation");
    }

    #[test]
    fn a_human_message_does_not_pretend_to_have_agent_provenance() {
        let actor = Principal::stub(
            PrincipalId("psn:bob".into()),
            PrincipalKind::Human,
            tenant(),
        );

        assert_eq!(agent_provenance(&message_from(actor)), None);
    }
}
