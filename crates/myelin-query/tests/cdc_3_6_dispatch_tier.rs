use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{
    CostGate, DispatchError, DispatchRequest, DispatchTarget, DispatchTier, Disposition,
    InMemoryCostGate, Reservation, TriggerKind,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("t1".into()),
    )
}

fn event(actor: &str, subject: &str, depth: u32, correlation: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("trigger:{subject}")),
        type_: EventType("chat.message.created".into()),
        schema_ver: 1,
        tenant: TenantId("t1".into()),
        region: Region("t1-home".into()),
        actor: Actor(principal(actor)),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("agg:1".into()),
        causation_id: None,
        correlation_id: CorrelationId(correlation.into()),
        caused_by: Some(CausedBy("session:human".into())),
        depth,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

#[derive(Default)]
struct InboxConsumer {
    received: RefCell<Vec<EventEnvelope>>,
}
impl DispatchTarget for InboxConsumer {
    fn deliver(&self, action: &EventEnvelope) -> Result<(), DispatchError> {
        self.received.borrow_mut().push(action.clone());
        Ok(())
    }
}

#[test]
fn cdc_3_6_provider_derives_nested_envelope_and_8_6_consumer_reads_it() {
    let gate = InMemoryCostGate::new(1);
    gate.credit(&TenantId("t1".into()), 5);
    let mut tier = DispatchTier::new(InboxConsumer::default(), gate);

    let trigger = event("human", "myelin://t1/chat/message/7", 4, "root-X");
    let req = DispatchRequest {
        event: trigger.clone(),
        agent: PrincipalId("agentX".into()),
        run_ref: "run-7".into(),
        trigger: TriggerKind::Automation,
    };
    let disp = tier.dispatch(
        &req,
        || EventId("action-7".into()),
        &Timestamp("2026-06-20T00:00:01Z".into()),
    );

    let action = match disp {
        Disposition::Delivered { action } => action,
        o => panic!("expected Delivered, got {o:?}"),
    };
    assert_eq!(action.causation_id, Some(trigger.event_id.clone()));
    assert_eq!(action.correlation_id, trigger.correlation_id);
    assert_eq!(action.depth, trigger.depth + 1);

    let received = tier.target().received.borrow();
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0], *action,
        "the 8.6 consumer reads the exact nested envelope"
    );
}

#[test]
fn cdc_11_7_consumer_enforces_no_balance_no_execution_and_idempotent_reserve() {
    let gate = InMemoryCostGate::new(1);
    let t1 = TenantId("t1".into());
    gate.credit(&t1, 1);
    let r1: Option<Reservation> = gate.reserve(&t1, "run-A");
    assert!(r1.is_some(), "reserve admits when there is balance");
    let r1b = gate.reserve(&t1, "run-A");
    assert_eq!(r1, r1b, "redelivery re-reserves the same reservation");
    assert!(gate.reserve(&t1, "run-B").is_none());

    let empty = InMemoryCostGate::new(1);
    let mut tier = DispatchTier::new(InboxConsumer::default(), empty);
    let trigger = event("human", "myelin://t1/chat/message/1", 1, "root-Y");
    let req = DispatchRequest {
        event: trigger,
        agent: PrincipalId("agentX".into()),
        run_ref: "run-Z".into(),
        trigger: TriggerKind::Automation,
    };
    let disp = tier.dispatch(
        &req,
        || EventId("a".into()),
        &Timestamp("2026-06-20T00:00:01Z".into()),
    );
    assert_eq!(disp, Disposition::NoBalanceRefused);
    assert_eq!(
        tier.target().received.borrow().len(),
        0,
        "no balance → 0 delivered"
    );
}
