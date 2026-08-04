use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{
    DispatchError, DispatchRequest, DispatchTarget, DispatchTier, Disposition, InMemoryCostGate,
    TriggerKind,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::HashSet;

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("t1".into()),
    )
}

fn event(n: u64) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("ev:{n}")),
        type_: EventType("chat.message.created".into()),
        schema_ver: 1,
        tenant: TenantId("t1".into()),
        region: Region("t1-home".into()),
        actor: Actor(principal("human")),
        subject: ArtifactRef(format!("myelin://t1/chat/message/{n}")),
        aggregate: AggregateKey(format!("agg:{n}")),
        causation_id: None,
        correlation_id: CorrelationId(format!("root-{n}")),
        caused_by: Some(CausedBy("session:human".into())),
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

fn req(n: u64) -> DispatchRequest {
    DispatchRequest {
        event: event(n),
        agent: PrincipalId("agentX".into()),
        run_ref: format!("run-{n}"),
        trigger: TriggerKind::Automation,
    }
}

struct BrokerAwareInbox {
    breaker: DependencyBreaker,
    landed: RefCell<HashSet<EventId>>,
    effect_count: RefCell<u64>,
}

impl BrokerAwareInbox {
    fn new(breaker: DependencyBreaker) -> BrokerAwareInbox {
        BrokerAwareInbox {
            breaker,
            landed: RefCell::new(HashSet::new()),
            effect_count: RefCell::new(0),
        }
    }
}

impl DispatchTarget for BrokerAwareInbox {
    fn deliver(&self, action: &EventEnvelope) -> Result<(), DispatchError> {
        if self
            .breaker
            .is_broken(&Dependency::Broker, &Scope::Tenant(TenantId("t1".into())))
        {
            return Err(DispatchError("broker severed".into()));
        }
        let mut landed = self.landed.borrow_mut();
        if landed.insert(action.event_id.clone()) {
            *self.effect_count.borrow_mut() += 1;
        }
        Ok(())
    }
}

#[test]
fn bus_d1_kill_consumer_sever_broker_zero_lost_zero_duplicate_on_reconnect() {
    let breaker = DependencyBreaker::new();
    let inbox = BrokerAwareInbox::new(breaker.clone());
    let gate = InMemoryCostGate::new(0);
    let mut tier = DispatchTier::new(inbox, gate);
    let t1 = TenantId("t1".into());

    let backlog: Vec<u64> = (0..8).collect();

    assert!(breaker
        .break_dependency(Dependency::Broker, Scope::Tenant(t1.clone()))
        .changed());
    let mut pending: Vec<u64> = Vec::new();
    for &n in &backlog {
        match tier.dispatch(
            &req(n),
            || EventId(format!("act-{n}")),
            &Timestamp("2026-06-20T00:00:01Z".into()),
        ) {
            Disposition::Delivered { .. } => panic!("should not deliver while the broker is down"),
            Disposition::BreakerShed { .. } => pending.push(n),
            o => panic!("unexpected disposition {o:?}"),
        }
    }
    assert_eq!(
        pending.len(),
        backlog.len(),
        "0 lost: every dispatch is queued for re-drive"
    );
    assert_eq!(
        *tier.target().effect_count.borrow(),
        0,
        "no effect landed while the broker was down"
    );

    assert!(breaker
        .restore_dependency(Dependency::Broker, Scope::Tenant(t1.clone()))
        .changed());
    let redrive: Vec<u64> = pending
        .iter()
        .copied()
        .chain([backlog[0], backlog[1]])
        .collect();
    for n in redrive {
        let disp = tier.dispatch(
            &req(n),
            || EventId(format!("act-{n}")),
            &Timestamp("2026-06-20T00:00:01Z".into()),
        );
        assert!(
            matches!(disp, Disposition::Delivered { .. }),
            "reconnect delivers"
        );
    }

    assert_eq!(
        *tier.target().effect_count.borrow(),
        backlog.len() as u64,
        "0 lost AND 0 duplicate: exactly one effect per distinct dispatch"
    );
    assert_eq!(tier.target().landed.borrow().len(), backlog.len());

    let mut src = SignalSource::new();
    let lag = backlog.len() as i64 - tier.target().landed.borrow().len() as i64;
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "dispatch-tier")],
        lag,
    );
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "dispatch-tier")],
        Predicate::Eq(0),
    )
    .expect_green();
}
