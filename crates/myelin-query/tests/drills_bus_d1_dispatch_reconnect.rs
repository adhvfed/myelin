//! # BUS-D1 (F5) — kill consumer + sever broker → 0 lost, 0 duplicate effects (EB-23 / P-143)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row BUS-D1
//! ("Kill consumer + sever broker during sustained publish → 0 lost, 0 duplicate effects on
//! reconnect", gate: **lost/dup = 0; lag drains**) — the F5 family; architecture §8.
//!
//! ## What this drill proves (the EB-23 GATE), in the dispatch tier's scope
//! The dispatch tier delivers dispatched actions to the Agent Fabric inbox (8.6) over the broker.
//! When the broker is SEVERED mid-stream (the consumer cannot deliver), the tier must NOT silently
//! drop the dispatch and must NOT double-effect it on reconnect:
//! 1. **0 LOST.** A dispatch attempted while the broker is down is NOT silently lost — it is
//!    re-driven on reconnect (the at-least-once posture; the durable re-drive is the outbox relay's
//!    in the production path, modelled here by the re-drive loop).
//! 2. **0 DUPLICATE EFFECTS.** The re-drive is **idempotent on `event_id`** — the inbox dedups a
//!    redelivered dispatched action (the `consumer_dedup` discipline, 2.5), so the surviving effect
//!    count after the storm-and-reconnect equals the number of DISTINCT dispatches, never more.
//! 3. **LAG DRAINS.** After reconnect every pending dispatch is delivered exactly once (the
//!    `ConsumerLag` survival signal reads `0`).
//!
//! The broker kill is driven through the FROZEN harness `Dependency::Broker` reversible break
//! injector (P-S03), and the verdict is read off the §10.2 `ConsumerLag` survival signal — exactly
//! the same fault + assertion vocabulary the events crate's SUB-D1/BUS-D1 drills use.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_harness::{Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{
    DispatchError, DispatchRequest, DispatchTarget, DispatchTier, Disposition, InMemoryCostGate,
    TriggerKind,
};
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::collections::HashSet;

fn principal(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, TenantId("t1".into()))
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

/// The 8.6 inbox CONSUMER, broker-aware + dedup-disciplined (the BUS-D1 model):
/// - while `Dependency::Broker` is SEVERED it refuses delivery (`Err`) — the consumer is "killed";
/// - it dedups a redelivered action by `event_id` (the `consumer_dedup` discipline, 2.5), so a
///   re-drive after reconnect lands the effect **exactly once** (0 duplicate).
struct BrokerAwareInbox {
    breaker: DependencyBreaker,
    /// The DISTINCT dispatched-action ids that landed an effect (the dedup ledger). A redelivery of
    /// an already-seen id is a no-op (0 duplicate effects).
    landed: RefCell<HashSet<EventId>>,
    /// The total number of effects applied (must equal `landed.len()` — proof of 0 duplicate).
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
        // The broker is severed → the consumer cannot deliver (it is "killed"). Surfaced, never a
        // silent success: the tier re-drives on reconnect.
        if self
            .breaker
            .is_broken(&Dependency::Broker, &Scope::Tenant(TenantId("t1".into())))
        {
            return Err(DispatchError("broker severed".into()));
        }
        // Dedup on event_id (2.5): a redelivered action lands its effect exactly once.
        let mut landed = self.landed.borrow_mut();
        if landed.insert(action.event_id.clone()) {
            *self.effect_count.borrow_mut() += 1;
        }
        Ok(())
    }
}

/// **BUS-D1 — sever the broker mid-stream, re-drive on reconnect → 0 lost, 0 duplicate effects.**
#[test]
fn bus_d1_kill_consumer_sever_broker_zero_lost_zero_duplicate_on_reconnect() {
    let breaker = DependencyBreaker::new();
    let inbox = BrokerAwareInbox::new(breaker.clone());
    let gate = InMemoryCostGate::new(0); // isolate from the balance gate
    let mut tier = DispatchTier::new(inbox, gate);
    let t1 = TenantId("t1".into());

    // The pending dispatch backlog (the "sustained publish" the broker drop interrupts).
    let backlog: Vec<u64> = (0..8).collect();

    // LEG 1 — sever the broker. Every dispatch attempted now FAILS to deliver (the consumer is
    // killed). The tier surfaces it (an audited BreakerShed), it is NOT silently lost — it stays in
    // the re-drive backlog.
    assert!(breaker
        .break_dependency(Dependency::Broker, Scope::Tenant(t1.clone()))
        .changed());
    let mut pending: Vec<u64> = Vec::new();
    for &n in &backlog {
        match tier.dispatch(&req(n), || EventId(format!("act-{n}")), &Timestamp("2026-06-20T00:00:01Z".into())) {
            Disposition::Delivered { .. } => panic!("should not deliver while the broker is down"),
            Disposition::BreakerShed { .. } => pending.push(n), // surfaced, queued for re-drive
            o => panic!("unexpected disposition {o:?}"),
        }
    }
    assert_eq!(pending.len(), backlog.len(), "0 lost: every dispatch is queued for re-drive");
    assert_eq!(*tier.target().effect_count.borrow(), 0, "no effect landed while the broker was down");

    // LEG 2 — restore the broker (reconnect). Re-drive the pending backlog AND replay the first two
    // (the at-least-once redelivery that BUS-D1 must dedup) — the inbox dedups by event_id.
    assert!(breaker
        .restore_dependency(Dependency::Broker, Scope::Tenant(t1.clone()))
        .changed());
    let redrive: Vec<u64> = pending.iter().copied().chain([backlog[0], backlog[1]]).collect();
    for n in redrive {
        let disp = tier.dispatch(&req(n), || EventId(format!("act-{n}")), &Timestamp("2026-06-20T00:00:01Z".into()));
        assert!(matches!(disp, Disposition::Delivered { .. }), "reconnect delivers");
    }

    // GATE: 0 lost (every distinct dispatch landed) + 0 duplicate (the at-least-once redelivery of
    // the first two landed exactly once each — effect_count == distinct count, NOT count + 2).
    assert_eq!(
        *tier.target().effect_count.borrow(),
        backlog.len() as u64,
        "0 lost AND 0 duplicate: exactly one effect per distinct dispatch"
    );
    assert_eq!(tier.target().landed.borrow().len(), backlog.len());

    // The §10.2 ConsumerLag survival signal drains to 0 after reconnect.
    let mut src = SignalSource::new();
    let lag = backlog.len() as i64 - tier.target().landed.borrow().len() as i64;
    src.set_labelled(SignalName::ConsumerLag, vec![Label::new("consumer", "dispatch-tier")], lag);
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", "dispatch-tier")],
        Predicate::Eq(0),
    )
    .expect_green();
}
