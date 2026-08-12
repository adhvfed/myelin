use myelin_events::{ArtifactRef, EventEnvelope};
use myelin_identity::{Literal, ObjectType, PrincipalId, SetExpr};
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::{Class, Reason};
use myelin_query::{
    arm_trigger, CmpOp, DurableTimer, EventMatcher, Expr, InMemoryTimer, Predicate, Resolution,
    StaleAfter, Trigger, TriggerArming, TriggerEngine, TriggerId, TriggerState,
};
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};

pub const DEFAULT_STALE_AFTER_DAYS: i64 = 30;

pub const VAR_BLOCKED_BY_UNRESOLVED: &str = "payload.blocked_by_unresolved";

pub const VAR_STATE_CATEGORY: &str = "payload.state_category";

pub const VAR_ASSIGNEE: &str = "payload.assignee";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArmableCondition {
    RemindWhenUnblocked,
    PingWhenLeavesState { x: String },
    NotifyWhenAssignedToMe { me: String },
    TellWhenSlaAtRisk,
    TellWhenInitiativeAtRisk,
}

impl ArmableCondition {
    fn base_predicate(&self) -> Predicate {
        match self {
            ArmableCondition::RemindWhenUnblocked => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(VAR_BLOCKED_BY_UNRESOLVED.into()),
                rhs: Expr::Lit(Literal::Int(0)),
            },
            ArmableCondition::PingWhenLeavesState { x } => Predicate::Cmp {
                op: CmpOp::Ne,
                lhs: Expr::Var(VAR_STATE_CATEGORY.into()),
                rhs: Expr::Lit(Literal::Str(x.clone())),
            },
            ArmableCondition::NotifyWhenAssignedToMe { me } => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(VAR_ASSIGNEE.into()),
                rhs: Expr::Lit(Literal::Str(me.clone())),
            },
            ArmableCondition::TellWhenSlaAtRisk => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("event.type".into()),
                rhs: Expr::Lit(Literal::Str("issue.sla.at_risk".into())),
            },
            ArmableCondition::TellWhenInitiativeAtRisk => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("event.type".into()),
                rhs: Expr::Lit(Literal::Str("issue.initiative.health_changed".into())),
            },
        }
    }

    pub fn to_matcher(&self, arms_subject: &ArtifactRef) -> EventMatcher {
        let scoped = Predicate::And(vec![
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("event.subject".into()),
                rhs: Expr::Lit(Literal::Str(arms_subject.0.clone())),
            },
            self.base_predicate(),
        ]);
        EventMatcher::compile(ObjectType("issue".into()), scoped)
            .expect("the scoped catalogue predicate is within the cost budget")
    }

    pub fn resolve_reason(&self) -> Reason {
        match self {
            ArmableCondition::RemindWhenUnblocked => Reason::Unblocked,
            ArmableCondition::PingWhenLeavesState { .. } => Reason::StateChanged,
            ArmableCondition::NotifyWhenAssignedToMe { .. } => Reason::Assigned,
            ArmableCondition::TellWhenSlaAtRisk => Reason::Sla,
            ArmableCondition::TellWhenInitiativeAtRisk => Reason::StateChanged,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerInboxItem {
    pub kind: TriggerInboxKind,
    pub recipient: PrincipalId,
    pub subject: ArtifactRef,
    pub reason: Reason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerInboxKind {
    Resolved,
    StaleNudge,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSnapshot {
    pub armings: Vec<TriggerArming>,
    pub next_arming: u64,
    pub next_timer: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmRequest {
    pub trigger_id: TriggerId,
    pub condition: ArmableCondition,
    pub owner: PrincipalId,
    pub arms_subject: ArtifactRef,
    pub stale_after: Option<StaleAfter>,
}

pub struct IssueTriggerEngine {
    tenant: TenantId,
    region: Region,
    engine: TriggerEngine,
    timer: InMemoryTimer,
    inbox: InboxProjection,
    next_arming: u64,
    next_timer: u64,
    meta: std::collections::BTreeMap<TriggerId, ArmMeta>,
    delivered: Vec<TriggerInboxItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ArmMeta {
    condition: ArmableCondition,
    owner: PrincipalId,
    arms_subject: ArtifactRef,
}

impl IssueTriggerEngine {
    pub fn new(tenant: TenantId, region: Region) -> IssueTriggerEngine {
        IssueTriggerEngine::with_inbox(tenant, region, InboxProjection::new())
    }

    pub fn with_inbox(
        tenant: TenantId,
        region: Region,
        inbox: InboxProjection,
    ) -> IssueTriggerEngine {
        IssueTriggerEngine {
            tenant,
            region,
            engine: TriggerEngine::new(),
            timer: InMemoryTimer::new(),
            inbox,
            next_arming: 0,
            next_timer: 0,
            meta: std::collections::BTreeMap::new(),
            delivered: Vec::new(),
        }
    }

    pub fn inbox(&self) -> &InboxProjection {
        &self.inbox
    }

    pub fn delivered(&self) -> &[TriggerInboxItem] {
        &self.delivered
    }

    pub fn arming(&self, trigger_id: &TriggerId) -> Option<&TriggerArming> {
        self.engine.arming(trigger_id)
    }

    pub fn arm(&mut self, req: ArmRequest) -> Result<TriggerId, myelin_query::TimerError> {
        let matcher = req.condition.to_matcher(&req.arms_subject);
        let trigger: Trigger = arm_trigger(
            req.owner.clone(),
            matcher,
            req.arms_subject.clone(),
            myelin_query::OnResolve::Notify,
            req.stale_after.clone(),
        );
        self.engine
            .arm(req.trigger_id.clone(), trigger, &self.timer)?;
        self.next_arming = self.next_arming.max(self.engine_next_arming());
        if req.stale_after.is_some() {
            self.next_timer += 1;
        }
        self.meta.insert(
            req.trigger_id.clone(),
            ArmMeta {
                condition: req.condition,
                owner: req.owner,
                arms_subject: req.arms_subject,
            },
        );
        Ok(req.trigger_id)
    }

    pub fn disarm(&mut self, trigger_id: &TriggerId) -> Result<bool, myelin_query::TimerError> {
        self.engine.disarm_trigger(trigger_id, &self.timer)
    }

    pub fn on_event(
        &mut self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&myelin_query::RelMembership) -> bool,
    ) -> usize {
        let resolutions = self
            .engine
            .on_event(envelope, visible, member_oracle, &self.timer);
        let mut fired = 0usize;
        for res in resolutions {
            if let Resolution::Resolved {
                trigger_id,
                owner,
                arms_subject,
                ..
            } = res
            {
                let reason = self
                    .meta
                    .get(&trigger_id)
                    .map(|m| m.condition.resolve_reason())
                    .unwrap_or(Reason::Unblocked);
                self.deliver(
                    TriggerInboxKind::Resolved,
                    owner,
                    arms_subject,
                    reason,
                    &trigger_id,
                );
                fired += 1;
            }
        }
        fired
    }

    pub fn on_stale_timer(&mut self, trigger_id: &TriggerId) -> bool {
        let Some(arming) = self.engine.arming(trigger_id) else {
            return false;
        };
        let arming_id = arming.arming_id.clone();
        if !self.engine.on_timer_fired(&arming_id) {
            return false;
        }
        if let Some(meta) = self.meta.get(trigger_id).cloned() {
            self.deliver(
                TriggerInboxKind::StaleNudge,
                meta.owner,
                meta.arms_subject,
                meta.condition.resolve_reason(),
                trigger_id,
            );
        }
        true
    }

    pub fn snapshot(&self) -> TriggerSnapshot {
        let mut armings: Vec<TriggerArming> = self
            .meta
            .keys()
            .filter_map(|id| self.engine.arming(id).cloned())
            .collect();
        armings.sort_by(|a, b| a.trigger_id.0.cmp(&b.trigger_id.0));
        TriggerSnapshot {
            armings,
            next_arming: self.next_arming,
            next_timer: self.next_timer,
        }
    }

    pub fn restore(
        tenant: TenantId,
        region: Region,
        inbox: InboxProjection,
        snapshot: TriggerSnapshot,
    ) -> IssueTriggerEngine {
        let mut engine = IssueTriggerEngine::with_inbox(tenant, region, inbox);
        engine.next_arming = snapshot.next_arming;
        engine.next_timer = snapshot.next_timer;
        for arming in snapshot.armings {
            if arming.state == TriggerState::Armed {
                if let Some(deadline) = &arming.trigger.stale_after {
                    let _ = engine.timer.arm(&arming.arming_id, deadline);
                }
            }
            engine.engine.restore_arming(arming);
        }
        engine
    }

    pub fn restore_meta(
        &mut self,
        trigger_id: TriggerId,
        condition: ArmableCondition,
        owner: PrincipalId,
        arms_subject: ArtifactRef,
    ) {
        self.meta.insert(
            trigger_id,
            ArmMeta {
                condition,
                owner,
                arms_subject,
            },
        );
    }

    pub fn meta_for_snapshot(
        &self,
    ) -> Vec<(TriggerId, ArmableCondition, PrincipalId, ArtifactRef)> {
        self.meta
            .iter()
            .map(|(id, m)| {
                (
                    id.clone(),
                    m.condition.clone(),
                    m.owner.clone(),
                    m.arms_subject.clone(),
                )
            })
            .collect()
    }

    fn deliver(
        &mut self,
        kind: TriggerInboxKind,
        owner: PrincipalId,
        arms_subject: ArtifactRef,
        reason: Reason,
        trigger_id: &TriggerId,
    ) {
        let class = match reason {
            Reason::Sla | Reason::ApprovalRequested => Class::Critical,
            Reason::Assigned => Class::Direct,
            _ => Class::Watching,
        };
        let dedup_key = format!(
            "trigger/{}/{}",
            trigger_id.0,
            match kind {
                TriggerInboxKind::Resolved => "resolved",
                TriggerInboxKind::StaleNudge => "stale",
            }
        );
        let item_id = format!("{}/{}/{}", self.tenant.0, owner.0, dedup_key);
        self.inbox.upsert_for_test(RoutedInboxItem {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            item_id,
            recipient: owner.0.clone(),
            subject: arms_subject.clone(),
            reason,
            class,
            origin_event: arms_subject.clone(),
            dedup_key,
            coalesce_count: 0,
            state: "unread".into(),
            snooze_until: None,
        });
        self.delivered.push(TriggerInboxItem {
            kind,
            recipient: owner,
            subject: arms_subject,
            reason,
        });
    }

    fn engine_next_arming(&self) -> u64 {
        self.engine.next_arming()
    }
}

pub fn default_stale_after(now_secs: i64) -> StaleAfter {
    let fire_at = now_secs + DEFAULT_STALE_AFTER_DAYS * 24 * 3600;
    StaleAfter(epoch_secs_to_rfc3339(fire_at))
}

fn epoch_secs_to_rfc3339(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let secs_of_day = epoch_secs.rem_euclid(86_400);
    let (h, m, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalKind};

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
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            payload: serde_json::json!({ "blocked_by_unresolved": blockers_open }),
        }
    }

    fn no_rel(_m: &myelin_query::RelMembership) -> bool {
        false
    }

    fn arm_unblock(stale: Option<StaleAfter>) -> ArmRequest {
        ArmRequest {
            trigger_id: TriggerId("t-unblock".into()),
            condition: ArmableCondition::RemindWhenUnblocked,
            owner: owner(),
            arms_subject: subject(),
            stale_after: stale,
        }
    }

    #[test]
    fn flagship_compiles_to_the_unblock_predicate() {
        let m = ArmableCondition::RemindWhenUnblocked.to_matcher(&subject());
        let e0 = unblock_event("e", 0);
        let e1 = unblock_event("e", 1);
        assert!(m.matches(&e0, &SetExpr::All, &no_rel).unwrap());
        assert!(!m.matches(&e1, &SetExpr::All, &no_rel).unwrap());
        let mut other = unblock_event("e", 0);
        other.subject = ArtifactRef("myelin://acme/issues/issue/OTHER-1".into());
        assert!(!m.matches(&other, &SetExpr::All, &no_rel).unwrap());
    }

    #[test]
    fn fires_exactly_once_on_last_blocker_into_the_one_inbox() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(None)).unwrap();

        assert_eq!(
            eng.on_event(&unblock_event("e-partial", 1), &SetExpr::All, &no_rel),
            0,
            "a partial resolution does not fire"
        );
        assert!(eng.delivered().is_empty());

        assert_eq!(
            eng.on_event(&unblock_event("e-done", 0), &SetExpr::All, &no_rel),
            1,
            "the last blocker clearing fires exactly once"
        );
        assert_eq!(eng.delivered().len(), 1);
        assert_eq!(eng.delivered()[0].kind, TriggerInboxKind::Resolved);
        assert_eq!(eng.delivered()[0].reason, Reason::Unblocked);
        assert_eq!(eng.inbox().snapshot_for_tenant(&tenant()).len(), 1);

        assert_eq!(
            eng.on_event(&unblock_event("e-dup", 0), &SetExpr::All, &no_rel),
            0,
            "a re-delivery does not fire a second time"
        );
        assert_eq!(eng.delivered().len(), 1, "still exactly one fire");
    }

    #[test]
    fn stale_nudge_fires_exactly_once_then_stale() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        let deadline = default_stale_after(0);
        eng.arm(arm_unblock(Some(deadline))).unwrap();
        let id = TriggerId("t-unblock".into());

        assert!(eng.on_stale_timer(&id), "the stale nudge fires once");
        assert_eq!(eng.delivered().len(), 1);
        assert_eq!(eng.delivered()[0].kind, TriggerInboxKind::StaleNudge);
        assert_eq!(eng.arming(&id).unwrap().state, TriggerState::Stale);

        assert!(
            !eng.on_stale_timer(&id),
            "a re-fire after stale is a no-op (stale-once)"
        );
        assert_eq!(eng.delivered().len(), 1, "still exactly one stale nudge");
    }

    #[test]
    fn resolve_wins_over_a_late_stale_timer() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(Some(default_stale_after(0)))).unwrap();
        let id = TriggerId("t-unblock".into());

        assert_eq!(
            eng.on_event(&unblock_event("e", 0), &SetExpr::All, &no_rel),
            1
        );
        assert_eq!(eng.arming(&id).unwrap().state, TriggerState::Resolved);

        assert!(
            !eng.on_stale_timer(&id),
            "the late stale timer loses to the resolve"
        );
        assert_eq!(eng.delivered().len(), 1, "only the resolve fire, no nudge");
        assert_eq!(eng.delivered()[0].kind, TriggerInboxKind::Resolved);
    }

    #[test]
    fn fires_exactly_once_across_a_restart() {
        let inbox = InboxProjection::new();
        let id = TriggerId("t-unblock".into());

        let mut eng = IssueTriggerEngine::with_inbox(tenant(), region(), inbox.clone());
        eng.arm(arm_unblock(Some(default_stale_after(0)))).unwrap();
        let snapshot = eng.snapshot();
        let meta = eng.meta_for_snapshot();
        drop(eng);

        let mut eng2 =
            IssueTriggerEngine::restore(tenant(), region(), inbox.clone(), snapshot.clone());
        for (tid, cond, ow, subj) in meta {
            eng2.restore_meta(tid, cond, ow, subj);
        }
        assert_eq!(eng2.arming(&id).unwrap().state, TriggerState::Armed);

        assert_eq!(
            eng2.on_event(&unblock_event("e-after-restart", 0), &SetExpr::All, &no_rel),
            1,
            "the resolve fires exactly once after the restart"
        );
        assert_eq!(eng2.delivered().len(), 1);
        assert_eq!(eng2.delivered()[0].kind, TriggerInboxKind::Resolved);

        let snap2 = eng2.snapshot();
        let meta2 = eng2.meta_for_snapshot();
        drop(eng2);
        let mut eng3 = IssueTriggerEngine::restore(tenant(), region(), inbox.clone(), snap2);
        for (tid, cond, ow, subj) in meta2 {
            eng3.restore_meta(tid, cond, ow, subj);
        }
        assert_eq!(eng3.arming(&id).unwrap().state, TriggerState::Resolved);
        assert_eq!(
            eng3.on_event(&unblock_event("e-replay", 0), &SetExpr::All, &no_rel),
            0,
            "a re-delivery after a second restart does not re-fire"
        );
        assert!(
            eng3.delivered().is_empty(),
            "the restored engine fires nothing for an already-resolved arming"
        );
    }

    #[test]
    fn owner_cancel_disarms_the_promise() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(Some(default_stale_after(0)))).unwrap();
        let id = TriggerId("t-unblock".into());

        assert!(eng.disarm(&id).unwrap(), "the owner cancel disarms");
        assert_eq!(eng.arming(&id).unwrap().state, TriggerState::Disarmed);
        assert_eq!(
            eng.on_event(&unblock_event("e", 0), &SetExpr::All, &no_rel),
            0,
            "a disarmed promise does not fire"
        );
        assert!(!eng.on_stale_timer(&id));
        assert!(eng.delivered().is_empty());
    }

    #[test]
    fn unviewable_subject_never_resolves() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(None)).unwrap();
        assert_eq!(
            eng.on_event(&unblock_event("e", 0), &SetExpr::None, &no_rel),
            0,
            "an unviewable subject never resolves (0-leak)"
        );
        assert!(eng.delivered().is_empty());
        assert_eq!(
            eng.arming(&TriggerId("t-unblock".into())).unwrap().state,
            TriggerState::Armed
        );
    }

    #[test]
    fn default_stale_after_is_thirty_days() {
        assert_eq!(default_stale_after(0).0, "1970-01-31T00:00:00Z");
        let known = 1_782_000_000;
        assert_eq!(default_stale_after(known).0, "2026-07-21T00:00:00Z");
    }

    #[test]
    fn snapshot_round_trips_stably() {
        let mut eng = IssueTriggerEngine::new(tenant(), region());
        eng.arm(arm_unblock(Some(default_stale_after(0)))).unwrap();
        let snap = eng.snapshot();
        let json = serde_json::to_string(&snap).unwrap();
        let back: TriggerSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, back);
        assert_eq!(back.armings.len(), 1);
    }
}
