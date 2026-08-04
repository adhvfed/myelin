use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Literal, Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_issues::{
    default_stale_after, ArmRequest, ArmableCondition, IssueTriggerEngine, TriggerInboxKind,
    VAR_BLOCKED_BY_UNRESOLVED,
};
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
fn subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/issues/issue/PROJ-7".into())
}
fn no_rel(_m: &RelMembership) -> bool {
    false
}

fn unblock_event(event_id: &str, blockers_open: i64) -> EventEnvelope {
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
        subject: subject(),
        aggregate: AggregateKey("issue:PROJ-7".into()),
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

#[test]
fn producer_catalogue_compiles_to_the_frozen_querast_matcher() {
    let m = ArmableCondition::RemindWhenUnblocked.to_matcher(&subject());
    assert_eq!(VAR_BLOCKED_BY_UNRESOLVED, "payload.blocked_by_unresolved");
    assert!(m
        .matches(&unblock_event("e", 0), &SetExpr::All, &no_rel)
        .unwrap());
    assert!(!m
        .matches(&unblock_event("e", 2), &SetExpr::All, &no_rel)
        .unwrap());

    let _ = ArmableCondition::PingWhenLeavesState {
        x: "unstarted".into(),
    }
    .to_matcher(&subject());
    let _ = ArmableCondition::NotifyWhenAssignedToMe { me: "alice".into() }.to_matcher(&subject());
    let _ = ArmableCondition::TellWhenSlaAtRisk.to_matcher(&subject());
    let _ = ArmableCondition::TellWhenInitiativeAtRisk.to_matcher(&subject());
    assert_eq!(
        ArmableCondition::RemindWhenUnblocked.resolve_reason(),
        Reason::Unblocked
    );
    assert_eq!(
        ArmableCondition::TellWhenSlaAtRisk.resolve_reason(),
        Reason::Sla
    );
}

#[test]
fn consumer_bus_engine_fires_once_per_arming_and_delivers_one_inbox_item() {
    let mut eng = IssueTriggerEngine::new(tenant(), region());
    eng.arm(ArmRequest {
        trigger_id: TriggerId("t".into()),
        condition: ArmableCondition::RemindWhenUnblocked,
        owner: owner(),
        arms_subject: subject(),
        stale_after: Some(default_stale_after(0)),
    })
    .unwrap();

    assert_eq!(
        eng.on_event(&unblock_event("e1", 0), &SetExpr::All, &no_rel),
        1
    );
    assert_eq!(eng.delivered().len(), 1);
    assert_eq!(eng.delivered()[0].kind, TriggerInboxKind::Resolved);
    assert_eq!(
        eng.arming(&TriggerId("t".into())).unwrap().state,
        TriggerState::Resolved
    );
    assert_eq!(
        eng.on_event(&unblock_event("e2", 0), &SetExpr::All, &no_rel),
        0
    );
    assert_eq!(eng.delivered().len(), 1, "still exactly one fire");
}

#[test]
fn consumer_disarm_trigger_cancels_the_promise() {
    let mut eng = IssueTriggerEngine::new(tenant(), region());
    let id = TriggerId("t".into());
    eng.arm(ArmRequest {
        trigger_id: id.clone(),
        condition: ArmableCondition::RemindWhenUnblocked,
        owner: owner(),
        arms_subject: subject(),
        stale_after: Some(default_stale_after(0)),
    })
    .unwrap();
    assert!(eng.disarm(&id).unwrap(), "the owner cancel disarms");
    assert_eq!(eng.arming(&id).unwrap().state, TriggerState::Disarmed);
    assert_eq!(
        eng.on_event(&unblock_event("e", 0), &SetExpr::All, &no_rel),
        0
    );
    assert!(eng.delivered().is_empty());
}

#[test]
fn condition_fails_closed_on_a_type_mismatch() {
    let mut eng = IssueTriggerEngine::new(tenant(), region());
    eng.arm(ArmRequest {
        trigger_id: TriggerId("t".into()),
        condition: ArmableCondition::RemindWhenUnblocked,
        owner: owner(),
        arms_subject: subject(),
        stale_after: None,
    })
    .unwrap();
    let mut e = unblock_event("e", 0);
    e.payload = serde_json::json!({ "blocked_by_unresolved": "zero" });
    let _ = Literal::Int(0);
    assert_eq!(
        eng.on_event(&e, &SetExpr::All, &no_rel),
        0,
        "a type-mismatched projection value does not fire (fail-closed, 3.4)"
    );
}
