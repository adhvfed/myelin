use myelin_chat::events::{CHAT_MESSAGE_MENTIONED, CHAT_REACTION_ADDED};
use myelin_chat::glue::{agent_dispatch_class, AgentDispatchClass};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{
    DispatchRequest, DispatchTier, Disposition, InMemoryCostGate, RecordingTarget, TriggerKind,
};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn provider_trigger_kind(token: &str, is_explicit_action: bool) -> TriggerKind {
    match agent_dispatch_class(token, is_explicit_action) {
        AgentDispatchClass::NotifyOnly => TriggerKind::Mention,
        AgentDispatchClass::ExplicitDispatch => TriggerKind::Automation,
    }
}

fn trigger_event(subject: &str, token: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("evt:{subject}")),
        type_: EventType(token.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("psn:alice".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("agg:chan".into()),
        causation_id: None,
        correlation_id: CorrelationId("root-1".into()),
        caused_by: Some(CausedBy("session:human".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:00Z".into()),
        pii_key_ref: None,
        payload: serde_json::json!({}),
    }
}

fn consumer_disposition(token: &str, is_explicit_action: bool) -> (Disposition, usize) {
    let gate = InMemoryCostGate::new(1);
    gate.credit(&tenant(), 100);
    let mut tier = DispatchTier::new(RecordingTarget::new(), gate);
    let req = DispatchRequest {
        event: trigger_event("myelin://acme/chat/message/M1", token),
        agent: PrincipalId("agent:assistant".into()),
        run_ref: "run:1".into(),
        trigger: provider_trigger_kind(token, is_explicit_action),
    };
    let disp = tier.dispatch(
        &req,
        || EventId("evt:dispatched".into()),
        &Timestamp("2026-06-23T00:00:01Z".into()),
    );
    let delivered = tier.target().delivered_count();
    (disp, delivered)
}

#[test]
fn cdc_8_6_chat_casual_mention_provider_notify_only_consumer_does_not_spawn() {
    assert_eq!(
        agent_dispatch_class(CHAT_MESSAGE_MENTIONED, false),
        AgentDispatchClass::NotifyOnly,
        "PROVIDER: chat classifies a casual mention as notify-only"
    );
    let (disp, delivered) = consumer_disposition(CHAT_MESSAGE_MENTIONED, false);
    assert_eq!(
        disp,
        Disposition::NotifiedOnly,
        "CONSUMER: the dispatch tier notifies-only for a casual mention"
    );
    assert_eq!(
        delivered, 0,
        "0 auto-spawn from a casual mention (CHAT-D17 threshold)"
    );
}

#[test]
fn cdc_8_6_chat_explicit_action_provider_dispatch_consumer_spawns_a_run() {
    assert_eq!(
        agent_dispatch_class(CHAT_REACTION_ADDED, true),
        AgentDispatchClass::ExplicitDispatch,
        "PROVIDER: chat classifies an explicit action as a dispatch"
    );
    let (disp, delivered) = consumer_disposition(CHAT_REACTION_ADDED, true);
    assert!(
        matches!(disp, Disposition::Delivered { .. }),
        "CONSUMER: the dispatch tier dispatches a costed run for an explicit action, got {disp:?}"
    );
    assert_eq!(
        delivered, 1,
        "exactly one run dispatched for the explicit action"
    );
}
