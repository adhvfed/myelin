use super::*;
use myelin_notif::{Class, NotifRuleRegistry, Reason};
use myelin_refs::ArtifactRef;
use std::collections::BTreeMap;

fn subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/chat/channel/eng".into())
}

#[test]
fn chat_humanise_keys_are_registered_into_the_one_templating_surface() {
    let rows = chat_humanise_templates();
    assert_eq!(rows.len(), 7, "exactly the seven chat humanise surfaces");

    let by_key: BTreeMap<&str, &HumaniseTemplate> =
        rows.iter().map(|r| (r.template_key.as_str(), r)).collect();
    for key in [
        TPL_CHAT_CARD,
        TPL_CHAT_CARD_FACETS,
        TPL_CHAT_AGENT_MESSAGE,
        TPL_CHAT_MENTIONED,
        TPL_CHAT_PROJECT_CHANNEL,
        TPL_CHAT_PROJECT_MESSAGE,
        TPL_CHAT_PROJECT_THREAD,
    ] {
        let row = by_key
            .get(key)
            .unwrap_or_else(|| panic!("chat must register `{key}`"));
        assert_eq!(
            row.tenant, PLATFORM_DEFAULT_TENANT,
            "`{key}` is a platform-default row"
        );
        assert_eq!(row.locale, myelin_notif::DEFAULT_LOCALE);
        assert!(
            row.body.contains("{0}"),
            "`{key}` body must bind the {{0}} slot"
        );
        assert!(!row.body.is_empty());
    }
}

#[test]
fn chat_humanise_keys_register_and_look_up_through_notif() {
    let mut store = TemplateStore::with_platform_defaults();
    register_chat_humanise_templates(&mut store);

    for key in [
        TPL_CHAT_CARD,
        TPL_CHAT_CARD_FACETS,
        TPL_CHAT_AGENT_MESSAGE,
        TPL_CHAT_MENTIONED,
    ] {
        let got = store
            .lookup(PLATFORM_DEFAULT_TENANT, key, myelin_notif::DEFAULT_LOCALE)
            .unwrap_or_else(|| panic!("Notif's ONE templating surface serves chat's `{key}`"));
        assert_eq!(got.template_key, key);
        assert!(got.body.contains("{0}"));
    }
}

#[test]
fn chat_mentioned_renders_through_the_one_notif_formatter() {
    let rows = chat_humanise_templates();
    let mentioned = rows
        .iter()
        .find(|r| r.template_key == TPL_CHAT_MENTIONED)
        .expect("mentioned");
    let bound = myelin_notif::render_message(&mentioned.body, &["#eng".to_string()]);
    assert_eq!(bound, "You were mentioned in #eng");
}

#[test]
fn chat_notif_rules_are_the_four_chat_reasons_at_their_bands() {
    let rules = chat_notif_rules();
    assert_eq!(rules.len(), 4, "exactly the four chat reasons");

    let by_key: BTreeMap<&str, &NotifRule> = rules.iter().map(|(k, r)| (*k, r)).collect();

    let m = by_key.get(RULE_KEY_MENTIONED).expect("mentioned");
    assert_eq!(m.reason, Reason::Mentioned);
    assert_eq!(
        m.default_class,
        Class::Direct,
        "@mention is a direct address"
    );

    let r = by_key.get(RULE_KEY_REPLIED).expect("replied");
    assert_eq!(r.reason, Reason::Replied);
    assert_eq!(
        r.default_class,
        Class::Participating,
        "a reply is participation"
    );

    let tw = by_key.get(RULE_KEY_THREAD_WATCHED).expect("thread_watched");
    assert_eq!(tw.reason, Reason::ThreadWatched);
    assert_eq!(
        tw.default_class,
        Class::Watching,
        "a watched thread is ambient"
    );

    let a = by_key
        .get(RULE_KEY_APPROVAL_REQUESTED)
        .expect("approval_requested");
    assert_eq!(a.reason, Reason::ApprovalRequested);
    assert_eq!(
        a.default_class,
        Class::Critical,
        "an HITL approval pierces (critical)"
    );
}

#[test]
fn each_chat_rule_round_trips_its_dedup_template_and_default_class() {
    for (key, rule) in chat_notif_rules() {
        let dk = rule.dedup_key("psn:alice", &subject());
        assert!(
            dk.contains("psn:alice") && dk.contains("myelin://acme/chat/channel/eng"),
            "rule `{key}` dedup key must bind (recipient, subject): got `{dk}`"
        );
        assert!(
            dk.starts_with("chat."),
            "rule `{key}` dedup key is chat-namespaced: `{dk}`"
        );
        assert_eq!(
            rule.default_class,
            myelin_notif::reason_base_class(rule.reason).1,
            "rule `{key}` default_class must be the §3.1 band for its reason"
        );
    }
}

#[test]
fn chat_notif_rules_register_and_classify_through_notif() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    register_chat_notif_rules(&mut reg);
    assert_eq!(
        reg.len(),
        before + 4,
        "the four chat rules accreted (no Notif change)"
    );

    let c = reg.classify(RULE_KEY_MENTIONED, "psn:alice", &subject());
    assert_eq!(c.reason, Reason::Mentioned);
    assert_eq!(c.default_class, Class::Direct);
    assert!(
        c.from_registered_rule,
        "the registered chat rule took effect"
    );
    assert_eq!(
        c.dedup_key,
        "chat.mentioned:psn:alice:myelin://acme/chat/channel/eng"
    );

    let c = reg.classify(RULE_KEY_APPROVAL_REQUESTED, "psn:bob", &subject());
    assert_eq!(c.reason, Reason::ApprovalRequested);
    assert_eq!(c.default_class, Class::Critical);
    assert!(c.from_registered_rule);
}

#[test]
fn chat_cannot_smuggle_a_reason_into_the_wrong_band() {
    let err = define_notif_rule(
        Reason::ApprovalRequested,
        DedupTpl("{subject}".into()),
        Class::Watching,
    )
    .expect_err("approval_requested must register at the critical band the §3.1 table owns");
    assert!(matches!(
        err,
        myelin_notif::DefineRuleError::ClassMismatch { .. }
    ));
}

#[test]
fn fanout_class_is_total_over_the_chat_durable_tokens() {
    assert!(
        fanout_class_is_total_over_durable_tokens(),
        "the fanout-class must be total over chat's durable tokens"
    );
    for t in CHAT_DURABLE_TOKENS {
        assert!(
            fanout_class(t).is_some(),
            "durable token `{t}` must classify into a fanout class"
        );
    }
    assert_eq!(fanout_class("git.pr.opened"), None);
    assert_eq!(fanout_class("chat.message.nonexistent"), None);
}

#[test]
fn write_fanout_is_the_bounded_high_signal_set_rest_is_read_fanout() {
    assert_eq!(
        fanout_class(CHAT_MESSAGE_MENTIONED),
        Some(FanoutClass::WriteFanout)
    );
    assert_eq!(
        fanout_class(crate::events::CHAT_THREAD_REPLIED),
        Some(FanoutClass::WriteFanout)
    );

    for ambient in [
        crate::events::CHAT_MESSAGE_CREATED,
        crate::events::CHAT_CHANNEL_MEMBER_ADDED,
        crate::events::CHAT_CHANNEL_SNAPSHOT,
        CHAT_CHANNEL_SNAPSHOT,
    ] {
        assert_eq!(
            fanout_class(ambient),
            Some(FanoutClass::ReadFanout),
            "`{ambient}` is the ambient read-fanout set (never write-amplifies)"
        );
    }
    for fh in CHAT_FIREHOSE_TOKENS {
        assert_eq!(
            fanout_class(fh),
            Some(FanoutClass::ReadFanout),
            "firehose `{fh}` is ambient"
        );
    }
}

#[test]
fn the_write_fanout_set_is_a_bounded_subset_of_the_durable_tokens() {
    let write_fanout_count = CHAT_DURABLE_TOKENS
        .iter()
        .filter(|t| fanout_class(t) == Some(FanoutClass::WriteFanout))
        .count();
    assert!(
        write_fanout_count >= 1,
        "at least the mention producer is write-fanout"
    );
    assert!(
        write_fanout_count < CHAT_DURABLE_TOKENS.len(),
        "write-fanout must be a STRICT subset (the unbounded ambient set never write-amplifies)"
    );
    assert!(CHAT_WRITE_FANOUT_TOKENS.contains(&CHAT_MESSAGE_MENTIONED));
}

#[test]
fn hitl_card_facets_render_action_risk_and_cost_through_the_one_surface() {
    let mut store = TemplateStore::with_platform_defaults();
    register_chat_humanise_templates(&mut store);

    let facets_tpl = store
        .lookup(
            PLATFORM_DEFAULT_TENANT,
            TPL_CHAT_CARD_FACETS,
            myelin_notif::DEFAULT_LOCALE,
        )
        .expect("the facets key is registered");
    for slot in [CARD_FACET_ACTION, CARD_FACET_RISK, CARD_FACET_COST] {
        assert!(
            facets_tpl.body.contains(&format!("{{{slot}}}")),
            "the facets body must bind slot {{{slot}}}: `{}`",
            facets_tpl.body
        );
    }

    let facets = chat_hitl_card_facets(&store, "merge", "irreversible", "0.40 USD");
    assert!(facets.contains("merge"), "action: `{facets}`");
    assert!(facets.contains("irreversible"), "risk: `{facets}`");
    assert!(facets.contains("0.40 USD"), "cost: `{facets}`");
}

#[test]
fn chat_firehose_scope_is_bounded_channel_never_star() {
    let scope = chat_channel_scope("eng").expect("a bounded channel scope parses");
    assert_eq!(
        scope.kind(),
        ScopeKind::Channel,
        "chat's per-view scope is channel:<id>"
    );
    assert_eq!(scope.id(), "eng");
    assert_eq!(
        scope.selector(),
        "channel:eng",
        "the channel scope round-trips its selector"
    );
}

#[test]
fn an_unbounded_chat_scope_is_rejected() {
    for bad in ["*", "", "*all"] {
        let r = chat_channel_scope(bad);
        assert!(
            r.is_err(),
            "an unbounded channel scope `channel:{bad}` must be rejected, got {r:?}"
        );
        assert!(
            r.unwrap_err().is_over_broad_scope(),
            "`channel:{bad}` is an over-broad-scope rejection (scope is never *)"
        );
    }
    assert!(FirehoseScope::parse("channel:")
        .unwrap_err()
        .is_over_broad_scope());
    assert!(FirehoseScope::parse("*").unwrap_err().is_over_broad_scope());
}

#[test]
fn the_resync_snapshot_fallback_names_chat_durable_snapshots() {
    assert_eq!(
        CHAT_RESYNC_SNAPSHOT_TOKENS.len(),
        3,
        "channel/message/thread snapshots"
    );
    for snap in CHAT_RESYNC_SNAPSHOT_TOKENS {
        assert!(
            CHAT_DURABLE_TOKENS.contains(snap),
            "the resync fallback snapshot `{snap}` must be a DURABLE token (rides the outbox)"
        );
        assert!(
            snap.ends_with(".snapshot"),
            "`{snap}` is a *.snapshot reindex-from-source token"
        );
    }
}

#[test]
fn te21_pin_is_rust_and_the_harness_shim_is_a_no_op() {
    assert_eq!(
        Te21LanguagePin::PINNED,
        Te21LanguagePin::Rust,
        "the M2-C0 pin is Rust"
    );
    assert!(
        Te21LanguagePin::Rust.is_no_op(),
        "the all-Rust default makes the shim a no-op"
    );
    let recorded = te21_harness_shim_obligation();
    assert_eq!(recorded, Te21LanguagePin::Rust);
    assert!(
        recorded.is_no_op(),
        "the recorded TE-21 obligation is a no-op against the 1.7 shim"
    );

    assert!(
        !Te21LanguagePin::Beam.is_no_op(),
        "the BEAM hatch carries the shim obligation (CHAT-P26)"
    );
}
