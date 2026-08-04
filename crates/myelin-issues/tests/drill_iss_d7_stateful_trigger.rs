use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_issues::{
    default_stale_after, ArmRequest, ArmableCondition, IssueTriggerEngine, TriggerInboxKind,
};
use myelin_notif::router::InboxProjection;
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

fn arm_unblock(key: &str) -> ArmRequest {
    ArmRequest {
        trigger_id: TriggerId(format!("t-unblock/{key}")),
        condition: ArmableCondition::RemindWhenUnblocked,
        owner: owner(),
        arms_subject: subject(key),
        stale_after: Some(default_stale_after(0)),
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
fn iss_d7_fire_once_across_a_restart() {
    let inbox = InboxProjection::new();
    let id = TriggerId("t-unblock/PROJ-7".into());

    let mut engine = IssueTriggerEngine::with_inbox(tenant(), region(), inbox.clone());
    engine.arm(arm_unblock("PROJ-7")).unwrap();

    assert_eq!(
        engine.on_event(&unblock_event("PROJ-7", "rm-1", 1), &SetExpr::All, &no_rel),
        0,
        "a partial resolution does not fire"
    );

    let mut engine = restart(engine, &inbox);
    assert_eq!(
        engine.arming(&id).unwrap().state,
        TriggerState::Armed,
        "the promise survives the restart still armed"
    );

    assert_eq!(
        engine.on_event(&unblock_event("PROJ-7", "rm-2", 0), &SetExpr::All, &no_rel),
        1,
        "the last blocker clearing fires exactly once after the restart"
    );
    assert_eq!(
        engine.delivered().len(),
        1,
        "exactly one inbox item delivered"
    );
    assert_eq!(engine.delivered()[0].kind, TriggerInboxKind::Resolved);
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant()).len(),
        1,
        "the ONE Notif inbox carries exactly one row (7.1 - no second store)"
    );

    let mut engine = restart(engine, &inbox);
    assert_eq!(
        engine.arming(&id).unwrap().state,
        TriggerState::Resolved,
        "the durable state is Resolved (the fire-once guard survives the restart)"
    );
    assert_eq!(
        engine.on_event(
            &unblock_event("PROJ-7", "rm-replay", 0),
            &SetExpr::All,
            &no_rel
        ),
        0,
        "a re-delivered resolving event after a restart does NOT re-fire (0 duplicate)"
    );
    assert_eq!(
        engine.delivered().len(),
        0,
        "the restored engine fires nothing for an already-resolved arming"
    );
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant()).len(),
        1,
        "0 duplicate - exactly one inbox row after two restarts"
    );
}

#[test]
fn iss_d7_stale_once_across_a_restart() {
    let inbox = InboxProjection::new();
    let id = TriggerId("t-unblock/PROJ-9".into());

    let mut engine = IssueTriggerEngine::with_inbox(tenant(), region(), inbox.clone());
    engine.arm(arm_unblock("PROJ-9")).unwrap();

    let mut engine = restart(engine, &inbox);
    assert_eq!(engine.arming(&id).unwrap().state, TriggerState::Armed);

    assert!(
        engine.on_stale_timer(&id),
        "the stale nudge fires once after stale_after"
    );
    assert_eq!(engine.delivered().len(), 1);
    assert_eq!(engine.delivered()[0].kind, TriggerInboxKind::StaleNudge);
    assert_eq!(
        engine.arming(&id).unwrap().state,
        TriggerState::Stale,
        "the trigger goes stale - no silent forever-armed promise"
    );
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant()).len(),
        1,
        "exactly one stale-nudge row in the one inbox"
    );

    let mut engine = restart(engine, &inbox);
    assert_eq!(engine.arming(&id).unwrap().state, TriggerState::Stale);
    assert!(
        !engine.on_stale_timer(&id),
        "a re-fire of the wheel after stale is a no-op (stale-once, 0 duplicate)"
    );
    assert_eq!(
        engine.delivered().len(),
        0,
        "the restored engine fires no second nudge"
    );
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant()).len(),
        1,
        "still exactly one stale-nudge row after two restarts"
    );
}
