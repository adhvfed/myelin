use myelin_chat::events::{
    chat_event_tokens, delivery_class, register_chat_tokens, split_is_disjoint_and_total,
    DeliveryClass, CHAT_DURABLE_TOKENS, CHAT_FIREHOSE_TOKENS, CHAT_MESSAGE_CREATED,
    CHAT_PRESENCE_CHANGED,
};
use myelin_events::{validate_event_type, TaxonomyError};

fn provider_registers_chat_tokens() -> Vec<&'static str> {
    chat_event_tokens()
}

fn consumer_admits(type_name: &str) -> bool {
    validate_event_type(type_name).is_ok()
}

#[test]
fn cdc_2_9_chat_provider_registers_consumer_admits_every_token() {
    for tok in provider_registers_chat_tokens() {
        assert!(
            consumer_admits(tok),
            "consumer (Bus validator) wrongly REJECTED registered chat token `{tok}`: {:?}",
            validate_event_type(tok)
        );
    }
    assert!(
        register_chat_tokens().is_ok(),
        "Chat's register_chat_tokens() must be green: {:?}",
        register_chat_tokens()
    );
}

#[test]
fn cdc_2_9_consumer_rejects_a_malformed_chat_type_loudly() {
    assert!(matches!(
        validate_event_type("chat.message.create"),
        Err(TaxonomyError::PresentTenseVerb { .. })
    ));
    assert!(matches!(
        validate_event_type("chat.messages.created"),
        Err(TaxonomyError::PluralToken { .. })
    ));
    assert!(matches!(
        validate_event_type("chat.Message.created"),
        Err(TaxonomyError::BadToken { .. })
    ));
}

#[test]
fn cdc_2_9_chat_registers_only_its_own_subsystem() {
    for tok in provider_registers_chat_tokens() {
        assert!(
            tok.starts_with("chat."),
            "chat registered the foreign-subsystem token `{tok}` (must own `chat.*` only)"
        );
    }
    assert!(provider_registers_chat_tokens().contains(&CHAT_MESSAGE_CREATED));
}

#[test]
fn cdc_2_9_chat_durable_firehose_split_is_disjoint_and_total() {
    assert!(
        split_is_disjoint_and_total(),
        "the chat durable/firehose split must be disjoint AND total (0 misclassified tokens)"
    );
    assert_eq!(
        delivery_class(CHAT_MESSAGE_CREATED),
        Some(DeliveryClass::Durable)
    );
    assert_eq!(
        delivery_class(CHAT_PRESENCE_CHANGED),
        Some(DeliveryClass::Firehose)
    );
    assert_eq!(
        CHAT_DURABLE_TOKENS.len() + CHAT_FIREHOSE_TOKENS.len(),
        chat_event_tokens().len()
    );
}
