use myelin_events::{
    HarnessError, RegisteredToken, TaxonomyError, TokenListHarness,
};
use myelin_git::events::{
    git_event_token_list, GIT_EVENT_TOKENS, GIT_PR_HEAD_TRIGGER_SCHEMA_V2, GIT_PR_OPENED,
    GIT_PR_SYNCHRONIZED,
};

#[test]
fn git_complete_list_is_admitted_by_the_bus_harness_in_full() {
    let mut harness = TokenListHarness::new();
    let git = git_event_token_list();
    let admitted = harness
        .register(&git)
        .expect("git's complete list is admitted by the harness");
    assert_eq!(
        admitted,
        GIT_EVENT_TOKENS.len(),
        "EVERY git token is admitted (0 ungrammatical)"
    );
    assert_eq!(harness.names_for("git").len(), GIT_EVENT_TOKENS.len());
    assert!(harness.is_registered("git.ref.updated"));
    assert!(harness.is_registered("git.repo.snapshot"));
    assert!(harness.is_registered("git.repo.erased"));
    for name in [GIT_PR_OPENED, GIT_PR_SYNCHRONIZED] {
        let (_, token) = harness
            .lookup(name)
            .expect("PR head-trigger token is registered");
        assert_eq!(token.current_schema_ver, GIT_PR_HEAD_TRIGGER_SCHEMA_V2);
    }
}

#[test]
fn the_harness_rejects_a_malformed_addition_to_gits_list() {
    let mut harness = TokenListHarness::new();
    harness
        .register(&git_event_token_list())
        .unwrap();

    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("git.pr.open")),
        Err(HarnessError::UngrammaticalToken {
            cause: TaxonomyError::PresentTenseVerb { .. },
            ..
        })
    ));
    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("ci.check.updated")),
        Err(HarnessError::ForeignPrefix { .. })
    ));
    assert_eq!(harness.names_for("git").len(), GIT_EVENT_TOKENS.len());
}
