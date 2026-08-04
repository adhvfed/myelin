use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use myelin_identity::{
    AuthzError, Consistency, ListObjectsResult, ObjectType, Permission, Principal, RelName,
    Result as AuthzResult, SetExpr, Zookie,
};
use myelin_notif::{
    define_notif_rule, Class, DedupTpl, NotifRule, NotifRuleRegistry, Reason, RelationalLeaf,
    ReverseIndexAnswer, RevisionWatermark, WatcherResolvePort,
};
use myelin_tenancy::TenantId;

use crate::knowledge_fragment::object_types;

pub const KN_MENTIONED_RULE: &str = "knowledge.mentioned";

pub const KN_COMMENTS_RULE: &str = "knowledge.comments";

pub const KN_SHARED_RULE: &str = "knowledge.shared";

pub const KN_WATCHED_RULE: &str = "knowledge.watched";

pub fn knowledge_notif_rules(
) -> Result<Vec<(&'static str, NotifRule)>, myelin_notif::DefineRuleError> {
    Ok(vec![
        (
            KN_MENTIONED_RULE,
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("kn-mention:{recipient}:{subject}".into()),
                Class::Direct,
            )?,
        ),
        (
            KN_COMMENTS_RULE,
            define_notif_rule(
                Reason::Comments,
                DedupTpl("kn-comments:{subject}".into()),
                Class::Participating,
            )?,
        ),
        (
            KN_SHARED_RULE,
            define_notif_rule(
                Reason::Shared,
                DedupTpl("kn-shared:{recipient}:{subject}".into()),
                Class::Direct,
            )?,
        ),
        (
            KN_WATCHED_RULE,
            define_notif_rule(
                Reason::Watched,
                DedupTpl("kn-watched:{subject}".into()),
                Class::Watching,
            )?,
        ),
    ])
}

pub fn register_knowledge_notif_rules(
    registry: &mut NotifRuleRegistry,
) -> Result<&mut NotifRuleRegistry, myelin_notif::DefineRuleError> {
    for (key, rule) in knowledge_notif_rules()? {
        registry.register(key, rule);
    }
    Ok(registry)
}

pub const KN_WATCHER_RELATION: &str = myelin_notif::WATCHER_RELATION;

pub fn knowledge_watchable_object_types() -> [&'static str; 3] {
    [
        object_types::SPACE,
        object_types::PAGE,
        object_types::DATABASE_ROW,
    ]
}

#[derive(Clone, Default)]
pub struct KnowledgeWatcherIndex {
    inner: Arc<Mutex<KnowledgeWatcherState>>,
}

#[derive(Default)]
struct KnowledgeWatcherState {
    watches: BTreeMap<(String, String), BTreeSet<String>>,
    revision: u64,
    unavailable: bool,
}

impl KnowledgeWatcherIndex {
    pub fn new() -> KnowledgeWatcherIndex {
        KnowledgeWatcherIndex::default()
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

impl WatcherResolvePort for KnowledgeWatcherIndex {
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
                "knowledge watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName(KN_WATCHER_RELATION.into()),
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
                "knowledge watcher reverse index unavailable (held, not leaked)".into(),
            ));
        }
        let watched = match leaf {
            RelationalLeaf::InRelation { relation, .. } if relation.0 == KN_WATCHER_RELATION => g
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

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    #[test]
    fn knowledge_rules_are_table_correct_mention_comments_shared_watched() {
        let rules = knowledge_notif_rules().expect("kn's set is table-correct by construction");
        let keys: Vec<&str> = rules.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![
                KN_MENTIONED_RULE,
                KN_COMMENTS_RULE,
                KN_SHARED_RULE,
                KN_WATCHED_RULE
            ]
        );
        for (key, rule) in &rules {
            assert_eq!(
                rule.default_class,
                reason_base_class(rule.reason).1,
                "rule `{key}` must register the §3.1 band for its reason"
            );
        }
        assert_eq!(rules[0].1.reason, Reason::Mentioned);
        assert_eq!(rules[0].1.default_class, Class::Direct);
        assert_eq!(rules[1].1.reason, Reason::Comments);
        assert_eq!(rules[1].1.default_class, Class::Participating);
        assert_eq!(rules[2].1.reason, Reason::Shared);
        assert_eq!(rules[2].1.default_class, Class::Direct);
        assert_eq!(rules[3].1.reason, Reason::Watched);
        assert_eq!(rules[3].1.default_class, Class::Watching);
    }

    #[test]
    fn knowledge_registers_with_zero_notif_change() {
        let mut reg = NotifRuleRegistry::platform_default();
        let before = reg.len();
        register_knowledge_notif_rules(&mut reg).expect("kn's set registers");
        assert_eq!(
            reg.len(),
            before + 4,
            "kn's four rules accreted (no Notif enum/match edit)"
        );

        let subject = myelin_refs::ArtifactRef("myelin://acme/knowledge/page/9".into());
        let m = reg.classify(KN_MENTIONED_RULE, "psn:bob", &subject);
        assert_eq!(m.reason, Reason::Mentioned);
        assert_eq!(m.default_class, Class::Direct);
        assert!(
            m.from_registered_rule,
            "the KN registration took effect (0 Notif change)"
        );
        assert_eq!(
            m.dedup_key,
            "kn-mention:psn:bob:myelin://acme/knowledge/page/9"
        );

        let c = reg.classify(KN_COMMENTS_RULE, "psn:alice", &subject);
        assert_eq!(c.reason, Reason::Comments);
        assert_eq!(c.default_class, Class::Participating);
        assert_eq!(c.dedup_key, "kn-comments:myelin://acme/knowledge/page/9");

        let w = reg.classify(KN_WATCHED_RULE, "psn:carol", &subject);
        assert_eq!(w.reason, Reason::Watched);
        assert_eq!(w.default_class, Class::Watching);
    }

    #[test]
    fn knowledge_re_registration_is_idempotent() {
        let mut reg = NotifRuleRegistry::new();
        register_knowledge_notif_rules(&mut reg).unwrap();
        register_knowledge_notif_rules(&mut reg).unwrap();
        assert_eq!(
            reg.len(),
            4,
            "re-registering KN's set keeps four rules (idempotent)"
        );
    }

    #[test]
    fn knowledge_watcher_relation_matches_the_frozen_fragment() {
        assert_eq!(KN_WATCHER_RELATION, "watcher");
        assert_eq!(KN_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
        assert_eq!(
            knowledge_watchable_object_types(),
            ["space", "page", "database_row"]
        );
        assert!(
            crate::knowledge_fragment::space_fragment().is_watchable(),
            "space is watchable"
        );
        assert!(
            crate::knowledge_fragment::page_fragment().is_watchable(),
            "page is watchable"
        );
        assert!(
            crate::knowledge_fragment::database_row_fragment().is_watchable(),
            "database_row is watchable"
        );
        assert!(
            !crate::knowledge_fragment::block_fragment().is_watchable(),
            "block is NOT independently watchable (it inherits its page's ACL)"
        );
    }

    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }

    #[test]
    fn knowledge_watcher_index_resolves_real_watched_pages() {
        let idx = KnowledgeWatcherIndex::new();
        let page = "myelin://acme/knowledge/page/9";
        idx.watch(&tenant(), "psn:alice", page);

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
            answer.subject_roots.contains(page),
            "alice watches the page"
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
    fn knowledge_watcher_index_only_serves_the_watcher_relation() {
        let idx = KnowledgeWatcherIndex::new();
        idx.watch(&tenant(), "psn:alice", "myelin://acme/knowledge/page/9");
        let other = RelationalLeaf::InRelation {
            relation: RelName("direct_reader".into()),
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
