use myelin_events::{
    HarnessError, PayloadShape, RegisteredToken, SubsystemTokenList, TaxonomyError,
    TokenListHarness,
};

#[test]
fn provider_subsystem_registers_its_completed_list_admitted_in_full() {
    let mut harness = TokenListHarness::new();
    let git = SubsystemTokenList::references_only(
        "git",
        &[
            "git.ref.updated",
            "git.pr.opened",
            "git.pr.merged",
            "git.repo.snapshot",
            "git.repo.erased",
        ],
    );
    assert_eq!(
        harness.register(&git).unwrap(),
        5,
        "the whole git list is admitted"
    );

    let kn = SubsystemTokenList::references_only(
        "knowledge",
        &[
            "knowledge.page.created",
            "knowledge.block.updated",
            "knowledge.page.snapshot",
        ],
    );
    assert_eq!(harness.register(&kn).unwrap(), 3);
    assert_eq!(harness.registered_subsystems(), vec!["git", "knowledge"]);
    assert!(harness.is_registered("git.ref.updated"));
    assert!(harness.is_registered("knowledge.block.updated"));
}

#[test]
fn consumer_harness_rejects_a_malformed_addition_loudly() {
    let mut harness = TokenListHarness::new();
    harness
        .register(&SubsystemTokenList::references_only(
            "git",
            &["git.ref.updated"],
        ))
        .unwrap();

    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("git.pr.open")),
        Err(HarnessError::UngrammaticalToken {
            cause: TaxonomyError::PresentTenseVerb { .. },
            ..
        })
    ));
    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("git.PR.opened")),
        Err(HarnessError::UngrammaticalToken {
            cause: TaxonomyError::BadToken { .. },
            ..
        })
    ));
    assert!(matches!(
        harness.add("git", RegisteredToken::references_only("ci.check.updated")),
        Err(HarnessError::ForeignPrefix { .. })
    ));
    assert_eq!(
        harness.len(),
        1,
        "a rejected addition never mutates the harness"
    );
}

#[test]
fn harness_holds_schema_ver_lineage_and_payload_shapes() {
    let mut harness = TokenListHarness::new();
    let kn = SubsystemTokenList::new(
        "knowledge",
        vec![
            RegisteredToken::references_only("knowledge.page.created"),
            RegisteredToken::references_only("knowledge.page.updated").at_schema_ver(2),
            RegisteredToken::inline_personal_data("knowledge.page.snapshot"),
            RegisteredToken::firehose("knowledge.block.op"),
        ],
    );
    harness.register(&kn).unwrap();

    assert_eq!(
        harness
            .lookup("knowledge.page.updated")
            .unwrap()
            .1
            .current_schema_ver,
        2
    );
    assert_eq!(
        harness.lookup("knowledge.page.created").unwrap().1.shape,
        PayloadShape::ReferencesOnly
    );
    assert_eq!(
        harness.lookup("knowledge.page.snapshot").unwrap().1.shape,
        PayloadShape::InlinePersonalData
    );
    assert_eq!(
        harness.lookup("knowledge.block.op").unwrap().1.shape,
        PayloadShape::EphemeralFirehose
    );
}

#[test]
fn harness_rejects_an_unknown_subsystem() {
    let mut harness = TokenListHarness::new();
    assert!(matches!(
        harness.register(&SubsystemTokenList::references_only(
            "billing",
            &["billing.invoice.created"]
        )),
        Err(HarnessError::UnknownSubsystem { .. })
    ));
}
