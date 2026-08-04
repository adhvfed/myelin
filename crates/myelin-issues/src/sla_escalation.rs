use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use myelin_identity::{
    AuthzError, Consistency, ListObjectsResult, ObjectType, Permission, Principal, RelName,
    Result as AuthzResult, SetExpr, Zookie,
};
use myelin_notif::{
    EscalationPolicy, EscalationStep, EscalationTarget, PrefChannel as EscalationChannel,
    RelationalLeaf, ReverseIndexAnswer, RevisionWatermark, WatcherResolvePort,
};
use myelin_tenancy::TenantId;

use crate::rebac_fragment::object_types;

pub const SLA_TEAM_ONCALL_SCHEDULE: &str = "issue-sla-team-oncall";

pub const SLA_PROJECT_ONCALL_SCHEDULE: &str = "issue-sla-project-oncall";

pub const SLA_ORG_ONCALL_SCHEDULE: &str = "issue-sla-org-oncall";

pub const SLA_ESCALATION_POLICY_ID: &str = "issue-sla-escalation";

pub fn issue_sla_escalation_policy(ack_window_minutes: u32, repeat: u32) -> EscalationPolicy {
    let channels = vec![EscalationChannel::InApp, EscalationChannel::WebPush];
    EscalationPolicy {
        policy_id: SLA_ESCALATION_POLICY_ID.to_string(),
        steps: vec![
            EscalationStep {
                target: EscalationTarget::Schedule(SLA_TEAM_ONCALL_SCHEDULE.to_string()),
                channels: channels.clone(),
                ack_window_minutes,
            },
            EscalationStep {
                target: EscalationTarget::Schedule(SLA_PROJECT_ONCALL_SCHEDULE.to_string()),
                channels: channels.clone(),
                ack_window_minutes,
            },
            EscalationStep {
                target: EscalationTarget::Schedule(SLA_ORG_ONCALL_SCHEDULE.to_string()),
                channels,
                ack_window_minutes,
            },
        ],
        repeat: repeat.max(1),
    }
}

pub const ISSUE_WATCHER_RELATION: &str = myelin_notif::WATCHER_RELATION;

pub fn issue_watchable_object_type() -> &'static str {
    object_types::ISSUE
}

#[derive(Clone, Default)]
pub struct IssueWatcherIndex {
    inner: Arc<Mutex<IssueWatcherState>>,
}

#[derive(Default)]
struct IssueWatcherState {
    watches: BTreeMap<(String, String), BTreeSet<String>>,
    revision: u64,
    unavailable: bool,
}

impl IssueWatcherIndex {
    pub fn new() -> IssueWatcherIndex {
        IssueWatcherIndex::default()
    }

    pub fn watch(&self, tenant: &TenantId, principal: &str, subject_root: &str) -> Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        g.watches
            .entry((tenant.0.clone(), principal.to_string()))
            .or_default()
            .insert(subject_root.to_string());
        Zookie(format!("zk-{}", g.revision))
    }

    pub fn unwatch(&self, tenant: &TenantId, principal: &str, subject_root: &str) -> Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        if let Some(set) = g
            .watches
            .get_mut(&(tenant.0.clone(), principal.to_string()))
        {
            set.remove(subject_root);
        }
        Zookie(format!("zk-{}", g.revision))
    }

    pub fn current_zookie(&self) -> Zookie {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Zookie(format!("zk-{}", g.revision))
    }

    pub fn set_unavailable(&self, on: bool) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unavailable = on;
    }
}

impl WatcherResolvePort for IssueWatcherIndex {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.unavailable {
            return Err(AuthzError::Unavailable(
                "issue watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName(ISSUE_WATCHER_RELATION.into()),
                via_column: myelin_notif::subject_root_col(),
            },
            zookie: Zookie(format!("zk-{}", g.revision)),
        })
    }

    fn resolve_relation(
        &self,
        subject: &Principal,
        leaf: &RelationalLeaf,
        _required: RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.unavailable {
            return Err(AuthzError::Unavailable(
                "issue watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        let watched = match leaf {
            RelationalLeaf::InRelation { relation, .. } if relation.0 == ISSUE_WATCHER_RELATION => {
                g.watches
                    .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                    .cloned()
                    .unwrap_or_default()
            }
            RelationalLeaf::TupleSet { .. } => g
                .watches
                .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                .cloned()
                .unwrap_or_default(),
            _ => BTreeSet::new(),
        };
        Ok(ReverseIndexAnswer {
            subject_roots: watched,
            revision: RevisionWatermark(g.revision),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId as IdPrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn viewer(id: &str) -> Principal {
        Principal::stub(IdPrincipalId(id.into()), PrincipalKind::Human, tenant())
    }

    fn strong(zk: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zk.into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        }
    }

    #[test]
    fn sla_chain_is_the_three_tier_frozen_shape() {
        let policy = issue_sla_escalation_policy(15, 1);
        assert_eq!(policy.policy_id, SLA_ESCALATION_POLICY_ID);
        assert_ne!(
            policy.policy_id, "esc-test-chain",
            "Issues passes its REAL chain, not the Notif test floor"
        );
        assert_eq!(policy.steps.len(), 3, "team → project → org incident lead");
        assert_eq!(policy.repeat, 1);

        let tiers: Vec<&EscalationTarget> = policy.steps.iter().map(|s| &s.target).collect();
        assert_eq!(
            tiers,
            vec![
                &EscalationTarget::Schedule(SLA_TEAM_ONCALL_SCHEDULE.to_string()),
                &EscalationTarget::Schedule(SLA_PROJECT_ONCALL_SCHEDULE.to_string()),
                &EscalationTarget::Schedule(SLA_ORG_ONCALL_SCHEDULE.to_string()),
            ]
        );
        for step in &policy.steps {
            assert_eq!(step.ack_window_minutes, 15);
            assert_eq!(
                step.channels,
                vec![EscalationChannel::InApp, EscalationChannel::WebPush]
            );
        }
    }

    #[test]
    fn sla_chain_repeat_is_clamped_to_at_least_one() {
        assert_eq!(issue_sla_escalation_policy(15, 0).repeat, 1);
        assert_eq!(issue_sla_escalation_policy(15, 3).repeat, 3);
        let policy = issue_sla_escalation_policy(10, 2);
        assert!(policy.step_at(0).is_some());
        assert!(
            policy.step_at(5).is_some(),
            "6 positions over 3 steps × 2 loops"
        );
        assert!(policy.step_at(6).is_none(), "exhausted after 3×2 walks");
    }

    #[test]
    fn issue_watcher_relation_matches_the_frozen_fragment() {
        assert_eq!(ISSUE_WATCHER_RELATION, "watcher");
        assert_eq!(ISSUE_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
        assert_eq!(issue_watchable_object_type(), "issue");
        let issue_rels: Vec<String> = crate::rebac_fragment::issue_fragment()
            .relations
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert!(issue_rels.contains(&"watcher".to_string()));
    }

    #[test]
    fn issue_watcher_index_resolves_real_watched_issues() {
        let idx = IssueWatcherIndex::new();
        let issue = "myelin://acme/issue/issue/ENG-1421";
        idx.watch(&tenant(), "psn:alice", issue);

        let result = idx
            .list_objects(
                &viewer("psn:alice"),
                &Permission(myelin_notif::WATCH_PERMISSION.into()),
                &ObjectType(myelin_notif::SUBJECT_ROOT_TYPE.into()),
                &strong("zk-1"),
            )
            .expect("the index is available");
        match result {
            ListObjectsResult::Filter { set_expr, .. } => assert_eq!(
                set_expr,
                SetExpr::InRelation {
                    relation: RelName("watcher".into()),
                    via_column: myelin_notif::subject_root_col(),
                }
            ),
            other => panic!("expected the pushed-down Filter, got {other:?}"),
        }

        let leaf = RelationalLeaf::InRelation {
            relation: RelName("watcher".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        let answer = idx
            .resolve_relation(&viewer("psn:alice"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(
            answer.subject_roots.contains(issue),
            "alice watches the issue"
        );

        let none = idx
            .resolve_relation(&viewer("psn:nobody"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(
            none.subject_roots.is_empty(),
            "a non-watcher reaches nothing"
        );
    }

    #[test]
    fn issue_watcher_index_only_serves_the_watcher_relation() {
        let idx = IssueWatcherIndex::new();
        idx.watch(&tenant(), "psn:alice", "myelin://acme/issue/issue/ENG-1");
        let other = RelationalLeaf::InRelation {
            relation: RelName("assignee".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        let answer = idx
            .resolve_relation(&viewer("psn:alice"), &other, RevisionWatermark(0))
            .expect("available");
        assert!(
            answer.subject_roots.is_empty(),
            "a non-watcher relation reaches nothing (no widen)"
        );
    }

    #[test]
    fn issue_watcher_unwatch_revokes_and_bumps_revision() {
        let idx = IssueWatcherIndex::new();
        let issue = "myelin://acme/issue/issue/ENG-9";
        let zk1 = idx.watch(&tenant(), "psn:alice", issue);
        let zk2 = idx.unwatch(&tenant(), "psn:alice", issue);
        assert_ne!(zk1, zk2, "the unwatch bumps the revision (a newer zookie)");

        let leaf = RelationalLeaf::InRelation {
            relation: RelName("watcher".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        let answer = idx
            .resolve_relation(&viewer("psn:alice"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(
            !answer.subject_roots.contains(issue),
            "the revoked watch is absent (held, not leaked)"
        );
    }

    #[test]
    fn issue_watcher_unavailable_is_held_not_leaked() {
        let idx = IssueWatcherIndex::new();
        idx.watch(&tenant(), "psn:alice", "myelin://acme/issue/issue/ENG-1");
        idx.set_unavailable(true);

        assert!(matches!(
            idx.list_objects(
                &viewer("psn:alice"),
                &Permission(myelin_notif::WATCH_PERMISSION.into()),
                &ObjectType(myelin_notif::SUBJECT_ROOT_TYPE.into()),
                &strong("zk-1"),
            ),
            Err(AuthzError::Unavailable(_))
        ));
        let leaf = RelationalLeaf::InRelation {
            relation: RelName("watcher".into()),
            via_column: myelin_notif::subject_root_col(),
        };
        assert!(matches!(
            idx.resolve_relation(&viewer("psn:alice"), &leaf, RevisionWatermark(0)),
            Err(AuthzError::Unavailable(_))
        ));
    }
}
