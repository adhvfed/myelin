use myelin_events::{validate_event_type, TaxonomyError};
use myelin_git::events::{register_git_tokens, GIT_EVENT_TOKENS, GIT_REF_UPDATED};

fn provider_registers_git_tokens() -> &'static [&'static str] {
    GIT_EVENT_TOKENS
}

fn consumer_admits(type_name: &str) -> bool {
    validate_event_type(type_name).is_ok()
}

#[test]
fn cdc_2_9_git_provider_registers_consumer_admits_every_token() {
    for &tok in provider_registers_git_tokens() {
        assert!(
            consumer_admits(tok),
            "consumer (Bus validator) wrongly REJECTED registered git token `{tok}`: {:?}",
            validate_event_type(tok)
        );
    }
    assert!(
        register_git_tokens().is_ok(),
        "Git's register_git_tokens() must be green: {:?}",
        register_git_tokens()
    );
}

#[test]
fn cdc_2_9_consumer_rejects_a_malformed_git_type_loudly() {
    assert!(matches!(
        validate_event_type("git.pr.open"),
        Err(TaxonomyError::PresentTenseVerb { .. })
    ));
    assert!(matches!(
        validate_event_type("git.comments.created"),
        Err(TaxonomyError::PluralToken { .. })
    ));
    assert!(matches!(
        validate_event_type("git.PR.opened"),
        Err(TaxonomyError::BadToken { .. })
    ));
}

#[test]
fn cdc_2_9_git_registers_only_its_own_subsystem() {
    for &tok in provider_registers_git_tokens() {
        assert!(
            tok.starts_with("git."),
            "git registered the foreign-subsystem token `{tok}` (must own `git.*` only)"
        );
    }
    assert!(provider_registers_git_tokens().contains(&GIT_REF_UPDATED));
}
