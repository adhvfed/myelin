//! # The CDC pair for contract 3.6 — the reactive/dispatch tier (EB-23 / P-143)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 3.6
//! (the reactive/dispatch tier — nested causality, structural loop guards, bounded dispatch,
//! explicit-first, reserve/settle — OWNED) + rows **8.6** (`EventInbox::deliver` explicit-first —
//! CONSUMED, the dispatch target) + **11.7** (reserve/settle — CONSUMED, the dispatch cost gate).
//! Owning architecture: `event-bus.md` §4.7 (the reactive/dispatch tier). AG-6 / BUS-5 / CHAT-1.
//!
//! ## The seam this pair pins
//! Row 3.6 is the seam between:
//! - the **PROVIDER** — the dispatch tier ([`myelin_query::DispatchTier`]): it runs the §4.7
//!   discipline gauntlet (self-guard → reference-gate → explicit-first → depth-ceiling →
//!   breaker/over-cap → reserve → nested-causality dispatch) and on a delivered dispatch produces
//!   a `Delivered { action }` whose envelope is NESTED on the trigger (`causation_id =
//!   trigger.event_id`, `correlation_id` carried, `depth = +1`). Its promise: the action's
//!   causality is correct-by-construction and it is reserved before delivery.
//! - the **CONSUMER (8.6)** — the Agent Fabric `EventInbox`: it receives the dispatched action's
//!   envelope through [`myelin_query::DispatchTarget::deliver`]. Its promise: it reads exactly the
//!   nested envelope the tier produced (the same causation/correlation/depth).
//! - the **CONSUMER (11.7)** — the reserve/settle `CostLedger`: the tier calls
//!   [`myelin_query::CostGate::reserve`] before delivery. Its promise: no balance → no execution,
//!   and a redelivery of the same `run_ref` re-reserves the same reservation (no double-charge).
//!
//! The pair asserts both sides agree: the provider derives the nested envelope + reserves; the
//! 8.6 consumer reads that exact envelope; the 11.7 consumer enforces no-balance-no-execution.

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

/// The 8.6 CONSUMER side: a stand-in Agent Fabric inbox that captures the delivered envelope so the
/// CDC can assert the consumer reads exactly the nested envelope the provider produced. (The real
/// `EventInbox` is `myelin-agent`, the named floor AG-P4 / P-216.)
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

    // PROVIDER promise: the action is NESTED on the trigger (correct-by-construction).
    let action = match disp {
        Disposition::Delivered { action } => action,
        o => panic!("expected Delivered, got {o:?}"),
    };
    assert_eq!(action.causation_id, Some(trigger.event_id.clone()));
    assert_eq!(action.correlation_id, trigger.correlation_id);
    assert_eq!(action.depth, trigger.depth + 1);

    // 8.6 CONSUMER promise: the inbox received exactly that envelope.
    let received = tier.target().received.borrow();
    assert_eq!(received.len(), 1);
    assert_eq!(
        received[0], *action,
        "the 8.6 consumer reads the exact nested envelope"
    );
}

#[test]
fn cdc_11_7_consumer_enforces_no_balance_no_execution_and_idempotent_reserve() {
    // 11.7 CONSUMER side, directly: reserve admits on balance, refuses on none, idempotent on run.
    let gate = InMemoryCostGate::new(1);
    let t1 = TenantId("t1".into());
    gate.credit(&t1, 1);
    let r1: Option<Reservation> = gate.reserve(&t1, "run-A");
    assert!(r1.is_some(), "reserve admits when there is balance");
    // Idempotent: a redelivery of the same run re-reserves the same reservation (no double-charge).
    let r1b = gate.reserve(&t1, "run-A");
    assert_eq!(r1, r1b, "redelivery re-reserves the same reservation");
    // A different run with exhausted balance is refused: no balance → no execution.
    assert!(gate.reserve(&t1, "run-B").is_none());

    // And through the tier: a no-balance tenant is refused, nothing delivered (the consumed gate).
    let empty = InMemoryCostGate::new(1); // tenant has 0 balance
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
