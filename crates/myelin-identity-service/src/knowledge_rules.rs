use myelin_notif::{define_notif_rule, Class, DedupTpl, NotifRule, NotifRuleRegistry, Reason};

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

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_notif::reason_base_class;

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
}
