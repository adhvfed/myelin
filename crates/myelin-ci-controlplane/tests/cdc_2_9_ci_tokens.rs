use myelin_ci_controlplane::events::{
    ci_event_tokens, register_ci_taxonomy, validate_ci_type_token, CiTypeTokenError,
    CI_DURABLE_TOKENS, CI_FIREHOSE_TOKENS, CI_SUBSYSTEM_TOKEN,
};
use myelin_events::{validate_event_type, TaxonomyError, SUBSYSTEM_TOKENS};

fn provider_registers_ci_tokens() -> Vec<&'static str> {
    ci_event_tokens().collect()
}

fn consumer_admits(type_name: &str) -> bool {
    validate_event_type(type_name).is_ok()
        && type_name
            .split('.')
            .next()
            .is_some_and(|head| SUBSYSTEM_TOKENS.contains(&head))
}

#[test]
fn cdc_2_9_ci_provider_registers_consumer_admits_every_token() {
    for tok in provider_registers_ci_tokens() {
        assert!(
            consumer_admits(tok),
            "consumer (Bus validator + §6.2 table) wrongly REJECTED registered ci token `{tok}`: {:?}",
            validate_event_type(tok)
        );
    }
    assert_eq!(
        register_ci_taxonomy(),
        Ok(()),
        "the CI Control Plane register_ci_taxonomy() must be green: {:?}",
        register_ci_taxonomy()
    );
}

#[test]
fn cdc_2_9_consumer_rejects_a_malformed_ci_type_loudly() {
    assert!(matches!(
        validate_event_type("ci.run.start"),
        Err(TaxonomyError::PresentTenseVerb { .. })
    ));
    assert!(matches!(
        validate_event_type("ci.Run.started"),
        Err(TaxonomyError::BadToken { .. })
    ));
    assert!(matches!(
        validate_event_type("ci.run-step.started"),
        Err(TaxonomyError::BadToken { .. })
    ));
}

#[test]
fn cdc_2_9_consumer_rejects_nonconforming_6_2_tokens_loudly() {
    assert!(matches!(
        validate_ci_type_token("git.pr.opened"),
        Err(CiTypeTokenError::NotCiSubsystem { .. })
    ));
    assert!(matches!(
        validate_ci_type_token("ci.widget.created"),
        Err(CiTypeTokenError::UnregisteredTypeToken { .. })
    ));
}

#[test]
fn cdc_2_9_ci_registers_only_its_own_subsystem() {
    assert!(
        SUBSYSTEM_TOKENS.contains(&CI_SUBSYSTEM_TOKEN),
        "`ci` must be a canonical Bus subsystem token (§6.2)"
    );
    for tok in provider_registers_ci_tokens() {
        assert!(
            tok.starts_with("ci."),
            "CI registered the foreign-subsystem token `{tok}` (must own `ci.*` only)"
        );
    }
    assert!(provider_registers_ci_tokens().contains(&"ci.check.updated"));
    assert!(provider_registers_ci_tokens().contains(&"ci.result"));
}

#[test]
fn cdc_2_9_ci_durable_firehose_split_partitions_the_registry() {
    for f in CI_FIREHOSE_TOKENS {
        assert!(
            !CI_DURABLE_TOKENS.contains(f),
            "firehose token `{f}` must NOT be in the durable set"
        );
    }
    assert_eq!(
        CI_DURABLE_TOKENS.len() + CI_FIREHOSE_TOKENS.len(),
        provider_registers_ci_tokens().len(),
        "the durable + firehose sizes must partition the registry exactly"
    );
    assert!(CI_FIREHOSE_TOKENS.contains(&"ci.log.appended"));
    assert!(CI_DURABLE_TOKENS.contains(&"ci.log.available"));
}
