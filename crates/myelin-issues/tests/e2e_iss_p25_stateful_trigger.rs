use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_issues::{
    default_stale_after, ArmRequest, ArmableCondition, IssueTriggerEngine, TriggerInboxKind,
};
use myelin_notif::router::InboxProjection;
use myelin_notif::Reason;
use myelin_query::{RelMembership, TriggerId, TriggerState};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn owner() -> PrincipalId {
    PrincipalId("alice".into())
}
fn subject(key: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/issues/issue/{key}"))
}
fn no_rel(_m: &RelMembership) -> bool {
    false
}

fn unblock_event(key: &str, event_id: &str, blockers_open: i64) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("issue.relation.removed".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("svc-bot".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: subject(key),
        aggregate: AggregateKey(format!("issue:{key}")),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:00Z".into()),
        payload: serde_json::json!({ "blocked_by_unresolved": blockers_open }),
    }
}

fn restart(engine: IssueTriggerEngine, inbox: &InboxProjection) -> IssueTriggerEngine {
    let snap = engine.snapshot();
    let meta = engine.meta_for_snapshot();
    drop(engine);
    let mut restored = IssueTriggerEngine::restore(tenant(), region(), inbox.clone(), snap);
    for (tid, cond, ow, subj) in meta {
        restored.restore_meta(tid, cond, ow, subj);
    }
    restored
}

#[test]
fn stateful_trigger_lifecycle_resolve_and_stale() {
    let inbox = InboxProjection::new();

    let resolve_id = TriggerId("t-unblock/PROJ-1".into());
    let stale_id = TriggerId("t-unblock/PROJ-2".into());

    let mut engine = IssueTriggerEngine::with_inbox(tenant(), region(), inbox.clone());

    engine
        .arm(ArmRequest {
            trigger_id: resolve_id.clone(),
            condition: ArmableCondition::RemindWhenUnblocked,
            owner: owner(),
            arms_subject: subject("PROJ-1"),
            stale_after: Some(default_stale_after(0)),
        })
        .unwrap();
    engine
        .arm(ArmRequest {
            trigger_id: stale_id.clone(),
            condition: ArmableCondition::RemindWhenUnblocked,
            owner: owner(),
            arms_subject: subject("PROJ-2"),
            stale_after: Some(default_stale_after(0)),
        })
        .unwrap();

    assert_eq!(
        engine.on_event(
            &unblock_event("PROJ-1", "p1-rm1", 1),
            &SetExpr::All,
            &no_rel
        ),
        0
    );

    let mut engine = restart(engine, &inbox);
    assert_eq!(
        engine.arming(&resolve_id).unwrap().state,
        TriggerState::Armed
    );
    assert_eq!(engine.arming(&stale_id).unwrap().state, TriggerState::Armed);

    assert_eq!(
        engine.on_event(
            &unblock_event("PROJ-1", "p1-rm2", 0),
            &SetExpr::All,
            &no_rel
        ),
        1,
        "1 fire"
    );
    assert_eq!(
        engine.arming(&resolve_id).unwrap().state,
        TriggerState::Resolved
    );

    assert!(engine.on_stale_timer(&stale_id), "1 stale nudge");
    assert_eq!(
        engine.arming(&stale_id).unwrap().state,
        TriggerState::Stale,
        "the unresolved promise goes stale (no silent forever-armed promise)"
    );

    let rows = inbox.snapshot_for_tenant(&tenant());
    assert_eq!(
        rows.len(),
        2,
        "two inbox rows: one resolve, one stale nudge"
    );

    let kinds: Vec<TriggerInboxKind> = engine.delivered().iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == TriggerInboxKind::Resolved)
            .count(),
        1
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == TriggerInboxKind::StaleNudge)
            .count(),
        1
    );
    let resolved = engine
        .delivered()
        .iter()
        .find(|d| d.kind == TriggerInboxKind::Resolved)
        .unwrap();
    assert_eq!(resolved.reason, Reason::Unblocked);
    assert_eq!(resolved.subject, subject("PROJ-1"));

    let mut engine = restart(engine, &inbox);
    assert_eq!(
        engine.on_event(
            &unblock_event("PROJ-1", "replay", 0),
            &SetExpr::All,
            &no_rel
        ),
        0,
        "0 duplicate resolve after restart"
    );
    assert!(!engine.on_stale_timer(&stale_id), "0 duplicate stale nudge");
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant()).len(),
        2,
        "still exactly two rows in the one inbox (0 duplicate)"
    );
}
