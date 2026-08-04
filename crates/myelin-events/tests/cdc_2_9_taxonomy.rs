use myelin_events::taxonomy::{self, new_tokens};
use myelin_events::{
    validate_event_type, ArtifactRef, EventDraft, EventType, SEED_EVENT_NAMES, SUBSYSTEM_TOKENS,
};
use myelin_events::{AggregateKey, DataRole, Visibility};

fn provider_emits_typed_draft(type_name: &str) -> EventDraft {
    EventDraft {
        type_: EventType(type_name.to_string()),
        subject: ArtifactRef("myelin://acme/ci/run/01J".into()),
        aggregate: AggregateKey("ci:01J".into()),
        payload: serde_json::json!({ "ref": "myelin://acme/ci/run/01J" }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

fn consumer_admits(draft: &EventDraft) -> bool {
    validate_event_type(&draft.type_.0).is_ok()
}

#[test]
fn cdc_2_9_provider_emits_new_tokens_consumer_admits_them() {
    for token in [
        new_tokens::CI_CHECK_UPDATED,
        new_tokens::CI_RESULT,
        new_tokens::ISSUE_INITIATIVE_CREATED,
    ] {
        let draft = provider_emits_typed_draft(token);
        assert!(
            consumer_admits(&draft),
            "consumer (validator) wrongly rejected the new token `{token}`"
        );
    }
}

#[test]
fn cdc_2_9_provider_seed_names_all_admitted_by_consumer() {
    for name in SEED_EVENT_NAMES {
        let draft = provider_emits_typed_draft(name);
        assert!(
            consumer_admits(&draft),
            "consumer (validator) wrongly rejected seed name `{name}`"
        );
    }
}

#[test]
fn cdc_2_9_consumer_rejects_a_malformed_type_loudly() {
    let bad = provider_emits_typed_draft("CI.Run.Started");
    assert!(
        !consumer_admits(&bad),
        "validator must reject `CI.Run.Started`"
    );

    let present = provider_emits_typed_draft("ci.run.start");
    assert!(matches!(
        validate_event_type(&present.type_.0),
        Err(taxonomy::TaxonomyError::PresentTenseVerb { .. })
    ));

    let unknown = provider_emits_typed_draft("billing.invoice.created");
    assert!(matches!(
        validate_event_type(&unknown.type_.0),
        Err(taxonomy::TaxonomyError::UnknownSubsystem { .. })
    ));
}

#[test]
fn cdc_2_9_subsystem_token_set_is_the_shared_anchor() {
    assert_eq!(
        SUBSYSTEM_TOKENS,
        &[
            "git",
            "ci",
            "issue",
            "knowledge",
            "chat",
            "identity",
            "refs"
        ]
    );
}
