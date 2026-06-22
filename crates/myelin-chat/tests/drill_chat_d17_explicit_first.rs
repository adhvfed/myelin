//! # CHAT-D17 — a casual `@agent` mention NOTIFIES, it does NOT auto-spawn a costed run
//! (NOTIF-P22 / P-343, M4) — the explicit-first agent-dispatch boundary (contract 8.6 / CHAT-1).
//!
//! **Drill catalogue** `05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **CHAT-D17** (explicit-first): "A casual `@agent` mention → notifies the agent's inbox, does
//! NOT spawn a costed run; only an explicit action/structured trigger dispatches; reserve/settle gates
//! even the explicit run." **Threshold: 0 auto-spawn; reserve gate.** Reconciliation
//! `00-reconciliation-decisions.md` §6 (Explicit-first dispatch CONFIRM, CHAT-1, AG-6): a mention
//! notifies, does not auto-spawn a costed run; implicit auto-dispatch is L-3 (counsel-gated).
//! **VISION §3** (agent-native — agents have inboxes; a casual @agent notifies, does not spawn a
//! costed run).
//!
//! ## What this drill PROVES (the chat side wired through the REAL dispatch tier)
//! Chat owns the explicit-first CLASS decision ([`myelin_chat::glue::agent_dispatch_class`]); the Bus
//! reactive/dispatch tier ([`myelin_query::DispatchTier`], the REAL §4.7 gauntlet — not a mock)
//! CONSUMES it. This drill wires a casual `@agent` mention through that real tier and asserts:
//! - the disposition is `NotifiedOnly` (explicit-first — the agent's inbox is notified, no run);
//! - **0 actions were delivered** to the Agent Fabric inbox (0 auto-spawn — the threshold);
//! - **0 cost was reserved** (the casual mention never touches reserve/settle — a notify is free);
//! - even with ample balance, the mention STILL does not spawn (it is not a budget refusal — it is
//!   the explicit-first floor: a mention can only notify);
//! - by contrast, an EXPLICIT action (the deliberate structured trigger) DOES dispatch a costed run
//!   AND passes the reserve gate (the boundary is real on BOTH sides — notify-only ≠ "never runs").
//!
//! The named floor: the REAL Agent Fabric `EventInbox` (8.6) is AG-P4 / P-216; the REAL `CostLedger`
//! (11.7) is P-ST-16/P-103 + P-ST-19/P-146. Here the deterministic `RecordingTarget` + `InMemoryCostGate`
//! model exactly their contract so the explicit-first PROPERTY is proven structurally.

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

/// The agent the human `@mentions` (an agent is a `Principal` too — §1.4).
fn agent_id() -> PrincipalId {
    PrincipalId("agent:assistant".into())
}

/// A chat trigger event: a human posted a message whose subject is a structured chat `ArtifactRef`
/// (so the dispatch tier's reference gate PASSES — the explicit-first check is what decides the
/// outcome, not the reference gate; the casual mention is gated by explicit-first, not by being
/// unstructured).
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

/// **The chat-side CLASS decision is notify-only for a casual mention (the boundary's chat half).**
#[test]
fn chat_classifies_a_casual_mention_as_notify_only() {
    assert_eq!(
        agent_dispatch_class(CHAT_MESSAGE_MENTIONED, false),
        AgentDispatchClass::NotifyOnly,
        "chat's explicit-first decision: a casual @agent mention notifies only"
    );
}

/// **CHAT-D17 — a casual `@agent` mention → `NotifiedOnly`, 0 auto-spawn, 0 reservation.** Wired
/// through the REAL dispatch tier with AMPLE balance, so a `NotifiedOnly` is unambiguously the
/// explicit-first floor (NOT a no-balance refusal).
#[test]
fn casual_agent_mention_notifies_does_not_spawn_a_costed_run() {
    // ample balance so a NotifiedOnly can only be the explicit-first floor, never a budget refusal.
    let gate = InMemoryCostGate::new(1);
    gate.credit(&tenant(), 100);
    let mut tier = DispatchTier::new(RecordingTarget::new(), gate);

    // chat decides the class for a casual @agent mention → notify-only → TriggerKind::Mention.
    let class = agent_dispatch_class(CHAT_MESSAGE_MENTIONED, /*is_explicit_action=*/ false);
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

    // (1) the disposition is NotifiedOnly — the explicit-first floor (CHAT-1).
    assert_eq!(
        disp,
        Disposition::NotifiedOnly,
        "a casual @agent mention NOTIFIES — it does not auto-spawn a costed run"
    );
    // (2) 0 auto-spawn — NOTHING was delivered to the Agent Fabric inbox (the threshold).
    assert_eq!(
        tier.target().delivered_count(),
        0,
        "CHAT-D17 threshold: 0 auto-spawn from a casual mention"
    );
    // (3) 0 reservation — a notify never touches reserve/settle (it is free; the run that DOES NOT
    // happen never reserved). The tenant's in-flight is empty.
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
        "the mention did NOT reserve (it is notify-only, not a budget refusal — ample balance)"
    );
}

/// **The boundary is REAL on both sides — an EXPLICIT action DOES dispatch a costed run + passes the
/// reserve gate.** Notify-only is not "the agent never runs": a deliberate structured action (chat's
/// `ExplicitDispatch` → `TriggerKind::Automation`) delivers ONE action AND reserves the cost. This
/// pins that the explicit-first floor narrows ONLY casual mentions, not all agent dispatch.
#[test]
fn an_explicit_action_dispatches_a_costed_run_through_the_reserve_gate() {
    let gate = InMemoryCostGate::new(1);
    gate.credit(&tenant(), 5);
    let mut tier = DispatchTier::new(RecordingTarget::new(), gate);

    // an EXPLICIT action (a deliberate approve-reaction targeting the agent) → ExplicitDispatch.
    let class = agent_dispatch_class(CHAT_REACTION_ADDED, /*is_explicit_action=*/ true);
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

    // the explicit action delivered exactly ONE action (it DID dispatch a costed run)...
    assert!(
        matches!(disp, Disposition::Delivered { .. }),
        "an explicit action dispatches a costed run, got {disp:?}"
    );
    assert_eq!(
        tier.target().delivered_count(),
        1,
        "exactly one run dispatched for the explicit action"
    );
    // ...AND it passed the reserve gate (the balance was charged — reserve/settle gated even this run).
    assert_eq!(
        tier.telemetry().delivered,
        1,
        "the explicit run was reserved + delivered (the reserve gate, 11.7)"
    );
}

/// **The reserve gate bites even the explicit run — no balance → no execution (11.7).** With ZERO
/// balance, an explicit action is REFUSED (not delivered): reserve/settle gates even the deliberate
/// run, the last clause of CHAT-D17 ("reserve/settle gates even the explicit run").
#[test]
fn the_reserve_gate_refuses_an_explicit_run_with_no_balance() {
    let gate = InMemoryCostGate::new(1); // no credit → zero balance.
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
        "no balance → no execution (11.7) — reserve/settle gates even the explicit run"
    );
    assert_eq!(
        tier.target().delivered_count(),
        0,
        "0 delivered — the unfunded explicit run was refused, never dispatched"
    );
}
