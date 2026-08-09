use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_identity::{
    AuthzError, Consistency, DataRole as IdentityDataRole, ListObjectsResult, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, PrincipalStatus, PseudonymHandle, RelName,
    Result as AuthzResult, SetExpr, Zookie,
};
use myelin_notif::{
    define_notif_rule, Class, DedupTpl, NotifRule, NotifRuleRegistry, Reason, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, WatcherResolvePort,
};
use myelin_query::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_tenancy::{Region, TenantId};

use crate::lifecycle::ReviewState;
use crate::pr_store::PrRecord;
use crate::rebac_fragment::object_types;

pub const GIT_REVIEW_REQUESTED_RULE: &str = "git.review_requested";

pub const GIT_MENTIONED_RULE: &str = "git.mentioned";

pub const GIT_WATCHED_RULE: &str = "git.watched";

pub fn git_notif_rules() -> Result<Vec<(&'static str, NotifRule)>, myelin_notif::DefineRuleError> {
    Ok(vec![
        (
            GIT_REVIEW_REQUESTED_RULE,
            define_notif_rule(
                Reason::ReviewRequested,
                DedupTpl("git-review:{subject}".into()),
                Class::Direct,
            )?,
        ),
        (
            GIT_MENTIONED_RULE,
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("git-mention:{recipient}:{subject}".into()),
                Class::Direct,
            )?,
        ),
        (
            GIT_WATCHED_RULE,
            define_notif_rule(
                Reason::Watched,
                DedupTpl("git-watched:{subject}".into()),
                Class::Watching,
            )?,
        ),
    ])
}

pub fn register_git_notif_rules(
    registry: &mut NotifRuleRegistry,
) -> Result<&mut NotifRuleRegistry, myelin_notif::DefineRuleError> {
    for (key, rule) in git_notif_rules()? {
        registry.register(key, rule);
    }
    Ok(registry)
}

pub(crate) fn review_request_signal_drafts(
    tenant: &TenantId,
    region: &Region,
    repo: &str,
    record: &PrRecord,
    recorded_at: &str,
) -> Result<Vec<EventDraft>, myelin_notif::DefineRuleError> {
    let rule = git_notif_rules()?
        .into_iter()
        .find_map(|(key, rule)| (key == GIT_REVIEW_REQUESTED_RULE).then_some(rule))
        .expect("Git's review-request rule is part of its frozen notification set");
    let subject = ArtifactRef(format!(
        "myelin://{}/git/pr/{repo}:{}",
        tenant.0, record.number
    ));
    let mut drafts = Vec::new();
    for review in &record.reviews {
        if !matches!(review.state, ReviewState::Requested) {
            continue;
        }
        let Some(handle) = PseudonymHandle::parse(&review.reviewer_pseudonym) else {
            continue;
        };
        if handle.tenant() != tenant.0 {
            continue;
        }
        let recipient = handle.pseudonym();
        let dedup_key = rule.dedup_key(recipient, &subject);
        let signal = Signal {
            rule_id: RuleId(GIT_REVIEW_REQUESTED_RULE.into()),
            tenant: tenant.clone(),
            severity: Severity::Notice,
            dedup_key: DedupKey(dedup_key.clone()),
            subject: subject.clone(),
            count: 1,
            state: SignalState::Open,
            first_seen: recorded_at.to_string(),
            last_seen: recorded_at.to_string(),
        };
        let recipient = Principal::new(
            tenant.clone(),
            region.clone(),
            PrincipalId(recipient.to_string()),
            PrincipalKind::Human,
            IdentityDataRole::Controller,
            PrincipalStatus::Active,
        );
        let aggregate_id = &blake3::hash(dedup_key.as_bytes()).to_hex()[..32];
        let mut payload =
            serde_json::to_value(signal).expect("the closed Signal wire shape is serializable");
        payload["mentions"] = serde_json::json!([{ "Mention": recipient }]);
        payload["notification_reason"] = serde_json::to_value(rule.reason)
            .expect("the closed notification reason vocabulary is serializable");
        drafts.push(EventDraft {
            type_: EventType("signal.opened".into()),
            subject: ArtifactRef(format!(
                "sig.{}.{}.{}",
                tenant.0,
                Severity::Notice.token(),
                GIT_REVIEW_REQUESTED_RULE
            )),
            aggregate: AggregateKey(format!("signal:{aggregate_id}")),
            payload,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        });
    }
    Ok(drafts)
}

pub const GIT_WATCHER_RELATION: &str = myelin_notif::WATCHER_RELATION;

pub fn git_watchable_object_types() -> [&'static str; 2] {
    [object_types::REPO, object_types::PULL_REQUEST]
}

#[derive(Clone, Default)]
pub struct GitWatcherIndex {
    inner: Arc<Mutex<GitWatcherState>>,
}

#[derive(Default)]
struct GitWatcherState {
    watches: BTreeMap<(String, String), BTreeSet<String>>,
    revision: u64,
    unavailable: bool,
}

impl GitWatcherIndex {
    pub fn new() -> GitWatcherIndex {
        GitWatcherIndex::default()
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

impl WatcherResolvePort for GitWatcherIndex {
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
                "git watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName(GIT_WATCHER_RELATION.into()),
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
                "git watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        let watched = match leaf {
            RelationalLeaf::InRelation { relation, .. } if relation.0 == GIT_WATCHER_RELATION => g
                .watches
                .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                .cloned()
                .unwrap_or_default(),
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
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_notif::reason_base_class;
    use myelin_query::Signal;

    use crate::lifecycle::PullRequest;
    use crate::pr_store::ReviewRecord;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    #[test]
    fn git_rules_are_table_correct_review_mention_watched() {
        let rules = git_notif_rules().expect("git's set is table-correct by construction");
        let keys: Vec<&str> = rules.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![
                GIT_REVIEW_REQUESTED_RULE,
                GIT_MENTIONED_RULE,
                GIT_WATCHED_RULE
            ]
        );
        for (key, rule) in &rules {
            assert_eq!(
                rule.default_class,
                reason_base_class(rule.reason).1,
                "rule `{key}` must register the §3.1 band for its reason"
            );
        }
        assert_eq!(rules[0].1.reason, Reason::ReviewRequested);
        assert_eq!(rules[0].1.default_class, Class::Direct);
        assert_eq!(rules[1].1.reason, Reason::Mentioned);
        assert_eq!(rules[1].1.default_class, Class::Direct);
        assert_eq!(rules[2].1.reason, Reason::Watched);
        assert_eq!(rules[2].1.default_class, Class::Watching);
    }

    #[test]
    fn review_request_signals_are_recipient_scoped_and_rule_classified() {
        let pr = PullRequest::open(
            7,
            "refs/heads/main",
            "refs/heads/feature",
            "author@acme.noreply",
            false,
        );
        let mut record = PrRecord::open(&pr, "1".repeat(40));
        record.reviews = vec![
            ReviewRecord {
                reviewer_pseudonym: "alice@acme.noreply".into(),
                state: ReviewState::Requested,
                is_agent: false,
            },
            ReviewRecord {
                reviewer_pseudonym: "bob@acme.noreply".into(),
                state: ReviewState::Submitted(crate::lifecycle::ReviewVerdict::Approve),
                is_agent: false,
            },
            ReviewRecord {
                reviewer_pseudonym: "mallory@other.noreply".into(),
                state: ReviewState::Requested,
                is_agent: false,
            },
            ReviewRecord {
                reviewer_pseudonym: "not-a-pseudonym".into(),
                state: ReviewState::Requested,
                is_agent: false,
            },
        ];

        let drafts = review_request_signal_drafts(
            &tenant(),
            &Region("fr-par".into()),
            "core",
            &record,
            "2026-08-09T12:00:00Z",
        )
        .unwrap();
        assert_eq!(drafts.len(), 1, "only the outstanding request is signalled");
        let draft = &drafts[0];
        assert_eq!(draft.type_.0, "signal.opened");
        assert_eq!(draft.subject.0, "sig.acme.notice.git.review_requested");
        assert!(draft.aggregate.0.starts_with("signal:"));
        assert!(!draft.aggregate.0.contains('.'));
        assert_eq!(draft.payload["notification_reason"], "review_requested");
        assert_eq!(
            draft.payload["mentions"][0]["Mention"]["principal_id"],
            "alice"
        );
        let signal: Signal = serde_json::from_value(draft.payload.clone()).unwrap();
        assert_eq!(signal.rule_id.0, GIT_REVIEW_REQUESTED_RULE);
        assert_eq!(signal.subject.0, "myelin://acme/git/pr/core:7");
        assert_eq!(signal.first_seen, "2026-08-09T12:00:00Z");
    }

    #[test]
    fn git_registers_with_zero_notif_change() {
        let mut reg = NotifRuleRegistry::platform_default();
        let before = reg.len();
        register_git_notif_rules(&mut reg).expect("git's set registers");
        assert_eq!(
            reg.len(),
            before + 3,
            "git's three rules accreted (no Notif enum/match edit)"
        );

        let subject = myelin_refs::ArtifactRef("myelin://acme/git/pr/9".into());
        let c = reg.classify(GIT_REVIEW_REQUESTED_RULE, "psn:reviewer", &subject);
        assert_eq!(c.reason, Reason::ReviewRequested);
        assert_eq!(c.default_class, Class::Direct);
        assert!(
            c.from_registered_rule,
            "the Git registration took effect (0 Notif change)"
        );
        assert_eq!(c.dedup_key, "git-review:myelin://acme/git/pr/9");

        let m = reg.classify(GIT_MENTIONED_RULE, "psn:bob", &subject);
        assert_eq!(m.reason, Reason::Mentioned);
        assert_eq!(m.dedup_key, "git-mention:psn:bob:myelin://acme/git/pr/9");
    }

    #[test]
    fn git_re_registration_is_idempotent() {
        let mut reg = NotifRuleRegistry::new();
        register_git_notif_rules(&mut reg).unwrap();
        register_git_notif_rules(&mut reg).unwrap();
        assert_eq!(
            reg.len(),
            3,
            "re-registering Git's set keeps three rules (idempotent)"
        );
    }

    #[test]
    fn git_watcher_relation_matches_the_frozen_fragment() {
        assert_eq!(GIT_WATCHER_RELATION, "watcher");
        assert_eq!(GIT_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
        assert_eq!(git_watchable_object_types(), ["repo", "pull_request"]);
        let repo_rels: Vec<String> = crate::rebac_fragment::repo_fragment()
            .relations
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert!(repo_rels.contains(&"watcher".to_string()));
        let pr_rels: Vec<String> = crate::rebac_fragment::pull_request_fragment()
            .relations
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert!(pr_rels.contains(&"watcher".to_string()));
    }

    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }

    #[test]
    fn git_watcher_index_resolves_real_watched_prs() {
        let idx = GitWatcherIndex::new();
        let pr = "myelin://acme/git/pr/9";
        idx.watch(&tenant(), "psn:alice", pr);

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
        assert!(answer.subject_roots.contains(pr), "alice watches the PR");

        let none = idx
            .resolve_relation(&viewer("psn:nobody"), &leaf, RevisionWatermark(0))
            .expect("available");
        assert!(
            none.subject_roots.is_empty(),
            "a non-watcher reaches nothing"
        );
    }

    #[test]
    fn git_watcher_index_only_serves_the_watcher_relation() {
        let idx = GitWatcherIndex::new();
        idx.watch(&tenant(), "psn:alice", "myelin://acme/git/pr/9");
        let other = RelationalLeaf::InRelation {
            relation: RelName("reviewer".into()),
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

    fn strong(zk: &str) -> Consistency {
        Consistency {
            at_least: Zookie(zk.into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        }
    }
}
