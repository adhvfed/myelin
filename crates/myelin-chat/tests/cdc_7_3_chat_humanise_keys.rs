//! # The CDC pair for contract 7.3 — chat's humanise keys (CHAT-P3 / P-245)
//!
//! **Contract:** `contract-index.md` row 7.3 (`humanise(item|(template_key, args), viewer, locale)
//! → HumanisedString` — the ONE templating surface; every consumer + every agent-authored message +
//! every subsystem registers here, OQ-L). **Reconciliation:** `00-reconciliation-decisions.md` OQ-L
//! (humanise is the ONE templating surface — no second template engine). Owning architecture: chat
//! `03-events-contracts-and-glue.md` §1.1 (the card / agent-message / `chat.message.mentioned`
//! string registration) + §3 (the title humanised via `humanise`, the sole templating surface).
//!
//! ## The seam this pair pins (chat REGISTERS keys; Notif owns the templating surface)
//! - **PROVIDER (chat — [`myelin_chat::glue`])** REGISTERS its humanise template keys (the card
//!   string, the agent-message string, the `chat.message.mentioned` string) — NO chat-private string
//!   map (OQ-L). Every key is a row in Notif's ONE templating surface.
//! - **CONSUMER (Notif — [`myelin_notif::TemplateStore`] / [`myelin_notif::render_message`])** ADMITS
//!   chat's rows into the ONE templating store and RENDERS them through the ONE ICU-subset formatter —
//!   chat does not author a second templating engine, and chat does not render strings itself.

use myelin_chat::glue::{
    chat_humanise_templates, register_chat_humanise_templates, TPL_CHAT_AGENT_MESSAGE,
    TPL_CHAT_CARD, TPL_CHAT_MENTIONED,
};
use myelin_notif::{
    render_message, HumaniseTemplate, TemplateStore, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT,
};

/// **PROVIDER side of 7.3** — chat registers exactly its three humanise keys (card / agent-message /
/// mentioned) as rows in the ONE templating surface. The provider's promise: chat holds no private
/// string map; every chat string is a Notif templating-surface row with a `{0}` per-viewer subject.
fn provider_chat_humanise_rows() -> Vec<HumaniseTemplate> {
    chat_humanise_templates()
}

/// **CONSUMER side of 7.3** — Notif's ONE templating surface ADMITS chat's rows and serves them by
/// `(tenant|default, key, locale)`. The consumer's promise: it admits chat's keys into the SAME
/// store every other subsystem registers into (no second engine), and renders them per the ONE
/// ICU-subset formatter.
fn consumer_admits_and_serves(rows: &[HumaniseTemplate]) -> TemplateStore {
    let mut store = TemplateStore::with_platform_defaults();
    for row in rows {
        store.put(row.clone());
    }
    store
}

/// The 7.3 pair, end-to-end: the PROVIDER (chat) registers its three humanise keys, and the CONSUMER
/// (Notif's ONE templating surface) ADMITS + SERVES each — the dated green artifact for the chat
/// humanise registration (the contract-coverage scanner's 7.3 chat row).
#[test]
fn cdc_7_3_chat_provider_registers_keys_consumer_admits_and_serves() {
    let rows = provider_chat_humanise_rows();
    assert_eq!(
        rows.len(),
        3,
        "chat registers exactly the three humanise surfaces"
    );

    let store = consumer_admits_and_serves(&rows);
    for key in [TPL_CHAT_CARD, TPL_CHAT_AGENT_MESSAGE, TPL_CHAT_MENTIONED] {
        let served = store
            .lookup(PLATFORM_DEFAULT_TENANT, key, DEFAULT_LOCALE)
            .unwrap_or_else(|| panic!("Notif's ONE templating surface must serve chat's `{key}`"));
        assert_eq!(served.template_key, key);
        // every chat string binds the {0} per-viewer subject slot (permission/erasure-safe by ctor).
        assert!(
            served.body.contains("{0}"),
            "`{key}` must bind the {{0}} subject slot"
        );
    }

    // the fluent register helper agrees (the production registration call).
    let mut store2 = TemplateStore::with_platform_defaults();
    register_chat_humanise_templates(&mut store2);
    assert!(store2
        .lookup(PLATFORM_DEFAULT_TENANT, TPL_CHAT_MENTIONED, DEFAULT_LOCALE)
        .is_some());
}

/// The CONSUMER renders a chat key through the ONE Notif ICU-subset formatter — chat does NOT render
/// strings itself (OQ-L). Binding `{0}` substitutes the per-viewer subject; the SAME formatter every
/// subsystem's strings lower through. Proves chat's string is a row in the ONE surface, not chat-local.
#[test]
fn cdc_7_3_consumer_renders_chat_key_through_the_one_formatter() {
    let rows = provider_chat_humanise_rows();
    let card = rows
        .iter()
        .find(|r| r.template_key == TPL_CHAT_CARD)
        .expect("the chat card key");
    let bound = render_message(&card.body, &["#incidents".to_string()]);
    assert!(
        bound.contains("#incidents"),
        "the ONE formatter binds the subject: `{bound}`"
    );
}
