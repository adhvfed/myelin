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

fn correlation_tree() -> Vec<(DispatchRequest, EventId)> {
    vec![
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
        ),
        (
            auto(node(4, "human", 3, "root-A"), "agentX", "run-4"),
            EventId("act-4".into()),
        ),
        (
            mention(node(5, "human", 1, "root-B"), "agentX", "run-5"),
            EventId("act-5".into()),
        ),
        (
            auto(raw_text(node(6, "human", 1, "root-B")), "agentX", "run-6"),
            EventId("act-6".into()),
        ),
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
    ev.subject = ArtifactRef("please do the thing".into());
    ev
}

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

#[test]
fn bus_d3_replay_equals_original_deterministic_idempotent_causality_preserved() {
    let original = run_tape();
    let replay = run_tape();

    assert_eq!(
        original, replay,
        "the dispatch tier replays byte-identically (replay == original)"
    );

    let tree = correlation_tree();
    for ((req, _), disp) in tree.iter().zip(original.iter()) {
        if let Disposition::Delivered { action } = disp {
            assert_eq!(action.causation_id, Some(req.event.event_id.clone()));
            assert_eq!(action.correlation_id, req.event.correlation_id);
            assert_eq!(action.depth, req.event.depth + 1);
        }
    }

    let count = |pred: fn(&Disposition) -> bool| original.iter().filter(|d| pred(d)).count();
    assert!(count(|d| matches!(d, Disposition::Delivered { .. })) >= 1);
    assert_eq!(count(|d| matches!(d, Disposition::SelfGuardDropped)), 1);
    assert_eq!(count(|d| matches!(d, Disposition::ReferenceGateDropped)), 1);
    assert_eq!(count(|d| matches!(d, Disposition::NotifiedOnly)), 1);
}

#[test]
fn bus_d3_redelivery_is_idempotent_no_double_charge_no_double_effect() {
    let gate = InMemoryCostGate::new(1);
    let t1 = TenantId("t1".into());
    gate.credit(&t1, 1);
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

    let second = tier.dispatch(
        &r,
        || EventId("act-42b".into()),
        &Timestamp("2026-06-20T00:00:01Z".into()),
    );
    assert!(
        matches!(second, Disposition::Delivered { .. }),
        "the redelivery re-reserves the same reservation - not a no-balance refusal"
    );
    assert_eq!(
        tier.telemetry().no_balance_refused,
        0,
        "no double-charge → no spurious refusal"
    );
}
