use myelin_events::validate_event_type;
use myelin_events::AggregateKey;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};

pub fn event_actor_pseudonym(tenant: &str, subject: &str) -> String {
    event_actor_field_pseudonym("principal", tenant, subject)
}

fn event_actor_field_pseudonym(field: &str, tenant: &str, subject: &str) -> String {
    let digest = blake3::hash(
        format!("myelin.chat.event-actor.v1\0{field}\0{tenant}\0{subject}").as_bytes(),
    );
    format!("chat-author:{}", &digest.to_hex()[..32])
}

pub fn pseudonymized_event_principal(tenant: &str, principal: &Principal) -> Principal {
    let mut projected = principal.clone();
    projected.principal_id = PrincipalId(event_actor_pseudonym(tenant, &principal.principal_id.0));
    if let PrincipalKind::Agent {
        runtime_ref,
        on_behalf_of,
    } = &principal.kind
    {
        projected.kind = PrincipalKind::Agent {
            runtime_ref: RuntimeRef(event_actor_field_pseudonym(
                "runtime-ref",
                tenant,
                &runtime_ref.0,
            )),
            on_behalf_of: on_behalf_of.as_ref().map(|delegator| {
                PrincipalId(event_actor_field_pseudonym(
                    "on-behalf-of",
                    tenant,
                    &delegator.0,
                ))
            }),
        };
    }
    projected
}

pub const CHAT_MESSAGE_CREATED: &str = "chat.message.created";
pub const CHAT_MESSAGE_EDITED: &str = "chat.message.edited";
pub const CHAT_MESSAGE_DELETED: &str = "chat.message.deleted";
pub const CHAT_MESSAGE_ERASED: &str = "chat.message.erased";
pub const CHAT_MESSAGE_MENTIONED: &str = "chat.message.mentioned";

pub const CHAT_REACTION_ADDED: &str = "chat.reaction.added";
pub const CHAT_REACTION_REMOVED: &str = "chat.reaction.removed";

pub const CHAT_THREAD_CREATED: &str = "chat.thread.created";
pub const CHAT_THREAD_REPLIED: &str = "chat.thread.replied";

// the ONE canonical chat ordering partition: every channel-scoped event
// (channel lifecycle, membership, messages) shares `channel:<conversation>`
// so the relay orders them per conversation and the stream subject stays
// in the canonical `type:id` aggregate form the publisher requires.
pub fn channel_aggregate(conversation_id: &str) -> AggregateKey {
    AggregateKey(format!("channel:{conversation_id}"))
}

pub const CHAT_CHANNEL_CREATED: &str = "chat.channel.created";
pub const CHAT_CHANNEL_ARCHIVED: &str = "chat.channel.archived";
pub const CHAT_CHANNEL_MEMBER_ADDED: &str = "chat.channel.member_added";
pub const CHAT_CHANNEL_MEMBER_REMOVED: &str = "chat.channel.member_removed";
pub const CHAT_CHANNEL_LINKED: &str = "chat.channel.linked";

pub const CHAT_READ_STATE_UPDATED: &str = "chat.read_state.updated";

pub const CHAT_POST_ATTEMPTED: &str = "chat.post.attempted";
pub const CHAT_POST_APPLIED: &str = "chat.post.applied";
pub const CHAT_POST_GATED: &str = "chat.post.gated";
pub const CHAT_POST_DENIED: &str = "chat.post.denied";
pub const CHAT_POST_INDETERMINATE: &str = "chat.post.indeterminate";

pub const CHAT_GOVERNANCE_AUDIT_EVENT_TOKENS: &[&str] = &[
    CHAT_POST_ATTEMPTED,
    CHAT_POST_APPLIED,
    CHAT_POST_GATED,
    CHAT_POST_DENIED,
    CHAT_POST_INDETERMINATE,
];

pub const CHAT_CHANNEL_SNAPSHOT: &str = "chat.channel.snapshot";
pub const CHAT_MESSAGE_SNAPSHOT: &str = "chat.message.snapshot";
pub const CHAT_THREAD_SNAPSHOT: &str = "chat.thread.snapshot";

pub const CHAT_DURABLE_TOKENS: &[&str] = &[
    CHAT_MESSAGE_CREATED,
    CHAT_MESSAGE_EDITED,
    CHAT_MESSAGE_DELETED,
    CHAT_MESSAGE_ERASED,
    CHAT_MESSAGE_MENTIONED,
    CHAT_REACTION_ADDED,
    CHAT_REACTION_REMOVED,
    CHAT_THREAD_CREATED,
    CHAT_THREAD_REPLIED,
    CHAT_CHANNEL_CREATED,
    CHAT_CHANNEL_ARCHIVED,
    CHAT_CHANNEL_MEMBER_ADDED,
    CHAT_CHANNEL_MEMBER_REMOVED,
    CHAT_CHANNEL_LINKED,
    CHAT_READ_STATE_UPDATED,
    CHAT_POST_ATTEMPTED,
    CHAT_POST_APPLIED,
    CHAT_POST_GATED,
    CHAT_POST_DENIED,
    CHAT_POST_INDETERMINATE,
    CHAT_CHANNEL_SNAPSHOT,
    CHAT_MESSAGE_SNAPSHOT,
    CHAT_THREAD_SNAPSHOT,
];

pub const CHAT_PRESENCE_CHANGED: &str = "chat.presence.changed";
pub const CHAT_TYPING_STARTED: &str = "chat.typing.started";
pub const CHAT_TYPING_STOPPED: &str = "chat.typing.stopped";
pub const CHAT_READ_STATE_VIEWED: &str = "chat.read_state.viewed";

pub const CHAT_FIREHOSE_TOKENS: &[&str] = &[
    CHAT_PRESENCE_CHANGED,
    CHAT_TYPING_STARTED,
    CHAT_TYPING_STOPPED,
    CHAT_READ_STATE_VIEWED,
];

pub fn chat_event_tokens() -> Vec<&'static str> {
    CHAT_DURABLE_TOKENS
        .iter()
        .chain(CHAT_FIREHOSE_TOKENS.iter())
        .copied()
        .collect()
}

pub fn register_chat_tokens() -> Result<(), (&'static str, myelin_events::TaxonomyError)> {
    for tok in chat_event_tokens() {
        validate_event_type(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryClass {
    Durable,
    Firehose,
}

pub fn delivery_class(token: &str) -> Option<DeliveryClass> {
    if CHAT_DURABLE_TOKENS.contains(&token) {
        Some(DeliveryClass::Durable)
    } else if CHAT_FIREHOSE_TOKENS.contains(&token) {
        Some(DeliveryClass::Firehose)
    } else {
        None
    }
}

pub fn split_is_disjoint_and_total() -> bool {
    let disjoint = !CHAT_DURABLE_TOKENS
        .iter()
        .any(|d| CHAT_FIREHOSE_TOKENS.contains(d));
    let total = chat_event_tokens()
        .iter()
        .all(|t| delivery_class(t).is_some());
    disjoint && total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chat_token_parses_the_bus_grammar() {
        for tok in chat_event_tokens() {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered chat token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        assert!(
            register_chat_tokens().is_ok(),
            "register_chat_tokens() must succeed: {:?}",
            register_chat_tokens()
        );
    }

    #[test]
    fn every_chat_token_carries_the_chat_subsystem_prefix() {
        for tok in chat_event_tokens() {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(
                head, "chat",
                "token `{tok}` must carry the `chat` subsystem prefix"
            );
        }
        assert!(
            myelin_events::SUBSYSTEM_TOKENS.contains(&"chat"),
            "`chat` must be a canonical Bus subsystem token"
        );
    }

    #[test]
    fn the_chat_token_registry_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for tok in chat_event_tokens() {
            assert!(
                seen.insert(tok),
                "chat token `{tok}` is registered more than once"
            );
        }
        assert_eq!(seen.len(), chat_event_tokens().len());
    }

    #[test]
    fn the_durable_firehose_split_is_disjoint_and_total() {
        assert!(
            split_is_disjoint_and_total(),
            "the durable/firehose split must be disjoint AND total"
        );
        for d in CHAT_DURABLE_TOKENS {
            assert!(
                !CHAT_FIREHOSE_TOKENS.contains(d),
                "token `{d}` is in BOTH the durable and firehose sets (misclassified)"
            );
        }
        for t in chat_event_tokens() {
            assert!(
                delivery_class(t).is_some(),
                "token `{t}` does not classify into a delivery class (split not total)"
            );
        }
        assert_eq!(
            CHAT_DURABLE_TOKENS.len() + CHAT_FIREHOSE_TOKENS.len(),
            chat_event_tokens().len(),
            "the durable + firehose sizes must partition the union exactly"
        );
    }

    #[test]
    fn delivery_class_matches_the_architecture_tables() {
        for d in CHAT_DURABLE_TOKENS {
            assert_eq!(
                delivery_class(d),
                Some(DeliveryClass::Durable),
                "`{d}` must be Durable"
            );
        }
        for f in CHAT_FIREHOSE_TOKENS {
            assert_eq!(
                delivery_class(f),
                Some(DeliveryClass::Firehose),
                "`{f}` must be Firehose"
            );
        }
        assert_eq!(delivery_class("git.pr.opened"), None);
        assert_eq!(delivery_class("chat.message.nonexistent"), None);
    }

    #[test]
    fn the_load_bearing_chat_tokens_are_registered() {
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_MESSAGE_CREATED));
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_MESSAGE_MENTIONED));
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_CHANNEL_MEMBER_ADDED));
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_CHANNEL_MEMBER_REMOVED));
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_MESSAGE_ERASED));
        assert!(CHAT_DURABLE_TOKENS.contains(&CHAT_MESSAGE_SNAPSHOT));
        for token in CHAT_GOVERNANCE_AUDIT_EVENT_TOKENS {
            assert!(CHAT_DURABLE_TOKENS.contains(token));
        }
        assert!(CHAT_FIREHOSE_TOKENS.contains(&CHAT_PRESENCE_CHANGED));
        assert!(CHAT_FIREHOSE_TOKENS.contains(&CHAT_READ_STATE_VIEWED));
    }

    #[test]
    fn chat_registers_no_foreign_subsystem_tokens() {
        for tok in chat_event_tokens() {
            assert!(
                tok.starts_with("chat."),
                "chat must not register the foreign-subsystem token `{tok}`"
            );
        }
    }

    #[test]
    fn read_state_coarse_is_durable_fine_is_firehose() {
        assert_eq!(
            delivery_class(CHAT_READ_STATE_UPDATED),
            Some(DeliveryClass::Durable)
        );
        assert_eq!(
            delivery_class(CHAT_READ_STATE_VIEWED),
            Some(DeliveryClass::Firehose)
        );
    }
}
