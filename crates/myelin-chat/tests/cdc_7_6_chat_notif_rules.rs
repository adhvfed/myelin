use myelin_chat::events::{CHAT_DURABLE_TOKENS, CHAT_MESSAGE_MENTIONED};
use myelin_chat::glue::{
    chat_notif_rules, fanout_class, fanout_class_is_total_over_durable_tokens,
    register_chat_notif_rules, FanoutClass, RULE_KEY_APPROVAL_REQUESTED, RULE_KEY_MENTIONED,
    RULE_KEY_REPLIED, RULE_KEY_THREAD_WATCHED,
};
use myelin_notif::{reason_base_class, Class, NotifRule, NotifRuleRegistry, Reason};
use myelin_refs::ArtifactRef;
use std::collections::BTreeMap;

fn provider_chat_rules() -> Vec<(&'static str, NotifRule)> {
    chat_notif_rules()
}

fn consumer_admits_and_classifies(
    rules: Vec<(&'static str, NotifRule)>,
    rule_key: &str,
    recipient: &str,
    subject: &ArtifactRef,
) -> myelin_notif::Classification {
    let mut reg = NotifRuleRegistry::platform_default();
    for (key, rule) in rules {
        reg.register(key, rule);
    }
    reg.classify(rule_key, recipient, subject)
}

#[test]
fn cdc_7_6_chat_provider_declares_reasons_consumer_admits_and_classifies() {
    let subject = ArtifactRef("myelin://acme/chat/channel/eng".into());
    let rules = provider_chat_rules();
    assert_eq!(rules.len(), 4, "chat declares the four reasons");

    let by_key: BTreeMap<&str, &NotifRule> = rules.iter().map(|(k, r)| (*k, r)).collect();
    assert_eq!(by_key[RULE_KEY_MENTIONED].default_class, Class::Direct);
    assert_eq!(by_key[RULE_KEY_REPLIED].default_class, Class::Participating);
    assert_eq!(
        by_key[RULE_KEY_THREAD_WATCHED].default_class,
        Class::Watching
    );
    assert_eq!(
        by_key[RULE_KEY_APPROVAL_REQUESTED].default_class,
        Class::Critical
    );

    for (key, reason, class) in [
        (RULE_KEY_MENTIONED, Reason::Mentioned, Class::Direct),
        (RULE_KEY_REPLIED, Reason::Replied, Class::Participating),
        (
            RULE_KEY_THREAD_WATCHED,
            Reason::ThreadWatched,
            Class::Watching,
        ),
        (
            RULE_KEY_APPROVAL_REQUESTED,
            Reason::ApprovalRequested,
            Class::Critical,
        ),
    ] {
        let c = consumer_admits_and_classifies(provider_chat_rules(), key, "psn:alice", &subject);
        assert_eq!(c.reason, reason, "rule `{key}` classifies to its reason");
        assert_eq!(
            c.default_class, class,
            "rule `{key}` lands in its §3.1 band"
        );
        assert_eq!(
            c.default_class,
            reason_base_class(reason).1,
            "the table owns the band"
        );
        assert!(
            c.from_registered_rule,
            "the registered chat rule took effect (0 Notif change)"
        );
    }
}

#[test]
fn cdc_7_6_chat_reason_set_accretes_with_zero_notif_change() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    register_chat_notif_rules(&mut reg);
    assert_eq!(
        reg.len(),
        before + 4,
        "the four chat rules accreted (no Notif enum/match edit)"
    );
}

#[test]
fn cdc_7_6_chat_fanout_class_is_total_and_write_fanout_is_bounded() {
    assert!(
        fanout_class_is_total_over_durable_tokens(),
        "the fanout-class must be total over chat's durable tokens"
    );
    assert_eq!(
        fanout_class(CHAT_MESSAGE_MENTIONED),
        Some(FanoutClass::WriteFanout)
    );
    let write_fanout = CHAT_DURABLE_TOKENS
        .iter()
        .filter(|t| fanout_class(t) == Some(FanoutClass::WriteFanout))
        .count();
    assert!(write_fanout >= 1 && write_fanout < CHAT_DURABLE_TOKENS.len());
}
