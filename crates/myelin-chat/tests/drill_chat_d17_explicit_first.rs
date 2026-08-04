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

fn human(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

fn agent_id() -> PrincipalId {
    PrincipalId("agent:assistant".into())
}

fn chat_trigger(actor: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("evt:{subject}")),
        type_: EventType(CHAT_MESSAGE_MENTIONED.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(human(actor)),
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

#[test]
fn chat_classifies_a_casual_mention_as_notify_only() {
    assert_eq!(
        agent_dispatch_class(CHAT_MESSAGE_MENTIONED, false),
        AgentDispatchClass::NotifyOnly,
        "chat's explicit-first decision: a casual @agent mention notifies only"
    );
}

#[test]
fn casual_agent_mention_notifies_does_not_spawn_a_costed_run() {
    let gate = InMemoryCostGate::new(1);
    gate.credit(&tenant(), 100);
    let mut tier = DispatchTier::new(RecordingTarget::new(), gate);

    let class = agent_dispatch_class(CHAT_MESSAGE_MENTIONED,  false);
    assert_eq!(class, AgentDispatchClass::NotifyOnly);
    let trigger = match class {
        AgentDispatchClass::NotifyOnly => TriggerKind::Mention,
        AgentDispatchClass::ExplicitDispatch => TriggerKind::Automation,
    };

    let req = DispatchRequest {
        event: chat_trigger("psn:alice", "myelin://acme/chat/message/M1"),
        agent: agent_id(),
        run_ref: "run:assistant:1".into(),
        trigger,
    };
    let disp = tier.dispatch(
        &req,
        || EventId("evt:dispatched".into()),
        &Timestamp("2026-06-23T00:00:01Z".into()),
    );

    assert_eq!(
        disp,
        Disposition::NotifiedOnly,
        "a casual @agent mention NOTIFIES - it does not auto-spawn a costed run"
    );
    assert_eq!(
        tier.target().delivered_count(),
        0,
        "CHAT-D17 threshold: 0 auto-spawn from a casual mention"
    );
    assert_eq!(
        tier.telemetry().delivered,
        0,
        "0 runs dispatched (the notify did not become a run)"
    );
    assert_eq!(
        tier.telemetry().notified_only,
        1,
        "exactly the one explicit-first notify recorded"
    );
    assert_eq!(
        tier.telemetry().no_balance_refused,
        0,
        "the mention did NOT reserve (it is notify-only, not a budget refusal - ample balance)"
    );
}

#[test]
fn an_explicit_action_dispatches_a_costed_run_through_the_reserve_gate() {
    let gate = InMemoryCostGate::new(1);
    gate.credit(&tenant(), 5);
    let mut tier = DispatchTier::new(RecordingTarget::new(), gate);

    let class = agent_dispatch_class(CHAT_REACTION_ADDED,  true);
    assert_eq!(class, AgentDispatchClass::ExplicitDispatch);
    let req = DispatchRequest {
        event: chat_trigger("psn:alice", "myelin://acme/chat/message/M2"),
        agent: agent_id(),
        run_ref: "run:assistant:2".into(),
        trigger: TriggerKind::Automation,
    };
    let disp = tier.dispatch(
        &req,
        || EventId("evt:dispatched-2".into()),
        &Timestamp("2026-06-23T00:00:02Z".into()),
    );

    assert!(
        matches!(disp, Disposition::Delivered { .. }),
        "an explicit action dispatches a costed run, got {disp:?}"
    );
    assert_eq!(
        tier.target().delivered_count(),
        1,
        "exactly one run dispatched for the explicit action"
    );
    assert_eq!(
        tier.telemetry().delivered,
        1,
        "the explicit run was reserved + delivered (the reserve gate, 11.7)"
    );
}

#[test]
fn the_reserve_gate_refuses_an_explicit_run_with_no_balance() {
    let gate = InMemoryCostGate::new(1);
    let mut tier = DispatchTier::new(RecordingTarget::new(), gate);

    let req = DispatchRequest {
        event: chat_trigger("psn:alice", "myelin://acme/chat/message/M3"),
        agent: agent_id(),
        run_ref: "run:assistant:3".into(),
        trigger: TriggerKind::Automation,
    };
    let disp = tier.dispatch(
        &req,
        || EventId("evt:dispatched-3".into()),
        &Timestamp("2026-06-23T00:00:03Z".into()),
    );

    assert_eq!(
        disp,
        Disposition::NoBalanceRefused,
        "no balance → no execution (11.7) - reserve/settle gates even the explicit run"
    );
    assert_eq!(
        tier.target().delivered_count(),
        0,
        "0 delivered - the unfunded explicit run was refused, never dispatched"
    );
}
