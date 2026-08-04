use myelin_events::{validate_event_type, TaxonomyError};
use myelin_issues::events::unit_check::{validate_issue_payload_units, UnitError};
use myelin_issues::events::{
    register_issue_tokens, INITIATIVE_HEALTH_CHANGED, ISSUE_EVENT_TOKENS, ISSUE_TRANSITIONED,
    ISSUE_UPDATED, RELATION_CREATED, SLA_AT_RISK,
};

fn provider_registers_issue_tokens() -> &'static [&'static str] {
    ISSUE_EVENT_TOKENS
}

fn consumer_admits(type_name: &str) -> bool {
    validate_event_type(type_name).is_ok()
}

#[test]
fn cdc_2_9_issues_provider_registers_consumer_admits_every_token() {
    for &tok in provider_registers_issue_tokens() {
        assert!(
            consumer_admits(tok),
            "consumer (Bus validator) wrongly REJECTED registered issue token `{tok}`: {:?}",
            validate_event_type(tok)
        );
    }
    assert!(
        register_issue_tokens().is_ok(),
        "Issues' register_issue_tokens() must be green: {:?}",
        register_issue_tokens()
    );
}

#[test]
fn cdc_2_9_consumer_rejects_a_malformed_issue_type_loudly() {
    assert!(matches!(
        validate_event_type("issue.issue.transition"),
        Err(TaxonomyError::PresentTenseVerb { .. })
    ));
    assert!(matches!(
        validate_event_type("issue.comments.created"),
        Err(TaxonomyError::PluralToken { .. })
    ));
    assert!(matches!(
        validate_event_type("issue.Issue.created"),
        Err(TaxonomyError::BadToken { .. })
    ));
}

#[test]
fn cdc_2_9_issues_registers_only_its_own_subsystem() {
    for &tok in provider_registers_issue_tokens() {
        assert!(
            tok.starts_with("issue."),
            "issue registered the foreign-subsystem token `{tok}` (must own `issue.*` only)"
        );
    }
    for tok in [
        ISSUE_UPDATED,
        ISSUE_TRANSITIONED,
        RELATION_CREATED,
        SLA_AT_RISK,
        INITIATIVE_HEALTH_CHANGED,
    ] {
        assert!(
            provider_registers_issue_tokens().contains(&tok),
            "`{tok}` must be registered (the names anchor X-5)"
        );
    }
}

#[test]
fn cdc_2_1_issue_payload_units_validate_and_seconds_vs_millis_is_rejected() {
    let frozen = serde_json::json!({
        "issue": "myelin://acme/issue/issue/ENG-1421",
        "target_seconds": 86_400,
        "stale_after_seconds": 2_592_000,
        "started_at": "2026-06-21T10:00:00Z"
    });
    assert_eq!(
        validate_issue_payload_units(&frozen),
        Ok(()),
        "an issue payload in the frozen units (seconds + RFC-3339 UTC) must validate"
    );

    let drifted = serde_json::json!({
        "issue": "myelin://acme/issue/issue/ENG-1421",
        "target_millis": 86_400_000,
        "started_at": "2026-06-21T10:00:00Z"
    });
    assert_eq!(
        validate_issue_payload_units(&drifted),
        Err(UnitError::DurationNotSeconds {
            field: "target_millis".into()
        }),
        "a millis-expressed duration must be REJECTED (the frozen unit is seconds)"
    );
}
