//! # The CDC pair for contract 7.6 — chat's `define_notif_rule` set + the fanout-class (CHAT-P3 / P-245)
//!
//! **Contract:** `contract-index.md` row 7.6 (`define_notif_rule(reason, dedup_tpl, default_class)`
//! — Signal class → inbox reason/priority; each subsystem registers its set: "Chat
//! mentioned/replied/thread_watched/approval"). Owning architecture: chat
//! `03-events-contracts-and-glue.md` §4 (the fanout boundary chat owns — write-fanout the bounded
//! high-signal set, read-fanout the unbounded ambient set; "Chat registers the default
//! Signal/notify-reason rules via `define_notif_rule`"). **Reconciliation:** OQ1 (the
//! `define_notif_rule` set is the per-subsystem enumeration — a CONFIRM, no Notif change).
//!
//! ## The seam this pair pins (chat REGISTERS its reason set; Notif owns the engine + the table)
//! - **PROVIDER (chat — [`myelin_chat::glue`])** declares its four reasons (mentioned / replied /
//!   thread_watched / approval_requested), each at its table-correct band via the frozen
//!   `define_notif_rule` verb, and its fanout-class decision (write-fanout vs read-fanout, arch §4).
//! - **CONSUMER (Notif — [`myelin_notif::NotifRuleRegistry`])** ADMITS chat's rules via the
//!   inverse-signal seam (zero Notif change) and CLASSIFIES a Signal carrying each `rule_key` into
//!   the registered reason + §3.1 band + rendered dedup key; the §3.1 ranking table owns the band.

use myelin_chat::events::{CHAT_DURABLE_TOKENS, CHAT_MESSAGE_MENTIONED};
use myelin_chat::glue::{
    chat_notif_rules, fanout_class, fanout_class_is_total_over_durable_tokens,
    register_chat_notif_rules, FanoutClass, RULE_KEY_APPROVAL_REQUESTED, RULE_KEY_MENTIONED,
    RULE_KEY_REPLIED, RULE_KEY_THREAD_WATCHED,
};
use myelin_notif::{reason_base_class, Class, NotifRule, NotifRuleRegistry, Reason};
use myelin_refs::ArtifactRef;
use std::collections::BTreeMap;

/// **PROVIDER side of 7.6** — chat declares its four `define_notif_rule` reasons. The provider's
/// promise: each is built by the frozen verb at the table-correct band (chat registers WHICH reason;
/// Notif's table owns the band — a wrong band would have panicked at construction).
fn provider_chat_rules() -> Vec<(&'static str, NotifRule)> {
    chat_notif_rules()
}

/// **CONSUMER side of 7.6** — Notif's registry ADMITS chat's rules and CLASSIFIES a Signal carrying a
/// `rule_key`. The consumer's promise: it admits the registration with ZERO Notif change (the
/// inverse-signal seam) and classifies through the registered rule.
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

/// The 7.6 pair, end-to-end: the PROVIDER (chat) declares its four reasons at their table-correct
/// bands, and the CONSUMER (Notif) ADMITS + CLASSIFIES each — the dated green artifact (the
/// contract-coverage scanner's 7.6 chat row).
#[test]
fn cdc_7_6_chat_provider_declares_reasons_consumer_admits_and_classifies() {
    let subject = ArtifactRef("myelin://acme/chat/channel/eng".into());
    let rules = provider_chat_rules();
    assert_eq!(rules.len(), 4, "chat declares the four reasons");

    // every rule registers at its §3.1 band (provider's table-correct construction).
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

    // the CONSUMER admits + classifies each rule_key through the registered chat rule.
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

/// The reason set registers WITH Notif and grows the registry by four with ZERO Notif change (the
/// inverse-signal seam — the per-subsystem accretion). The fluent register helper is the production
/// registration call.
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

/// **The fanout-class is chat's OWNED per-event decision (arch §4) and is TOTAL over the durable
/// tokens.** The PROVIDER decides write-fanout (the bounded high-signal `mention` producer) vs
/// read-fanout (the unbounded ambient set that never write-amplifies — the celebrity-fanout
/// mitigation). Every durable token classifies; the write-fanout set is a strict bounded subset.
#[test]
fn cdc_7_6_chat_fanout_class_is_total_and_write_fanout_is_bounded() {
    assert!(
        fanout_class_is_total_over_durable_tokens(),
        "the fanout-class must be total over chat's durable tokens"
    );
    // the canonical write-fanout producer is the mention (contract 13.1).
    assert_eq!(
        fanout_class(CHAT_MESSAGE_MENTIONED),
        Some(FanoutClass::WriteFanout)
    );
    // write-fanout is a STRICT bounded subset (the unbounded ambient set never write-amplifies).
    let write_fanout = CHAT_DURABLE_TOKENS
        .iter()
        .filter(|t| fanout_class(t) == Some(FanoutClass::WriteFanout))
        .count();
    assert!(write_fanout >= 1 && write_fanout < CHAT_DURABLE_TOKENS.len());
}
