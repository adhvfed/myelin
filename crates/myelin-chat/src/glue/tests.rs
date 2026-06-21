//! Unit tests for the CHAT-P3 glue slice (humanise keys + notif rules + fanout-class +
//! firehose-scope validation + TE-21 no-op). The prompt's TESTS line:
//! - each `define_notif_rule` round-trips its dedup template + default class;
//! - the fanout-class is total over the `chat.*` durable tokens;
//! - the firehose scope shape is bounded (never `*`).

use super::*;
use myelin_notif::{Class, NotifRuleRegistry, Reason};
use myelin_refs::ArtifactRef;
use std::collections::BTreeMap;

fn subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/chat/channel/eng".into())
}

// ---------------------------------------------------------------------------------------------
// §1 — the humanise template keys (contract 7.3) — the ONE templating surface (OQ-L)
// ---------------------------------------------------------------------------------------------

/// **Chat registers exactly its three humanise keys into the ONE templating surface (contract
/// 7.3).** The card / agent-message / `chat.message.mentioned` strings — each a NULL-tenant `en`
/// platform-default row, NO chat-private string map (OQ-L). A rename/drop here is a contract change.
#[test]
fn chat_humanise_keys_are_registered_into_the_one_templating_surface() {
    let rows = chat_humanise_templates();
    assert_eq!(rows.len(), 3, "exactly the three chat humanise surfaces");

    let by_key: BTreeMap<&str, &HumaniseTemplate> =
        rows.iter().map(|r| (r.template_key.as_str(), r)).collect();
    for key in [TPL_CHAT_CARD, TPL_CHAT_AGENT_MESSAGE, TPL_CHAT_MENTIONED] {
        let row = by_key.get(key).unwrap_or_else(|| panic!("chat must register `{key}`"));
        // each is a platform-default NULL-tenant `en` row (a tenant overrides; chat never renders).
        assert_eq!(row.tenant, PLATFORM_DEFAULT_TENANT, "`{key}` is a platform-default row");
        assert_eq!(row.locale, myelin_notif::DEFAULT_LOCALE);
        // the body carries the `{0}` SUBJECT slot (resolved per-viewer → title|tombstone).
        assert!(row.body.contains("{0}"), "`{key}` body must bind the {{0}} subject slot");
        assert!(!row.body.is_empty());
    }
}

/// **The keys register into Notif's ONE `TemplateStore` and look up (the GATE — there is no
/// chat-private string map, OQ-L).** Registering chat's rows into a platform-default store and
/// looking each up returns chat's body — the honest "accepted": Notif's ONE templating surface admits
/// + serves chat's keys. (The live per-viewer humanise RENDER route is CHAT-P16/P18.)
#[test]
fn chat_humanise_keys_register_and_look_up_through_notif() {
    let mut store = TemplateStore::with_platform_defaults();
    register_chat_humanise_templates(&mut store);

    for key in [TPL_CHAT_CARD, TPL_CHAT_AGENT_MESSAGE, TPL_CHAT_MENTIONED] {
        let got = store
            .lookup(PLATFORM_DEFAULT_TENANT, key, myelin_notif::DEFAULT_LOCALE)
            .unwrap_or_else(|| panic!("Notif's ONE templating surface serves chat's `{key}`"));
        assert_eq!(got.template_key, key);
        assert!(got.body.contains("{0}"));
    }
}

/// **Chat holds NO private string map — the keys ARE Notif's ICU-subset rows (OQ-L).** The
/// `chat.message.mentioned` body is the per-viewer subject-binding shape; rendering it through the
/// ONE Notif formatter substitutes the bound slot — proving chat's string is a row in the ONE
/// surface, not a chat-local rendered string.
#[test]
fn chat_mentioned_renders_through_the_one_notif_formatter() {
    let rows = chat_humanise_templates();
    let mentioned = rows.iter().find(|r| r.template_key == TPL_CHAT_MENTIONED).expect("mentioned");
    // the ONE Notif ICU-subset formatter binds {0} — chat does not render strings itself.
    let bound = myelin_notif::render_message(&mentioned.body, &["#eng".to_string()]);
    assert_eq!(bound, "You were mentioned in #eng");
}

// ---------------------------------------------------------------------------------------------
// §2 — the define_notif_rule set (contract 7.6): mentioned / replied / thread_watched / approval
// ---------------------------------------------------------------------------------------------

/// **The reason set IS the four frozen chat reasons at their §3.1 bands.** mentioned → direct,
/// replied → participating, thread_watched → watching, approval_requested → critical. A re-band
/// (a `define_notif_rule` reconciliation drop) would have panicked at construction; this pins the
/// accepted result.
#[test]
fn chat_notif_rules_are_the_four_chat_reasons_at_their_bands() {
    let rules = chat_notif_rules();
    assert_eq!(rules.len(), 4, "exactly the four chat reasons");

    let by_key: BTreeMap<&str, &NotifRule> = rules.iter().map(|(k, r)| (*k, r)).collect();

    let m = by_key.get(RULE_KEY_MENTIONED).expect("mentioned");
    assert_eq!(m.reason, Reason::Mentioned);
    assert_eq!(m.default_class, Class::Direct, "@mention is a direct address");

    let r = by_key.get(RULE_KEY_REPLIED).expect("replied");
    assert_eq!(r.reason, Reason::Replied);
    assert_eq!(r.default_class, Class::Participating, "a reply is participation");

    let tw = by_key.get(RULE_KEY_THREAD_WATCHED).expect("thread_watched");
    assert_eq!(tw.reason, Reason::ThreadWatched);
    assert_eq!(tw.default_class, Class::Watching, "a watched thread is ambient");

    let a = by_key.get(RULE_KEY_APPROVAL_REQUESTED).expect("approval_requested");
    assert_eq!(a.reason, Reason::ApprovalRequested);
    assert_eq!(a.default_class, Class::Critical, "an HITL approval pierces (critical)");
}

/// **Each `define_notif_rule` round-trips its dedup template + default class (the prompt's TESTS
/// line).** Rendering each rule's dedup key for a `(recipient, subject)` substitutes the template's
/// placeholders (the §3.2 collapse key), and the rule's `default_class` is exactly its §3.1 band — a
/// round-trip through the frozen verb's value.
#[test]
fn each_chat_rule_round_trips_its_dedup_template_and_default_class() {
    for (key, rule) in chat_notif_rules() {
        // the dedup template renders the §3.2 collapse key for (recipient, subject) — round-trip.
        let dk = rule.dedup_key("psn:alice", &subject());
        assert!(
            dk.contains("psn:alice") && dk.contains("myelin://acme/chat/channel/eng"),
            "rule `{key}` dedup key must bind (recipient, subject): got `{dk}`"
        );
        assert!(dk.starts_with("chat."), "rule `{key}` dedup key is chat-namespaced: `{dk}`");
        // the default_class is the §3.1 table band for the reason (the table owns the band).
        assert_eq!(
            rule.default_class,
            myelin_notif::reason_base_class(rule.reason).1,
            "rule `{key}` default_class must be the §3.1 band for its reason"
        );
    }
}

/// **The reason set registers with Notif and CLASSIFIES (the GATE — the inverse-signal seam).**
/// Registering the set into a platform-default registry and classifying a Signal carrying each
/// `rule_key` routes through the registered chat rule (`from_registered_rule = true`) with the right
/// reason + band + a dedup key collapsing by `(recipient, subject)` — Notif admits + routes chat's
/// rules with ZERO Notif change.
#[test]
fn chat_notif_rules_register_and_classify_through_notif() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    register_chat_notif_rules(&mut reg);
    assert_eq!(reg.len(), before + 4, "the four chat rules accreted (no Notif change)");

    let c = reg.classify(RULE_KEY_MENTIONED, "psn:alice", &subject());
    assert_eq!(c.reason, Reason::Mentioned);
    assert_eq!(c.default_class, Class::Direct);
    assert!(c.from_registered_rule, "the registered chat rule took effect");
    assert_eq!(c.dedup_key, "chat.mentioned:psn:alice:myelin://acme/chat/channel/eng");

    let c = reg.classify(RULE_KEY_APPROVAL_REQUESTED, "psn:bob", &subject());
    assert_eq!(c.reason, Reason::ApprovalRequested);
    assert_eq!(c.default_class, Class::Critical);
    assert!(c.from_registered_rule);
}

/// **Chat cannot smuggle a reason into the wrong band — the table owns the band (OQ1 / NOTIF-D1).**
/// `approval_requested` registered at a non-critical band is rejected LOUDLY by the frozen verb. This
/// pins that chat registers WHICH reason; Notif's §3.1 table owns WHICH band.
#[test]
fn chat_cannot_smuggle_a_reason_into_the_wrong_band() {
    let err = define_notif_rule(
        Reason::ApprovalRequested,
        DedupTpl("{subject}".into()),
        Class::Watching,
    )
    .expect_err("approval_requested must register at the critical band the §3.1 table owns");
    assert!(matches!(err, myelin_notif::DefineRuleError::ClassMismatch { .. }));
}

// ---------------------------------------------------------------------------------------------
// §3 — the fanout-class (arch 03 §4): write-fanout (bounded) vs read-fanout (unbounded ambient)
// ---------------------------------------------------------------------------------------------

/// **The fanout-class is TOTAL over the `chat.*` durable tokens (the prompt's TESTS line / the GATE).**
/// Every durable token classifies into exactly one [`FanoutClass`] — a NEW durable token without a
/// fanout decision would fail this. The callable invariant agrees.
#[test]
fn fanout_class_is_total_over_the_chat_durable_tokens() {
    assert!(
        fanout_class_is_total_over_durable_tokens(),
        "the fanout-class must be total over chat's durable tokens"
    );
    for t in CHAT_DURABLE_TOKENS {
        assert!(fanout_class(t).is_some(), "durable token `{t}` must classify into a fanout class");
    }
    // a non-chat / unregistered token does not classify.
    assert_eq!(fanout_class("git.pr.opened"), None);
    assert_eq!(fanout_class("chat.message.nonexistent"), None);
}

/// **The write-fanout set is the BOUNDED high-signal set; everything else is ambient read-fanout
/// (arch §4).** `chat.message.mentioned` (the canonical write-fanout producer) + `chat.thread.replied`
/// (a reply in your thread) are write-fanout; `chat.channel.member_added` / `chat.message.created` /
/// the snapshots are read-fanout (the unbounded ambient set never write-amplifies — celebrity-fanout).
#[test]
fn write_fanout_is_the_bounded_high_signal_set_rest_is_read_fanout() {
    // the bounded high-signal write-fanout set (per-recipient inbox items).
    assert_eq!(fanout_class(CHAT_MESSAGE_MENTIONED), Some(FanoutClass::WriteFanout));
    assert_eq!(
        fanout_class(crate::events::CHAT_THREAD_REPLIED),
        Some(FanoutClass::WriteFanout)
    );

    // the unbounded ambient read-fanout set — a 100k-member channel post does ZERO per-member writes.
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
    // the firehose ephemeral frames are ambient read-fanout (never per-recipient writes).
    for fh in CHAT_FIREHOSE_TOKENS {
        assert_eq!(fanout_class(fh), Some(FanoutClass::ReadFanout), "firehose `{fh}` is ambient");
    }
}

/// **The write-fanout set is BOUNDED — it is the minted high-signal subset, not the whole durable
/// list.** The bounded-vs-unbounded distinction IS the celebrity-fanout mitigation: the write-fanout
/// set must be a strict, small subset of the durable tokens (else every post would write-amplify).
#[test]
fn the_write_fanout_set_is_a_bounded_subset_of_the_durable_tokens() {
    let write_fanout_count = CHAT_DURABLE_TOKENS
        .iter()
        .filter(|t| fanout_class(t) == Some(FanoutClass::WriteFanout))
        .count();
    assert!(write_fanout_count >= 1, "at least the mention producer is write-fanout");
    assert!(
        write_fanout_count < CHAT_DURABLE_TOKENS.len(),
        "write-fanout must be a STRICT subset (the unbounded ambient set never write-amplifies)"
    );
    // the mention producer is in the write-fanout set (the canonical producer, contract 13.1).
    assert!(CHAT_WRITE_FANOUT_TOKENS.contains(&CHAT_MESSAGE_MENTIONED));
}

// ---------------------------------------------------------------------------------------------
// §4 — the firehose scope shape (contract 3.5): channel:<id>, bounded (never *)
// ---------------------------------------------------------------------------------------------

/// **Chat's firehose scope shape is BOUNDED — `channel:<id>`, never `*` (the prompt's TESTS line /
/// the GATE: 0 unbounded-scope declarations).** A valid channel id parses to a `ScopeKind::Channel`
/// bounded selector through the Bus-owned `*`-rejecting chokepoint; round-trips its selector string.
#[test]
fn chat_firehose_scope_is_bounded_channel_never_star() {
    let scope = chat_channel_scope("eng").expect("a bounded channel scope parses");
    assert_eq!(scope.kind(), ScopeKind::Channel, "chat's per-view scope is channel:<id>");
    assert_eq!(scope.id(), "eng");
    assert_eq!(scope.selector(), "channel:eng", "the channel scope round-trips its selector");
}

/// **An unbounded / `*` chat scope is REJECTED at the chokepoint (0 unbounded-scope declarations).**
/// `*`, an empty channel id, a `*`-containing id are all rejected as over-broad — chat cannot
/// declare an unbounded subscription (the `*`-rejection generalises to `channel:` exactly).
#[test]
fn an_unbounded_chat_scope_is_rejected() {
    for bad in ["*", "", "*all"] {
        let r = chat_channel_scope(bad);
        assert!(r.is_err(), "an unbounded channel scope `channel:{bad}` must be rejected, got {r:?}");
        assert!(
            r.unwrap_err().is_over_broad_scope(),
            "`channel:{bad}` is an over-broad-scope rejection (scope is never *)"
        );
    }
    // a bare `channel:` (no id) is also over-broad (the chokepoint rejects an empty resource id).
    assert!(FirehoseScope::parse("channel:").unwrap_err().is_over_broad_scope());
    // and the raw `*` scope through the chokepoint is rejected.
    assert!(FirehoseScope::parse("*").unwrap_err().is_over_broad_scope());
}

/// **The `resync_required → *.snapshot` fallback contract names chat's durable snapshot tokens
/// (contract 3.5).** The fallback a `resync_required` client cold-rebuilds from is the chat
/// reindex-from-source snapshots (channel/message/thread), each a registered DURABLE token — never a
/// firehose frame (the cold rebuild rides the durable outbox, arch §6).
#[test]
fn the_resync_snapshot_fallback_names_chat_durable_snapshots() {
    assert_eq!(CHAT_RESYNC_SNAPSHOT_TOKENS.len(), 3, "channel/message/thread snapshots");
    for snap in CHAT_RESYNC_SNAPSHOT_TOKENS {
        assert!(
            CHAT_DURABLE_TOKENS.contains(snap),
            "the resync fallback snapshot `{snap}` must be a DURABLE token (rides the outbox)"
        );
        assert!(snap.ends_with(".snapshot"), "`{snap}` is a *.snapshot reindex-from-source token");
    }
}

// ---------------------------------------------------------------------------------------------
// §5 — the TE-21 connection-tier language pin (contract 1.7): Rust default, the shim is a NO-OP
// ---------------------------------------------------------------------------------------------

/// **The TE-21 pin is Rust and the harness shim is a NO-OP today (the GATE — the shim's no-op
/// obligation is satisfied).** The pinned value is Rust; `is_no_op()` is true; the recorded obligation
/// returns the pinned Rust value. The BEAM hatch is written-but-closed (its obligation binds only in
/// CHAT-P26).
#[test]
fn te21_pin_is_rust_and_the_harness_shim_is_a_no_op() {
    assert_eq!(Te21LanguagePin::PINNED, Te21LanguagePin::Rust, "the M2-C0 pin is Rust");
    assert!(Te21LanguagePin::Rust.is_no_op(), "the all-Rust default makes the shim a no-op");
    // the recorded obligation returns the pinned Rust no-op.
    let recorded = te21_harness_shim_obligation();
    assert_eq!(recorded, Te21LanguagePin::Rust);
    assert!(recorded.is_no_op(), "the recorded TE-21 obligation is a no-op against the 1.7 shim");

    // the BEAM hatch is written-but-CLOSED: it exists as a variant but is NOT a no-op (its
    // cross-language harness-shim obligations bind when selected — CHAT-P26).
    assert!(!Te21LanguagePin::Beam.is_no_op(), "the BEAM hatch carries the shim obligation (CHAT-P26)");
}
