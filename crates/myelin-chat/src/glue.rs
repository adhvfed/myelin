use myelin_events::{FirehoseError, FirehoseScope, ScopeKind};
use myelin_notif::{
    define_notif_rule, Class, DedupTpl, HumaniseTemplate, NotifRule, NotifRuleRegistry, Reason,
    TemplateStore, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT,
};

use crate::events::{
    CHAT_CHANNEL_SNAPSHOT, CHAT_DURABLE_TOKENS, CHAT_FIREHOSE_TOKENS, CHAT_MESSAGE_MENTIONED,
    CHAT_MESSAGE_SNAPSHOT, CHAT_THREAD_SNAPSHOT,
};

pub const TPL_CHAT_CARD: &str = "chat.card";

pub const TPL_CHAT_CARD_FACETS: &str = "chat.card.facets";

pub const CARD_FACET_ACTION: usize = 0;
pub const CARD_FACET_RISK: usize = 1;
pub const CARD_FACET_COST: usize = 2;

pub const TPL_CHAT_AGENT_MESSAGE: &str = "chat.agent_message";

pub const TPL_CHAT_MENTIONED: &str = "chat.message.mentioned";

pub const TPL_CHAT_PROJECT_CHANNEL: &str = "chat.project.channel";

pub const TPL_CHAT_PROJECT_MESSAGE: &str = "chat.project.message";

pub const TPL_CHAT_PROJECT_THREAD: &str = "chat.project.thread";

pub fn chat_humanise_templates() -> Vec<HumaniseTemplate> {
    let row = |key: &str, body: &str, icon: &str| HumaniseTemplate {
        tenant: PLATFORM_DEFAULT_TENANT.to_string(),
        template_key: key.to_string(),
        locale: myelin_notif::DEFAULT_LOCALE.to_string(),
        body: body.to_string(),
        icon: icon.to_string(),
    };
    vec![
        row(TPL_CHAT_CARD, "Approval requested on {0}", "approval"),
        row(TPL_CHAT_CARD_FACETS, "**{0}** ({1}, ~{2})", "approval"),
        row(TPL_CHAT_AGENT_MESSAGE, "An agent posted in {0}", "agent"),
        row(TPL_CHAT_MENTIONED, "You were mentioned in {0}", "mention"),
        row(TPL_CHAT_PROJECT_CHANNEL, "{0}", "channel"),
        row(TPL_CHAT_PROJECT_MESSAGE, "{0}", "message"),
        row(
            TPL_CHAT_PROJECT_THREAD,
            "{0} ({1, plural, one {# reply} other {# replies}})",
            "thread",
        ),
    ]
}

pub fn register_chat_humanise_templates(store: &mut TemplateStore) -> &mut TemplateStore {
    for row in chat_humanise_templates() {
        store.put(row);
    }
    store
}

pub fn chat_hitl_card_facets(
    store: &TemplateStore,
    action: &str,
    risk: &str,
    cost: &str,
) -> String {
    let body = store
        .lookup(
            PLATFORM_DEFAULT_TENANT,
            TPL_CHAT_CARD_FACETS,
            DEFAULT_LOCALE,
        )
        .map(|t| t.body.clone())
        .unwrap_or_else(|| "{0} ({1}, ~{2})".to_string());
    myelin_notif::render_message(
        &body,
        &[action.to_string(), risk.to_string(), cost.to_string()],
    )
}

pub const RULE_KEY_MENTIONED: &str = "chat.message.mentioned";
pub const RULE_KEY_REPLIED: &str = "chat.thread.replied";
pub const RULE_KEY_THREAD_WATCHED: &str = "chat.thread.watched";
pub const RULE_KEY_APPROVAL_REQUESTED: &str = "chat.approval.requested";

pub fn chat_notif_rules() -> Vec<(&'static str, NotifRule)> {
    vec![
        (
            RULE_KEY_MENTIONED,
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("chat.mentioned:{recipient}:{subject}".to_string()),
                Class::Direct,
            )
            .expect("Reason::Mentioned reconciles to Class::Direct in the §3.1 table"),
        ),
        (
            RULE_KEY_REPLIED,
            define_notif_rule(
                Reason::Replied,
                DedupTpl("chat.replied:{recipient}:{subject}".to_string()),
                Class::Participating,
            )
            .expect("Reason::Replied reconciles to Class::Participating in the §3.1 table"),
        ),
        (
            RULE_KEY_THREAD_WATCHED,
            define_notif_rule(
                Reason::ThreadWatched,
                DedupTpl("chat.thread_watched:{recipient}:{subject}".to_string()),
                Class::Watching,
            )
            .expect("Reason::ThreadWatched reconciles to Class::Watching in the §3.1 table"),
        ),
        (
            RULE_KEY_APPROVAL_REQUESTED,
            define_notif_rule(
                Reason::ApprovalRequested,
                DedupTpl("chat.approval:{recipient}:{subject}".to_string()),
                Class::Critical,
            )
            .expect("Reason::ApprovalRequested reconciles to Class::Critical in the §3.1 table"),
        ),
    ]
}

pub fn register_chat_notif_rules(registry: &mut NotifRuleRegistry) -> &mut NotifRuleRegistry {
    for (key, rule) in chat_notif_rules() {
        registry.register(key, rule);
    }
    registry
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FanoutClass {
    WriteFanout,
    ReadFanout,
}

const CHAT_WRITE_FANOUT_TOKENS: &[&str] =
    &[CHAT_MESSAGE_MENTIONED, crate::events::CHAT_THREAD_REPLIED];

pub fn fanout_class(token: &str) -> Option<FanoutClass> {
    if CHAT_WRITE_FANOUT_TOKENS.contains(&token) {
        Some(FanoutClass::WriteFanout)
    } else if CHAT_DURABLE_TOKENS.contains(&token) || CHAT_FIREHOSE_TOKENS.contains(&token) {
        Some(FanoutClass::ReadFanout)
    } else {
        None
    }
}

pub fn fanout_class_is_total_over_durable_tokens() -> bool {
    CHAT_DURABLE_TOKENS
        .iter()
        .all(|t| fanout_class(t).is_some())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentDispatchClass {
    NotifyOnly,
    ExplicitDispatch,
}

pub fn agent_dispatch_class(token: &str, is_explicit_action: bool) -> AgentDispatchClass {
    if token == CHAT_MESSAGE_MENTIONED {
        return AgentDispatchClass::NotifyOnly;
    }
    if is_explicit_action {
        AgentDispatchClass::ExplicitDispatch
    } else {
        AgentDispatchClass::NotifyOnly
    }
}

pub const CHAT_FIREHOSE_STREAM_PREFIX: &str = "fan";

pub fn chat_channel_scope(channel_id: &str) -> Result<FirehoseScope, FirehoseError> {
    let scope = FirehoseScope::parse(&format!("channel:{channel_id}"))?;
    debug_assert_eq!(
        scope.kind(),
        ScopeKind::Channel,
        "chat's per-view scope is channel:<id>"
    );
    Ok(scope)
}

pub const CHAT_RESYNC_SNAPSHOT_TOKENS: &[&str] = &[
    CHAT_CHANNEL_SNAPSHOT,
    CHAT_MESSAGE_SNAPSHOT,
    CHAT_THREAD_SNAPSHOT,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Te21LanguagePin {
    Rust,
    Beam,
}

impl Te21LanguagePin {
    pub const PINNED: Te21LanguagePin = Te21LanguagePin::Rust;

    pub fn is_no_op(self) -> bool {
        matches!(self, Te21LanguagePin::Rust)
    }
}

pub fn te21_harness_shim_obligation() -> Te21LanguagePin {
    let pin = Te21LanguagePin::PINNED;
    debug_assert!(
        pin.is_no_op(),
        "the M2-C0 TE-21 pin is Rust - the cross-language harness shim is a NO-OP (the BEAM hatch is closed)"
    );
    pin
}

#[cfg(test)]
mod tests;
