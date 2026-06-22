//! # The CDC pair for contract 3.3 — Triggers (`arm_trigger`/`disarm_trigger`) (EB-20 / P-140)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 3.3
//! (`arm_trigger`/`disarm_trigger(Trigger{owner, condition, arms_subject, on_resolve, stale_after})`
//! — the **stateful per-person promise**; `armed → {resolved | stale | disarmed}`; `condition` is a
//! `QueryAst` over projection state; `stale_after` is a `myelin-flow` durable timer). Owning
//! architecture: `event-bus.md` §1.2 (the four primitives — Trigger = the stateful per-person
//! promise, NOT the stateless Automation), §3.6 (the `trigger` store), §4.6 (the state machine +
//! the atomic guarded UPDATE fire-once-per-arming), §5.4 (the surface). ADR-19.
//!
//! ## The seam this pair pins
//! Row 3.3 is the seam between:
//! - the **PROVIDER** — the Trigger engine ([`myelin_query::TriggerEngine`], the stateful
//!   per-person-promise consumer over the matcher): a person arms a promise; the engine evaluates
//!   each arming's `condition` (permission-aware) against incoming events and, on a match, performs
//!   the atomic guarded UPDATE. Its promise: the trigger fires `on_resolve` EXACTLY ONCE per arming
//!   (only one concurrent resolving event wins the guard); `armed → stale` is DELEGATED to the
//!   `myelin-flow` timer (9.3, never reinvented); `armed → disarmed` is the owner cancel; re-arming
//!   creates a fresh arming (idempotency is per-arming).
//! - the **CONSUMER** — the Bus dispatch tier (EB-23) + the `myelin-flow` durable timer wheel: the
//!   dispatch tier reads the [`Resolution`] (the won transition + `on_resolve` + the resolving event
//!   as cause) and runs `on_resolve` carrying nested causality + records the durable transition; the
//!   durable timer wheel (the CONSUMED 9.3 seam) receives the `stale_after` `arm`/`disarm` calls.
//!   Their promise: a `Resolved` outcome carries the cause + action exactly once; the timer wheel is
//!   armed iff a `stale_after` is set (the consumer side of 9.3).
//!
//! The pair asserts both sides agree: the registration shape (`owner/condition/arms_subject/
//! on_resolve/stale_after`), the fire-once-per-arming property, and the timer delegation through
//! the durable-timer seam (the consumer side of 9.3).

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

/// `event.type == <type>` condition (the simplest projection-state condition).
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

/// **PROVIDER side of 3.3** — an `arm_trigger` arming + the Trigger engine that resolves. A
/// per-person "notify me when this issue is unblocked" promise.
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

/// The 3.3 pair, FIRE-ONCE-PER-ARMING leg: the PROVIDER resolves on a matching event and yields the
/// `Resolved` outcome (the cause + the action); under a SECOND concurrent resolving event the
/// CONSUMER reads `AlreadyResolved` (on_resolve runs ZERO more times — the atomic guarded UPDATE).
#[test]
fn cdc_3_3_fires_exactly_once_per_arming_under_concurrent_events() {
    let timer = InMemoryTimer::new();
    let (mut engine, _arming) = provider_engine(&timer, None);

    // PROVIDER: the first resolving event WINS the arming and yields Resolved (the cause carried).
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

    // CONSUMER: a SECOND concurrent resolving event LOSES the guarded UPDATE (no second on_resolve).
    let e2 = envelope("issues.issue.unblocked", "PROJ-1", "evt-b");
    let r2 = engine.on_event(&e2, &SetExpr::All, &see_all, &timer);
    assert!(
        matches!(&r2[0], Resolution::AlreadyResolved { .. }),
        "the second concurrent delivery loses the guard — fires once per arming"
    );
    // The durable state is Resolved, resolved_by the FIRST event.
    assert_eq!(
        engine
            .arming(&TriggerId("notify_on_unblock".into()))
            .unwrap()
            .resolved_by,
        Some(EventId("evt-a".into()))
    );
}

/// The 3.3 pair, STALE-DELEGATION leg (this is ALSO the **consumer side of 9.3**): the PROVIDER's
/// `stale_after` is DELEGATED to the `myelin-flow` durable timer wheel (armed through the 9.3 seam);
/// the CONSUMER (the timer wheel) receives the `arm` and, when it fires, drives `armed → stale`.
#[test]
fn cdc_3_3_stale_after_delegates_to_durable_timer_9_3() {
    let timer = InMemoryTimer::new();
    let deadline = StaleAfter("2026-06-21T00:00:00Z".into());
    let (mut engine, arming) = provider_engine(&timer, Some(deadline.clone()));

    // CONSUMER side of 9.3: the timer wheel was armed through the seam with the precomputed fire_at
    // (the stale_after deadline — DELEGATED, not reinvented in the engine).
    assert_eq!(
        timer.armed_count(),
        1,
        "the stale_after timer was armed via the 9.3 seam"
    );
    assert_eq!(timer.deadline_for(&arming), Some(deadline));

    // The durable timer fires (the minute-bucket wheel delivers the stale callback) → armed → stale.
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

/// The 3.3 pair, DISARM leg: the PROVIDER's owner cancels (`armed → disarmed`); the CONSUMER
/// (the timer wheel) receives the `disarm` so the `stale_after` never fires on a cancelled arming;
/// and a later resolving event does NOT resolve a disarmed arming (the guarded UPDATE rejects it).
#[test]
fn cdc_3_3_disarm_cancels_the_arming_and_the_timer() {
    let timer = InMemoryTimer::new();
    let (mut engine, _arming) =
        provider_engine(&timer, Some(StaleAfter("2026-06-21T00:00:00Z".into())));
    assert_eq!(timer.armed_count(), 1);

    // CONSUMER: the owner disarms → armed → disarmed; the timer wheel is disarmed via the seam.
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

    // A late resolving event does NOT resolve a disarmed arming.
    let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-late");
    let r = engine.on_event(&e, &SetExpr::All, &see_all, &timer);
    assert!(matches!(&r[0], Resolution::AlreadyResolved { .. }));
}

/// The 3.3 pair, RE-ARMING leg: re-arming the SAME trigger id mints a FRESH arming (idempotency is
/// per-arming); the re-armed promise can fire AGAIN. The CONSUMER sees two distinct armings resolve.
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

    // RE-ARM → a fresh arming (new ArmingId), and it can fire again.
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

/// The 3.3 pair, CONSUMER-of-9.3 NEGATIVE leg: a `stale_after` arm FAILURE on the durable-timer
/// seam is SURFACED to the dispatch tier (never a silent drop), so the arming can be retried/alerted.
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

/// The 3.3 pair, REGISTRATION-SHAPE leg: the frozen `Trigger{ owner, condition, arms_subject,
/// on_resolve, stale_after }` round-trips byte-stably (the durable `trigger` row the CONSUMER
/// reads), and the `condition` field is the byte-identical `QueryAst` (no drift, 13.3).
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

    // The condition field is the byte-identical QueryAst the saved-view/Search/Signal/Automation
    // consumers read (no drift, 13.3).
    let v = serde_json::to_value(&trigger).unwrap();
    let condition_predicate = &v["condition"]["predicate"];
    let bare = serde_json::to_value(type_condition("issues.issue.unblocked").predicate()).unwrap();
    assert_eq!(
        condition_predicate, &bare,
        "no QueryAst drift in the condition field"
    );
}
