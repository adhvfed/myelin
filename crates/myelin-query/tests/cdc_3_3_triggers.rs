use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Literal, ObjectType, Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_query::{
    arm_trigger, ArmingId, CmpOp, DurableTimer, EventMatcher, Expr, InMemoryTimer, OnResolve,
    Predicate, Resolution, StaleAfter, TimerError, Trigger, TriggerEngine, TriggerId, TriggerState,
    WorkflowRef,
};
use myelin_tenancy::{Region, TenantId};

fn owner() -> PrincipalId {
    PrincipalId("alice".into())
}

fn type_condition(type_: &str) -> EventMatcher {
    EventMatcher::compile(
        ObjectType("issue".into()),
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("event.type".into()),
            rhs: Expr::Lit(Literal::Str(type_.into())),
        },
    )
    .unwrap()
}

fn envelope(type_: &str, id: &str, event_id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("svc-bot".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef(format!("myelin://acme/issues/issue/{id}")),
        aggregate: AggregateKey(format!("issue:{id}")),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

fn see_all(_m: &myelin_query::RelMembership) -> bool {
    false
}

fn provider_engine(
    timer: &dyn DurableTimer,
    stale: Option<StaleAfter>,
) -> (TriggerEngine, ArmingId) {
    let mut engine = TriggerEngine::new();
    let arming = engine
        .arm(
            TriggerId("notify_on_unblock".into()),
            arm_trigger(
                owner(),
                type_condition("issues.issue.unblocked"),
                ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
                OnResolve::Notify,
                stale,
            ),
            timer,
        )
        .unwrap();
    (engine, arming)
}

#[test]
fn cdc_3_3_fires_exactly_once_per_arming_under_concurrent_events() {
    let timer = InMemoryTimer::new();
    let (mut engine, _arming) = provider_engine(&timer, None);

    let e1 = envelope("issues.issue.unblocked", "PROJ-1", "evt-a");
    let r1 = engine.on_event(&e1, &SetExpr::All, &see_all, &timer);
    match &r1[0] {
        Resolution::Resolved {
            resolved_by,
            on_resolve,
            owner: o,
            ..
        } => {
            assert_eq!(
                resolved_by,
                &EventId("evt-a".into()),
                "the resolving event is the cause"
            );
            assert!(matches!(on_resolve, OnResolve::Notify));
            assert_eq!(o, &owner());
        }
        other => panic!("expected Resolved, got {other:?}"),
    }

    let e2 = envelope("issues.issue.unblocked", "PROJ-1", "evt-b");
    let r2 = engine.on_event(&e2, &SetExpr::All, &see_all, &timer);
    assert!(
        matches!(&r2[0], Resolution::AlreadyResolved { .. }),
        "the second concurrent delivery loses the guard - fires once per arming"
    );
    assert_eq!(
        engine
            .arming(&TriggerId("notify_on_unblock".into()))
            .unwrap()
            .resolved_by,
        Some(EventId("evt-a".into()))
    );
}

#[test]
fn cdc_3_3_stale_after_delegates_to_durable_timer_9_3() {
    let timer = InMemoryTimer::new();
    let deadline = StaleAfter("2026-06-21T00:00:00Z".into());
    let (mut engine, arming) = provider_engine(&timer, Some(deadline.clone()));

    assert_eq!(
        timer.armed_count(),
        1,
        "the stale_after timer was armed via the 9.3 seam"
    );
    assert_eq!(timer.deadline_for(&arming), Some(deadline));

    assert!(
        engine.on_timer_fired(&arming),
        "the timer firing drives armed → stale"
    );
    assert_eq!(
        engine
            .arming(&TriggerId("notify_on_unblock".into()))
            .unwrap()
            .state,
        TriggerState::Stale
    );
}

#[test]
fn cdc_3_3_disarm_cancels_the_arming_and_the_timer() {
    let timer = InMemoryTimer::new();
    let (mut engine, _arming) =
        provider_engine(&timer, Some(StaleAfter("2026-06-21T00:00:00Z".into())));
    assert_eq!(timer.armed_count(), 1);

    assert!(engine
        .disarm_trigger(&TriggerId("notify_on_unblock".into()), &timer)
        .unwrap());
    assert_eq!(
        engine
            .arming(&TriggerId("notify_on_unblock".into()))
            .unwrap()
            .state,
        TriggerState::Disarmed
    );
    assert_eq!(
        timer.armed_count(),
        0,
        "the disarm cancelled the stale_after timer"
    );

    let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-late");
    let r = engine.on_event(&e, &SetExpr::All, &see_all, &timer);
    assert!(matches!(&r[0], Resolution::AlreadyResolved { .. }));
}

#[test]
fn cdc_3_3_re_arming_creates_a_fresh_arming() {
    let timer = InMemoryTimer::new();
    let id = TriggerId("notify_on_unblock".into());

    let mut engine = TriggerEngine::new();
    let a1 = engine
        .arm(
            id.clone(),
            arm_trigger(
                owner(),
                type_condition("issues.issue.unblocked"),
                ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
                OnResolve::Notify,
                None,
            ),
            &timer,
        )
        .unwrap();
    let r1 = engine.on_event(
        &envelope("issues.issue.unblocked", "PROJ-1", "evt-1"),
        &SetExpr::All,
        &see_all,
        &timer,
    );
    assert!(matches!(&r1[0], Resolution::Resolved { .. }));

    let a2 = engine
        .arm(
            id.clone(),
            arm_trigger(
                owner(),
                type_condition("issues.issue.unblocked"),
                ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
                OnResolve::Notify,
                None,
            ),
            &timer,
        )
        .unwrap();
    assert_ne!(a1, a2, "re-arming mints a fresh ArmingId");
    let r2 = engine.on_event(
        &envelope("issues.issue.unblocked", "PROJ-1", "evt-2"),
        &SetExpr::All,
        &see_all,
        &timer,
    );
    assert!(
        matches!(&r2[0], Resolution::Resolved { arming_id, .. } if arming_id == &a2),
        "the re-armed promise fires again on its fresh arming"
    );
}

#[test]
fn cdc_3_3_stale_after_arm_failure_is_surfaced() {
    struct Failing;
    impl DurableTimer for Failing {
        fn arm(&self, _a: &ArmingId, _f: &StaleAfter) -> Result<(), TimerError> {
            Err(TimerError("myelin-flow timer wheel unreachable".into()))
        }
        fn disarm(&self, _a: &ArmingId) -> Result<(), TimerError> {
            Ok(())
        }
    }
    let mut engine = TriggerEngine::new();
    let res = engine.arm(
        TriggerId("notify_on_unblock".into()),
        arm_trigger(
            owner(),
            type_condition("issues.issue.unblocked"),
            ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            OnResolve::Notify,
            Some(StaleAfter("2026-06-21T00:00:00Z".into())),
        ),
        &Failing,
    );
    assert!(
        res.is_err(),
        "a stale_after arm failure is surfaced, never swallowed"
    );
}

#[test]
fn cdc_3_3_registration_shape_round_trips_stably() {
    let trigger: Trigger = arm_trigger(
        owner(),
        type_condition("issues.issue.unblocked"),
        ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
        OnResolve::Workflow {
            workflow_ref: WorkflowRef("escalate_incident".into()),
        },
        Some(StaleAfter("2026-06-30T00:00:00Z".into())),
    );
    let json = serde_json::to_string(&trigger).unwrap();
    let back: Trigger = serde_json::from_str(&json).unwrap();
    assert_eq!(trigger, back);

    let v = serde_json::to_value(&trigger).unwrap();
    let condition_predicate = &v["condition"]["predicate"];
    let bare = serde_json::to_value(type_condition("issues.issue.unblocked").predicate()).unwrap();
    assert_eq!(
        condition_predicate, &bare,
        "no QueryAst drift in the condition field"
    );
}
