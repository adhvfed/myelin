//! # The CDC pair for contract 8.6 — chat's explicit-first agent-dispatch boundary (NOTIF-P22 / P-343)
//!
//! **Contract:** `contract-index.md` row **8.6** (`EventInbox::deliver(InboxEvent)` — platform
//! delivers matched events; **explicit-first dispatch** (CHAT-1): a mention notifies, does not
//! auto-spawn a costed run; implicit auto-dispatch is L-3, counsel-gated). **Reconciliation:**
//! `00-reconciliation-decisions.md` §6 (Explicit-first dispatch — CONFIRM, CHAT-1, AG-6).
//!
//! ## The seam this pair pins (chat decides the CLASS; the Bus dispatch tier enforces the boundary)
//! - **PROVIDER (chat — [`myelin_chat::glue::agent_dispatch_class`])** decides, per chat event,
//!   whether it is a casual `@agent` mention (explicit-first → NOTIFY only) or an explicit dispatch
//!   action. The provider's promise: a casual `chat.message.mentioned` is ALWAYS notify-only — chat
//!   never asks the platform to auto-spawn a costed run from a casual mention.
//! - **CONSUMER (the Bus dispatch tier — [`myelin_query::DispatchTier`], 8.6 / §4.7)** consumes that
//!   class as a `TriggerKind` and ENFORCES the boundary: a `Mention` → `Disposition::NotifiedOnly`
//!   (no reservation, no inbox dispatch of a run); an `Automation` → a guarded costed run. The
//!   consumer's promise: a mention can ONLY notify (0 auto-spawn), and the explicit-first check fires
//!   BEFORE reserve/settle (a notify is free).
//!
//! The dispatch tier lives in `myelin-query` (the Bus's reactive/dispatch tier — the documented §2.9
//! deviation; the real Agent Fabric `EventInbox`/8.6 is AG-P4/P-216). This pair asserts both sides
//! agree: chat's `NotifyOnly` class maps to the tier's `NotifiedOnly` disposition with 0 delivery.

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

/// **PROVIDER side of 8.6** — chat's explicit-first class decision, mapped to the frozen
/// `TriggerKind` the dispatch tier consumes. The provider's promise: a casual mention → notify-only.
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

/// **CONSUMER side of 8.6** — the Bus dispatch tier consumes chat's class and returns the boundary
/// disposition. The consumer's promise: a `Mention` notifies-only (0 delivery); the explicit-first
/// check fires before reserve.
fn consumer_disposition(token: &str, is_explicit_action: bool) -> (Disposition, usize) {
    let gate = InMemoryCostGate::new(1);
    gate.credit(&tenant(), 100); // ample balance — a NotifiedOnly is unambiguously explicit-first.
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

/// The 8.6 pair, end-to-end: the PROVIDER (chat) classifies a casual mention as notify-only and the
/// CONSUMER (the dispatch tier) enforces `NotifiedOnly` with 0 auto-spawn — the dated green artifact
/// for the chat side of the explicit-first boundary (the contract-coverage scanner's 8.6 chat row).
#[test]
fn cdc_8_6_chat_casual_mention_provider_notify_only_consumer_does_not_spawn() {
    // PROVIDER: a casual @agent mention is notify-only.
    assert_eq!(
        agent_dispatch_class(CHAT_MESSAGE_MENTIONED, false),
        AgentDispatchClass::NotifyOnly,
        "PROVIDER: chat classifies a casual mention as notify-only"
    );
    // CONSUMER: the dispatch tier enforces NotifiedOnly + 0 auto-spawn.
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

/// The complement of the pair: the PROVIDER classifies an EXPLICIT action as a dispatch, and the
/// CONSUMER DOES dispatch a costed run (1 delivered) — the boundary is real on both sides.
#[test]
fn cdc_8_6_chat_explicit_action_provider_dispatch_consumer_spawns_a_run() {
    // PROVIDER: a deliberate explicit action is a dispatch.
    assert_eq!(
        agent_dispatch_class(CHAT_REACTION_ADDED, true),
        AgentDispatchClass::ExplicitDispatch,
        "PROVIDER: chat classifies an explicit action as a dispatch"
    );
    // CONSUMER: the dispatch tier delivers exactly one costed run.
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
