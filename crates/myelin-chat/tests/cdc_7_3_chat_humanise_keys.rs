use myelin_chat::glue::{
    chat_humanise_templates, register_chat_humanise_templates, TPL_CHAT_AGENT_MESSAGE,
    TPL_CHAT_CARD, TPL_CHAT_CARD_FACETS, TPL_CHAT_MENTIONED, TPL_CHAT_PROJECT_CHANNEL,
    TPL_CHAT_PROJECT_MESSAGE, TPL_CHAT_PROJECT_THREAD,
};
use myelin_notif::{
    render_message, HumaniseTemplate, TemplateStore, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT,
};

fn provider_chat_humanise_rows() -> Vec<HumaniseTemplate> {
    chat_humanise_templates()
}

fn consumer_admits_and_serves(rows: &[HumaniseTemplate]) -> TemplateStore {
    let mut store = TemplateStore::with_platform_defaults();
    for row in rows {
        store.put(row.clone());
    }
    store
}

#[test]
fn cdc_7_3_chat_provider_registers_keys_consumer_admits_and_serves() {
    let rows = provider_chat_humanise_rows();
    assert_eq!(
        rows.len(),
        7,
        "chat registers exactly the seven humanise surfaces (card subject + card facets + agent-message + mentioned + the three project(ref,viewer) title surfaces)"
    );

    let store = consumer_admits_and_serves(&rows);
    for key in [
        TPL_CHAT_CARD,
        TPL_CHAT_CARD_FACETS,
        TPL_CHAT_AGENT_MESSAGE,
        TPL_CHAT_MENTIONED,
        TPL_CHAT_PROJECT_CHANNEL,
        TPL_CHAT_PROJECT_MESSAGE,
        TPL_CHAT_PROJECT_THREAD,
    ] {
        let served = store
            .lookup(PLATFORM_DEFAULT_TENANT, key, DEFAULT_LOCALE)
            .unwrap_or_else(|| panic!("Notif's ONE templating surface must serve chat's `{key}`"));
        assert_eq!(served.template_key, key);
        assert!(
            served.body.contains("{0}"),
            "`{key}` must bind the {{0}} subject slot"
        );
    }

    let mut store2 = TemplateStore::with_platform_defaults();
    register_chat_humanise_templates(&mut store2);
    assert!(store2
        .lookup(PLATFORM_DEFAULT_TENANT, TPL_CHAT_MENTIONED, DEFAULT_LOCALE)
        .is_some());
}

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
