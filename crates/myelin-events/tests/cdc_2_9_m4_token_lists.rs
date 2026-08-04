use myelin_events::{
    HarnessError, RegisteredToken, SubsystemTokenList, TaxonomyError, TokenListHarness,
};

#[test]
fn m4_subsystems_register_their_lists_admitted_in_full() {
    let mut harness = TokenListHarness::new();

    let ci = SubsystemTokenList::references_only(
        "ci",
        &[
            "ci.check.updated",
            "ci.result",
            "ci.run.started",
            "ci.run.succeeded",
            "ci.run.failed",
            "ci.log.available",
            "ci.run.snapshot",
            "ci.run.erased",
        ],
    );
    assert_eq!(
        harness.register(&ci).unwrap(),
        8,
        "the whole CI list is admitted"
    );

    let issues = SubsystemTokenList::references_only(
        "issue",
        &[
            "issue.issue.created",
            "issue.issue.transitioned",
            "issue.issue.snapshot",
            "issue.issue.erased",
        ],
    );
    assert_eq!(harness.register(&issues).unwrap(), 4);

    let chat = SubsystemTokenList::references_only(
        "chat",
        &[
            "chat.message.created",
            "chat.message.edited",
            "chat.message.snapshot",
            "chat.message.erased",
        ],
    );
    assert_eq!(harness.register(&chat).unwrap(), 4);

    assert_eq!(
        harness.registered_subsystems(),
        vec!["chat", "ci", "issue"],
        "all three M4 subsystems coexist (deterministic order)"
    );
    assert!(harness.is_registered("ci.check.updated"));
    assert!(harness.is_registered("ci.result"));
    assert!(harness.is_registered("issue.issue.transitioned"));
    assert!(harness.is_registered("chat.message.snapshot"));
}

#[test]
fn m4_harness_rejects_malformed_additions_loudly() {
    let mut harness = TokenListHarness::new();
    harness
        .register(&SubsystemTokenList::references_only(
            "ci",
            &["ci.check.updated"],
        ))
        .unwrap();

    assert!(matches!(
        harness.add("ci", RegisteredToken::references_only("ci.run.start")),
        Err(HarnessError::UngrammaticalToken {
            cause: TaxonomyError::PresentTenseVerb { .. },
            ..
        })
    ));
    assert!(matches!(
        harness.add("ci", RegisteredToken::references_only("ci.Run.started")),
        Err(HarnessError::UngrammaticalToken {
            cause: TaxonomyError::BadToken { .. },
            ..
        })
    ));
    assert!(matches!(
        harness.add(
            "ci",
            RegisteredToken::references_only("chat.message.created")
        ),
        Err(HarnessError::ForeignPrefix { .. })
    ));
    assert_eq!(
        harness.len(),
        1,
        "a rejected addition never mutates the harness"
    );
}

#[test]
fn superseded_legacy_tokens_are_grammatical_but_unregistered() {
    assert!(myelin_events::validate_event_type("ci.status.updated").is_ok());
    assert!(myelin_events::validate_event_type("ci.run.passed").is_ok());
    let mut harness = TokenListHarness::new();
    harness
        .register(&SubsystemTokenList::references_only(
            "ci",
            &["ci.check.updated", "ci.result"],
        ))
        .unwrap();
    assert!(!harness.is_registered("ci.status.updated"));
    assert!(!harness.is_registered("ci.run.passed"));
}
