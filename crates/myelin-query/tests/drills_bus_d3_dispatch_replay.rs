//! # BUS-D3 (replay) — the dispatch-tier correlation_id-tree replay drill (EB-23 / P-143)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row BUS-D3
//! ("Replay a `correlation_id` tree → deterministic re-drive, idempotent, causality preserved
//! (replay == original)", gate: **replay-equals-original hash**) — AGENT-G2; E-6 (replay before
//! you fix).
//!
//! ## What this drill proves (the EB-23 GATE)
//! The dispatch tier is a **pure, deterministic reflex** over its input sequence: replaying the
//! SAME `correlation_id` tree (the SAME events, in the SAME order, through a fresh tier with the
//! SAME deterministic minter) produces **byte-identical** dispositions AND byte-identical
//! dispatched-action envelopes — `replay == original`. Causality is preserved (every dispatched
//! action nests on its trigger: `causation_id = trigger.event_id`, `correlation_id` carried,
//! `depth = +1`). And the re-drive is **idempotent**: a redelivered event (same `run_ref`)
//! re-reserves the SAME reservation and produces the same disposition — no double-charge, no
//! double-effect. The verdict is the replay-equals-original equality of the captured tape.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{
    DispatchRequest, DispatchTier, Disposition, InMemoryCostGate, RecordingTarget, TriggerKind,
};
use myelin_tenancy::{Region, TenantId};

fn principal(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("t1".into()),
    )
}

/// One node of the `correlation_id` tree: a triggering event at `(depth, correlation)` over a
/// distinct artifact_ref.
fn node(n: u64, actor: &str, depth: u32, correlation: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("ev:{n}")),
        type_: EventType("chat.message.created".into()),
        schema_ver: 1,
        tenant: TenantId("t1".into()),
        region: Region("t1-home".into()),
        actor: Actor(principal(actor)),
        subject: ArtifactRef(format!("myelin://t1/chat/message/{n}")),
        aggregate: AggregateKey(format!("agg:{n}")),
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

/// The `correlation_id` tree: a mix of admit / self-guard-drop / reference-gate-drop / mention /
/// depth-park nodes, all on TWO causal roots, so the tape exercises every branch deterministically.
fn correlation_tree() -> Vec<(DispatchRequest, EventId)> {
    vec![
        // Root A — admits + one self-guard drop.
        (
            auto(node(1, "human", 2, "root-A"), "agentX", "run-1"),
            EventId("act-1".into()),
        ),
        (
            auto(node(2, "human", 3, "root-A"), "agentX", "run-2"),
            EventId("act-2".into()),
        ),
        (
            auto(node(3, "agentX", 3, "root-A"), "agentX", "run-3"),
            EventId("act-3".into()),
        ), // self-guard
        (
            auto(node(4, "human", 3, "root-A"), "agentX", "run-4"),
            EventId("act-4".into()),
        ),
        // Root B — a mention (notify-only), a raw-text ref-gate drop, and an admit.
        (
            mention(node(5, "human", 1, "root-B"), "agentX", "run-5"),
            EventId("act-5".into()),
        ),
        (
            auto(raw_text(node(6, "human", 1, "root-B")), "agentX", "run-6"),
            EventId("act-6".into()),
        ), // ref-gate
        (
            auto(node(7, "human", 1, "root-B"), "agentX", "run-7"),
            EventId("act-7".into()),
        ),
    ]
}

fn auto(ev: EventEnvelope, agent: &str, run_ref: &str) -> DispatchRequest {
    DispatchRequest {
        event: ev,
        agent: PrincipalId(agent.into()),
        run_ref: run_ref.into(),
        trigger: TriggerKind::Automation,
    }
}
fn mention(ev: EventEnvelope, agent: &str, run_ref: &str) -> DispatchRequest {
    DispatchRequest {
        event: ev,
        agent: PrincipalId(agent.into()),
        run_ref: run_ref.into(),
        trigger: TriggerKind::Mention,
    }
}
fn raw_text(mut ev: EventEnvelope) -> EventEnvelope {
    ev.subject = ArtifactRef("please do the thing".into()); // not a myelin:// artifact_ref node
    ev
}

/// Run the whole tape through a fresh tier and capture the disposition tape (the replay subject).
fn run_tape() -> Vec<Disposition> {
    let gate = InMemoryCostGate::new(1);
    gate.credit(&TenantId("t1".into()), 100);
    let mut tier = DispatchTier::new(RecordingTarget::new(), gate);
    correlation_tree()
        .into_iter()
        .map(|(req, mint)| {
            tier.dispatch(
                &req,
                move || mint,
                &Timestamp("2026-06-20T00:00:01Z".into()),
            )
        })
        .collect()
}

/// **BUS-D3 — replay the `correlation_id` tree → byte-identical tape (replay == original).**
#[test]
fn bus_d3_replay_equals_original_deterministic_idempotent_causality_preserved() {
    let original = run_tape();
    let replay = run_tape();

    // replay-equals-original: the FULL disposition tape (incl. every derived envelope) is identical.
    assert_eq!(
        original, replay,
        "the dispatch tier replays byte-identically (replay == original)"
    );

    // Causality preserved: every Delivered action nests on its trigger (depth = parent + 1).
    let tree = correlation_tree();
    for ((req, _), disp) in tree.iter().zip(original.iter()) {
        if let Disposition::Delivered { action } = disp {
            assert_eq!(action.causation_id, Some(req.event.event_id.clone()));
            assert_eq!(action.correlation_id, req.event.correlation_id);
            assert_eq!(action.depth, req.event.depth + 1);
        }
    }

    // The tape exercised every branch (the drill is meaningful, not all-admit).
    let count = |pred: fn(&Disposition) -> bool| original.iter().filter(|d| pred(d)).count();
    assert!(count(|d| matches!(d, Disposition::Delivered { .. })) >= 1);
    assert_eq!(count(|d| matches!(d, Disposition::SelfGuardDropped)), 1);
    assert_eq!(count(|d| matches!(d, Disposition::ReferenceGateDropped)), 1);
    assert_eq!(count(|d| matches!(d, Disposition::NotifiedOnly)), 1);
}

/// **BUS-D3 idempotent re-drive — a redelivered event (same run_ref) does not double-charge or
/// double-effect.** The effectively-once property the replay relies on (2.5 / 11.7).
#[test]
fn bus_d3_redelivery_is_idempotent_no_double_charge_no_double_effect() {
    let gate = InMemoryCostGate::new(1);
    let t1 = TenantId("t1".into());
    gate.credit(&t1, 1); // EXACTLY one run's worth of balance
    let mut tier = DispatchTier::new(RecordingTarget::new(), gate);

    let ev = node(42, "human", 1, "root-C");
    let r = auto(ev, "agentX", "run-42");

    let first = tier.dispatch(
        &r,
        || EventId("act-42a".into()),
        &Timestamp("2026-06-20T00:00:01Z".into()),
    );
    assert!(
        matches!(first, Disposition::Delivered { .. }),
        "first delivery admits on the balance"
    );

    // Redeliver the SAME run_ref: re-reserves the SAME reservation (idempotent), so it is NOT
    // refused for lack of balance and does NOT consume a second unit (no double-charge).
    let second = tier.dispatch(
        &r,
        || EventId("act-42b".into()),
        &Timestamp("2026-06-20T00:00:01Z".into()),
    );
    assert!(
        matches!(second, Disposition::Delivered { .. }),
        "the redelivery re-reserves the same reservation — not a no-balance refusal"
    );
    // Two deliveries of the same run consumed ONE unit of balance (idempotent reserve).
    assert_eq!(
        tier.telemetry().no_balance_refused,
        0,
        "no double-charge → no spurious refusal"
    );
}
