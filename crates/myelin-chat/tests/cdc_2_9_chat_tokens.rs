//! # The CDC pair for contract 2.9 — Chat's `chat.*` token registration (CHAT-P1 / P-128)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 2.9
//! (Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`; **each subsystem
//! completes its list**). Owning architecture: Chat
//! `04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md` §1.1 (the
//! DURABLE-via-outbox `chat.*` set) / §1.2 (the FIREHOSE-only `chat.*` set); Bus `event-bus.md`
//! §6.1 (the grammar — the AUTHORITY), §6.2 (the subsystem/type token table).
//!
//! ## The seam this pair pins (chat registers; the Bus owns the grammar)
//! Row 2.9 is the seam between the side that OWNS + registers a subsystem's dotted-name list (the
//! **PROVIDER** — here Chat, [`myelin_chat::events`]) and the side that owns the grammar + validates
//! every registered name (the **CONSUMER** — the one Bus validator,
//! [`myelin_events::validate_event_type`]). The frozen behaviour both sides agree on:
//!
//! - the PROVIDER (Chat) registers its COMPLETE v1 `chat.*` list (durable ∪ firehose) — every name
//!   of the §6.1 shape (lowercase, singular, past-tense, `[a-z][a-z0-9_]*` tokens, 2-or-3 segments,
//!   the leading token the canonical `chat` subsystem), partitioned into a DURABLE class (the only
//!   class that may ride `OutboxTx::emit`) and a FIREHOSE class (never the durable bus), and
//!   registers NO foreign-subsystem token;
//! - the CONSUMER (the Bus validator) ADMITS every registered chat name (0 ungrammatical) and would
//!   REJECT a malformed chat-shaped name LOUDLY — chat does not get to author the grammar.
//!
//! This is the dedicated 2.9 provider+consumer pair the CHAT-P1 TESTS field names; the focused
//! per-token round-trip + split-disjoint/total fixtures live in `myelin_chat::events::tests`.

use myelin_chat::events::{
    chat_event_tokens, delivery_class, register_chat_tokens, split_is_disjoint_and_total,
    DeliveryClass, CHAT_DURABLE_TOKENS, CHAT_FIREHOSE_TOKENS, CHAT_MESSAGE_CREATED,
    CHAT_PRESENCE_CHANGED,
};
use myelin_events::{validate_event_type, TaxonomyError};

/// **PROVIDER side of 2.9** — Chat, the owner, registers its complete `chat.*` list (durable ∪
/// firehose). The provider's promise: every `type` token it puts on the wire is one of these,
/// grammar-conformant by construction, and correctly classed durable-vs-firehose. Returns the
/// registry the consumer validates.
fn provider_registers_chat_tokens() -> Vec<&'static str> {
    chat_event_tokens()
}

/// **CONSUMER side of 2.9** — the one Bus grammar validator every consumer (and the Bus itself)
/// runs a `type` through. It ADMITS a canonical name and REJECTS a malformed one. The consumer's
/// promise: it never silently accepts a non-conformant `type`.
fn consumer_admits(type_name: &str) -> bool {
    validate_event_type(type_name).is_ok()
}

/// The 2.9 pair, end-to-end: the PROVIDER (Chat) registers its complete list, and the CONSUMER (the
/// Bus validator) admits **every** registered token — 0 ungrammatical. This is the dated green
/// artifact the CHAT-P1 GATE names.
#[test]
fn cdc_2_9_chat_provider_registers_consumer_admits_every_token() {
    for tok in provider_registers_chat_tokens() {
        assert!(
            consumer_admits(tok),
            "consumer (Bus validator) wrongly REJECTED registered chat token `{tok}`: {:?}",
            validate_event_type(tok)
        );
    }
    // The whole-list registration helper is the provider's one-call assertion (0 ungrammatical).
    assert!(
        register_chat_tokens().is_ok(),
        "Chat's register_chat_tokens() must be green: {:?}",
        register_chat_tokens()
    );
}

/// The CONSUMER validator REJECTS a malformed chat-shaped `type` LOUDLY (the specific
/// [`TaxonomyError`] for the broken rule), never silently coerced — chat does NOT get to author the
/// grammar. The negative half of the seam: the validator is a real gate, not a pass-through.
#[test]
fn cdc_2_9_consumer_rejects_a_malformed_chat_type_loudly() {
    // present-tense verb (chat.message.create, not created)
    assert!(matches!(
        validate_event_type("chat.message.create"),
        Err(TaxonomyError::PresentTenseVerb { .. })
    ));
    // plural artifact-type token (chat.messages.created)
    assert!(matches!(
        validate_event_type("chat.messages.created"),
        Err(TaxonomyError::PluralToken { .. })
    ));
    // uppercase token
    assert!(matches!(
        validate_event_type("chat.Message.created"),
        Err(TaxonomyError::BadToken { .. })
    ));
}

/// The PROVIDER registers NO foreign-subsystem token — chat does NOT register `agent.*` (the
/// Agent-owned `agent.message.partial` firehose frame is the agent subsystem's, arch §1.2) nor
/// `refs.*` / `ci.*` echoes. The acyclic-producer invariant (EI-02 §3), pinned at the contract seam.
#[test]
fn cdc_2_9_chat_registers_only_its_own_subsystem() {
    for tok in provider_registers_chat_tokens() {
        assert!(
            tok.starts_with("chat."),
            "chat registered the foreign-subsystem token `{tok}` (must own `chat.*` only)"
        );
    }
    // The load-bearing durable message-created event is present under its named constant (X-5).
    assert!(provider_registers_chat_tokens().contains(&CHAT_MESSAGE_CREATED));
}

/// **The DURABLE / FIREHOSE split is part of the 2.9 contract** (arch §1.1 vs §1.2): the provider's
/// classification is DISJOINT (no token in both classes) and TOTAL (every token classifies). The
/// durable class is the ONLY class that may ride `OutboxTx::emit`; the firehose class never touches
/// the durable bus. 0 misclassified tokens — the structural gate, pinned at the seam.
#[test]
fn cdc_2_9_chat_durable_firehose_split_is_disjoint_and_total() {
    assert!(
        split_is_disjoint_and_total(),
        "the chat durable/firehose split must be disjoint AND total (0 misclassified tokens)"
    );
    // The representative durable token classes durable; the representative firehose token firehose.
    assert_eq!(delivery_class(CHAT_MESSAGE_CREATED), Some(DeliveryClass::Durable));
    assert_eq!(delivery_class(CHAT_PRESENCE_CHANGED), Some(DeliveryClass::Firehose));
    // The two classes partition the registry exactly (sizes add up — no token lost/double-counted).
    assert_eq!(
        CHAT_DURABLE_TOKENS.len() + CHAT_FIREHOSE_TOKENS.len(),
        chat_event_tokens().len()
    );
}
