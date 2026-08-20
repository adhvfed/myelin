use crate::matcher::RelMembership;
use crate::{EventMatcher, WorkflowRef};
use myelin_events::{ArtifactRef, EventEnvelope, EventId};
use myelin_identity::{PrincipalId, SetExpr};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TriggerId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArmingId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnResolve {
    Notify,
    Workflow { workflow_ref: WorkflowRef },
    Emit { emit_type: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    pub owner: PrincipalId,
    pub condition: EventMatcher,
    pub arms_subject: ArtifactRef,
    pub on_resolve: OnResolve,
    pub stale_after: Option<StaleAfter>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StaleAfter(pub String);

pub fn arm_trigger(
    owner: PrincipalId,
    condition: EventMatcher,
    arms_subject: ArtifactRef,
    on_resolve: OnResolve,
    stale_after: Option<StaleAfter>,
) -> Trigger {
    Trigger {
        owner,
        condition,
        arms_subject,
        on_resolve,
        stale_after,
    }
}

pub fn disarm_trigger(id: TriggerId) -> TriggerId {
    id
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerState {
    Armed,
    Resolved,
    Stale,
    Disarmed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerArming {
    pub trigger_id: TriggerId,
    pub arming_id: ArmingId,
    pub trigger: Trigger,
    pub state: TriggerState,
    pub resolved_by: Option<EventId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Resolved {
        trigger_id: TriggerId,
        arming_id: ArmingId,
        resolved_by: EventId,
        on_resolve: OnResolve,
        owner: PrincipalId,
        arms_subject: ArtifactRef,
    },
    AlreadyResolved {
        trigger_id: TriggerId,
        arming_id: ArmingId,
    },
}

pub trait DurableTimer {
    fn arm(&self, arming_id: &ArmingId, fire_at: &StaleAfter) -> Result<(), TimerError>;

    fn disarm(&self, arming_id: &ArmingId) -> Result<(), TimerError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerError(pub String);

#[derive(Debug, Default)]
pub struct InMemoryTimer {
    armed: std::cell::RefCell<BTreeMap<ArmingId, StaleAfter>>,
}

impl InMemoryTimer {
    pub fn new() -> InMemoryTimer {
        InMemoryTimer::default()
    }

    pub fn armed_count(&self) -> usize {
        self.armed.borrow().len()
    }

    pub fn deadline_for(&self, arming_id: &ArmingId) -> Option<StaleAfter> {
        self.armed.borrow().get(arming_id).cloned()
    }
}

impl DurableTimer for InMemoryTimer {
    fn arm(&self, arming_id: &ArmingId, fire_at: &StaleAfter) -> Result<(), TimerError> {
        self.armed
            .borrow_mut()
            .insert(arming_id.clone(), fire_at.clone());
        Ok(())
    }

    fn disarm(&self, arming_id: &ArmingId) -> Result<(), TimerError> {
        self.armed.borrow_mut().remove(arming_id);
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct TriggerEngine {
    armings: BTreeMap<TriggerId, TriggerArming>,
    next_arming: u64,
}

impl TriggerEngine {
    pub fn new() -> TriggerEngine {
        TriggerEngine::default()
    }

    pub fn arm(
        &mut self,
        trigger_id: TriggerId,
        trigger: Trigger,
        timer: &dyn DurableTimer,
    ) -> Result<ArmingId, TimerError> {
        if let Some(prev) = self.armings.get(&trigger_id) {
            if prev.state == TriggerState::Armed && prev.trigger.stale_after.is_some() {
                timer.disarm(&prev.arming_id)?;
            }
        }
        let arming_id = ArmingId(format!("{}#{}", trigger_id.0, self.next_arming));
        self.next_arming += 1;

        if let Some(deadline) = &trigger.stale_after {
            timer.arm(&arming_id, deadline)?;
        }

        self.armings.insert(
            trigger_id.clone(),
            TriggerArming {
                trigger_id,
                arming_id: arming_id.clone(),
                trigger,
                state: TriggerState::Armed,
                resolved_by: None,
            },
        );
        Ok(arming_id)
    }

    pub fn arming(&self, trigger_id: &TriggerId) -> Option<&TriggerArming> {
        self.armings.get(trigger_id)
    }

    pub fn next_arming(&self) -> u64 {
        self.next_arming
    }

    pub fn restore_arming(&mut self, arming: TriggerArming) {
        let trigger_id = arming.trigger_id.clone();
        if let Some(n) = arming
            .arming_id
            .0
            .rsplit('#')
            .next()
            .and_then(|s| s.parse::<u64>().ok())
        {
            self.next_arming = self.next_arming.max(n + 1);
        }
        self.armings.insert(trigger_id, arming);
    }

    pub fn disarm_trigger(
        &mut self,
        trigger_id: &TriggerId,
        timer: &dyn DurableTimer,
    ) -> Result<bool, TimerError> {
        let Some(arming) = self.armings.get_mut(trigger_id) else {
            return Ok(false);
        };
        if arming.state != TriggerState::Armed {
            return Ok(false);
        }
        if arming.trigger.stale_after.is_some() {
            timer.disarm(&arming.arming_id)?;
        }
        arming.state = TriggerState::Disarmed;
        Ok(true)
    }

    pub fn on_event(
        &mut self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
        timer: &dyn DurableTimer,
    ) -> Vec<Resolution> {
        let ids: Vec<TriggerId> = self.armings.keys().cloned().collect();
        let mut resolutions = Vec::new();
        for id in &ids {
            if let Some(res) = self.try_resolve(id, envelope, visible, member_oracle, timer) {
                resolutions.push(res);
            }
        }
        resolutions
    }

    fn try_resolve(
        &mut self,
        trigger_id: &TriggerId,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
        timer: &dyn DurableTimer,
    ) -> Option<Resolution> {
        let arming = self.armings.get(trigger_id)?;

        let matched = arming
            .trigger
            .condition
            .matches(envelope, visible, member_oracle)
            .unwrap_or(false);
        if !matched {
            return None;
        }

        let arming = self.armings.get_mut(trigger_id)?;
        if arming.state != TriggerState::Armed {
            return Some(Resolution::AlreadyResolved {
                trigger_id: arming.trigger_id.clone(),
                arming_id: arming.arming_id.clone(),
            });
        }

        arming.state = TriggerState::Resolved;
        arming.resolved_by = Some(envelope.event_id.clone());
        let resolution = Resolution::Resolved {
            trigger_id: arming.trigger_id.clone(),
            arming_id: arming.arming_id.clone(),
            resolved_by: envelope.event_id.clone(),
            on_resolve: arming.trigger.on_resolve.clone(),
            owner: arming.trigger.owner.clone(),
            arms_subject: arming.trigger.arms_subject.clone(),
        };

        if arming.trigger.stale_after.is_some() {
            let _ = timer.disarm(&arming.arming_id);
        }
        Some(resolution)
    }

    pub fn on_timer_fired(&mut self, arming_id: &ArmingId) -> bool {
        let Some(arming) = self
            .armings
            .values_mut()
            .find(|a| &a.arming_id == arming_id)
        else {
            return false;
        };
        if arming.state != TriggerState::Armed {
            return false;
        }
        arming.state = TriggerState::Stale;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CmpOp, Expr, Predicate};
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Literal, ObjectType, Principal, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn var(name: &str) -> Expr {
        Expr::Var(name.into())
    }
    fn str_(s: &str) -> Expr {
        Expr::Lit(Literal::Str(s.into()))
    }

    fn owner() -> PrincipalId {
        PrincipalId("alice".into())
    }

    fn type_condition(object_type: &str, type_: &str) -> EventMatcher {
        EventMatcher::compile(
            ObjectType(object_type.into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("event.type"),
                rhs: str_(type_),
            },
        )
        .unwrap()
    }

    fn all_blockers_resolved(object_type: &str) -> EventMatcher {
        EventMatcher::compile(
            ObjectType(object_type.into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("payload.blocked_by_unresolved"),
                rhs: Expr::Lit(Literal::Int(0)),
            },
        )
        .unwrap()
    }

    fn envelope(type_: &str, id: &str, event_id: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(event_id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("svc-bot".into()),
                PrincipalKind::Human,
                TenantId("t1".into()),
            )),
            subject: ArtifactRef(format!("myelin://t1/issues/issue/{id}")),
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

    fn no_rel(_m: &RelMembership) -> bool {
        false
    }

    fn notify_trigger(on_type: &str, stale_after: Option<StaleAfter>) -> Trigger {
        arm_trigger(
            owner(),
            type_condition("issue", on_type),
            ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
            OnResolve::Notify,
            stale_after,
        )
    }

    #[test]
    fn fires_exactly_once_per_arming_under_concurrent_events() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t-block".into()),
                notify_trigger("issues.issue.unblocked", None),
                &timer,
            )
            .unwrap();

        let e1 = envelope("issues.issue.unblocked", "PROJ-1", "evt-resolve-a");
        let e2 = envelope("issues.issue.unblocked", "PROJ-1", "evt-resolve-b");

        let r1 = engine.on_event(&e1, &SetExpr::All, &no_rel, &timer);
        let r2 = engine.on_event(&e2, &SetExpr::All, &no_rel, &timer);

        assert_eq!(r1.len(), 1);
        assert_eq!(r2.len(), 1);
        assert!(
            matches!(&r1[0], Resolution::Resolved { resolved_by, .. } if resolved_by.0 == "evt-resolve-a"),
            "the first delivery won the arming and runs on_resolve once"
        );
        assert!(
            matches!(&r2[0], Resolution::AlreadyResolved { .. }),
            "the second delivery LOST the guarded UPDATE - on_resolve does not run again"
        );
        let arming = engine.arming(&TriggerId("t-block".into())).unwrap();
        assert_eq!(arming.state, TriggerState::Resolved);
        assert_eq!(arming.resolved_by, Some(EventId("evt-resolve-a".into())));
    }

    #[test]
    fn armed_to_stale_delegates_to_myelin_flow_timer() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        let deadline = StaleAfter("2026-06-21T00:00:00Z".into());
        let arming_id = engine
            .arm(
                TriggerId("t-stale".into()),
                notify_trigger("issues.issue.unblocked", Some(deadline.clone())),
                &timer,
            )
            .unwrap();

        assert_eq!(
            timer.armed_count(),
            1,
            "the stale_after timer was armed via the seam"
        );
        assert_eq!(timer.deadline_for(&arming_id), Some(deadline));

        assert!(
            engine.on_timer_fired(&arming_id),
            "the timer fired armed → stale"
        );
        assert_eq!(
            engine.arming(&TriggerId("t-stale".into())).unwrap().state,
            TriggerState::Stale
        );
    }

    #[test]
    fn stale_timer_loses_to_a_prior_resolve() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        let arming_id = engine
            .arm(
                TriggerId("t".into()),
                notify_trigger(
                    "issues.issue.unblocked",
                    Some(StaleAfter("2026-06-21T00:00:00Z".into())),
                ),
                &timer,
            )
            .unwrap();

        let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-resolve");
        let r = engine.on_event(&e, &SetExpr::All, &no_rel, &timer);
        assert!(matches!(&r[0], Resolution::Resolved { .. }));
        assert_eq!(
            timer.armed_count(),
            0,
            "the resolve disarmed the stale_after timer"
        );

        assert!(
            !engine.on_timer_fired(&arming_id),
            "the late timer loses to the resolve"
        );
        assert_eq!(
            engine.arming(&TriggerId("t".into())).unwrap().state,
            TriggerState::Resolved
        );
    }

    #[test]
    fn armed_to_disarmed_on_owner_cancel() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t".into()),
                notify_trigger(
                    "issues.issue.unblocked",
                    Some(StaleAfter("2026-06-21T00:00:00Z".into())),
                ),
                &timer,
            )
            .unwrap();
        assert_eq!(timer.armed_count(), 1);

        assert!(engine
            .disarm_trigger(&TriggerId("t".into()), &timer)
            .unwrap());
        assert_eq!(
            engine.arming(&TriggerId("t".into())).unwrap().state,
            TriggerState::Disarmed
        );
        assert_eq!(
            timer.armed_count(),
            0,
            "the owner cancel disarmed the stale_after timer"
        );

        assert!(!engine
            .disarm_trigger(&TriggerId("t".into()), &timer)
            .unwrap());
        let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-late");
        let r = engine.on_event(&e, &SetExpr::All, &no_rel, &timer);
        assert!(matches!(&r[0], Resolution::AlreadyResolved { .. }));
    }

    #[test]
    fn re_arming_creates_a_fresh_arming() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        let id = TriggerId("t".into());

        let a1 = engine
            .arm(
                id.clone(),
                notify_trigger("issues.issue.unblocked", None),
                &timer,
            )
            .unwrap();
        let e1 = envelope("issues.issue.unblocked", "PROJ-1", "evt-1");
        let r1 = engine.on_event(&e1, &SetExpr::All, &no_rel, &timer);
        assert!(matches!(&r1[0], Resolution::Resolved { .. }));

        let a2 = engine
            .arm(
                id.clone(),
                notify_trigger("issues.issue.unblocked", None),
                &timer,
            )
            .unwrap();
        assert_ne!(
            a1, a2,
            "re-arming mints a fresh ArmingId (idempotency is per-arming)"
        );
        assert_eq!(engine.arming(&id).unwrap().state, TriggerState::Armed);

        let e2 = envelope("issues.issue.unblocked", "PROJ-1", "evt-2");
        let r2 = engine.on_event(&e2, &SetExpr::All, &no_rel, &timer);
        assert!(
            matches!(&r2[0], Resolution::Resolved { arming_id, resolved_by, .. }
                if arming_id == &a2 && resolved_by.0 == "evt-2"),
            "the re-armed promise fires again on its own arming"
        );
    }

    #[test]
    fn unviewable_subject_never_resolves_the_trigger() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t".into()),
                notify_trigger("issues.issue.unblocked", None),
                &timer,
            )
            .unwrap();
        let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-hidden");
        let r = engine.on_event(&e, &SetExpr::None, &no_rel, &timer);
        assert!(
            r.is_empty(),
            "an unviewable subject never resolves (0-leak)"
        );
        assert_eq!(
            engine.arming(&TriggerId("t".into())).unwrap().state,
            TriggerState::Armed,
            "the arming stays armed - no leak-driven resolution"
        );
    }

    #[test]
    fn all_blockers_resolved_projection_state_condition() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t-blockers".into()),
                arm_trigger(
                    owner(),
                    all_blockers_resolved("issue"),
                    ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
                    OnResolve::Notify,
                    None,
                ),
                &timer,
            )
            .unwrap();

        let mut partial = envelope("issues.issue.relation_resolved", "PROJ-1", "evt-partial");
        partial.payload = serde_json::json!({ "blocked_by_unresolved": 1 });
        let r0 = engine.on_event(&partial, &SetExpr::All, &no_rel, &timer);
        assert!(
            r0.is_empty(),
            "a partial resolution does not fire the trigger"
        );

        let mut done = envelope("issues.issue.relation_resolved", "PROJ-1", "evt-done");
        done.payload = serde_json::json!({ "blocked_by_unresolved": 0 });
        let r1 = engine.on_event(&done, &SetExpr::All, &no_rel, &timer);
        assert!(
            matches!(&r1[0], Resolution::Resolved { .. }),
            "the projection-state condition resolves when all blockers clear"
        );
    }

    #[test]
    fn resolution_carries_cause_owner_and_action() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t-wf".into()),
                arm_trigger(
                    owner(),
                    type_condition("issue", "issues.issue.unblocked"),
                    ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
                    OnResolve::Workflow {
                        workflow_ref: WorkflowRef("notify_owner".into()),
                    },
                    None,
                ),
                &timer,
            )
            .unwrap();
        let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-cause");
        let r = engine.on_event(&e, &SetExpr::All, &no_rel, &timer);
        match &r[0] {
            Resolution::Resolved {
                resolved_by,
                on_resolve,
                owner: o,
                arms_subject,
                ..
            } => {
                assert_eq!(
                    resolved_by.0, "evt-cause",
                    "the resolving event is the cause"
                );
                assert_eq!(o, &owner());
                assert_eq!(
                    arms_subject,
                    &ArtifactRef("myelin://t1/issues/issue/PROJ-1".into())
                );
                assert!(matches!(
                    on_resolve,
                    OnResolve::Workflow { workflow_ref } if workflow_ref.0 == "notify_owner"
                ));
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn on_event_is_replay_deterministic() {
        let stream: Vec<EventEnvelope> = (0..5)
            .map(|i| envelope("issues.issue.unblocked", "PROJ-1", &format!("evt-{i}")))
            .collect();
        let final_state = || {
            let mut e = TriggerEngine::new();
            let timer = InMemoryTimer::new();
            e.arm(
                TriggerId("t".into()),
                notify_trigger("issues.issue.unblocked", None),
                &timer,
            )
            .unwrap();
            for env in &stream {
                e.on_event(env, &SetExpr::All, &no_rel, &timer);
            }
            e.arming(&TriggerId("t".into())).unwrap().clone()
        };
        let a = final_state();
        let b = final_state();
        assert_eq!(a.state, b.state);
        assert_eq!(a.resolved_by, b.resolved_by);
        assert_eq!(a.state, TriggerState::Resolved);
        assert_eq!(a.resolved_by, Some(EventId("evt-0".into())));
    }

    #[test]
    fn trigger_round_trips_stably() {
        let trigger = arm_trigger(
            owner(),
            all_blockers_resolved("issue"),
            ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
            OnResolve::Emit {
                emit_type: "issues.issue.all_blockers_cleared".into(),
            },
            Some(StaleAfter("2026-06-30T00:00:00Z".into())),
        );
        let json = serde_json::to_string(&trigger).unwrap();
        let back: Trigger = serde_json::from_str(&json).unwrap();
        assert_eq!(trigger, back);
    }

    #[test]
    fn timer_arm_failure_is_surfaced() {
        struct FailingTimer;
        impl DurableTimer for FailingTimer {
            fn arm(&self, _a: &ArmingId, _f: &StaleAfter) -> Result<(), TimerError> {
                Err(TimerError("myelin-flow timer wheel unreachable".into()))
            }
            fn disarm(&self, _a: &ArmingId) -> Result<(), TimerError> {
                Ok(())
            }
        }
        let mut engine = TriggerEngine::new();
        let res = engine.arm(
            TriggerId("t".into()),
            notify_trigger(
                "issues.issue.unblocked",
                Some(StaleAfter("2026-06-21T00:00:00Z".into())),
            ),
            &FailingTimer,
        );
        assert_eq!(
            res,
            Err(TimerError("myelin-flow timer wheel unreachable".into())),
            "a stale_after arm failure is surfaced, never swallowed"
        );
    }

    #[test]
    fn timer_disarm_failure_leaves_the_trigger_armed_for_retry() {
        struct RefusingDisarm;
        impl DurableTimer for RefusingDisarm {
            fn arm(&self, _arming: &ArmingId, _fire_at: &StaleAfter) -> Result<(), TimerError> {
                Ok(())
            }

            fn disarm(&self, _arming: &ArmingId) -> Result<(), TimerError> {
                Err(TimerError("myelin-flow timer wheel unreachable".into()))
            }
        }

        let id = TriggerId("keep-armed-until-durable-disarm".into());
        let mut engine = TriggerEngine::new();
        engine
            .arm(
                id.clone(),
                notify_trigger(
                    "issues.issue.unblocked",
                    Some(StaleAfter("2026-06-21T00:00:00Z".into())),
                ),
                &RefusingDisarm,
            )
            .expect("the trigger arms before the timer outage");

        let error = engine
            .disarm_trigger(&id, &RefusingDisarm)
            .expect_err("an unconfirmed durable disarm cannot look successful");

        assert_eq!(
            error,
            TimerError("myelin-flow timer wheel unreachable".into())
        );
        assert_eq!(
            engine.arming(&id).map(|arming| arming.state),
            Some(TriggerState::Armed),
            "the owner can safely retry instead of losing track of the live timer"
        );
    }
}
