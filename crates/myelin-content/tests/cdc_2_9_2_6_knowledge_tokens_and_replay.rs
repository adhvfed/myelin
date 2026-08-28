use myelin_content::events::{
    register_knowledge_tokens, KNOWLEDGE_DURABLE_TOKENS, KNOWLEDGE_FIREHOSE_TOKENS,
};
use myelin_events::{
    HarnessError, SubsystemTokenList, TokenListHarness,
};

#[test]
fn cdc_2_9_knowledge_complete_list_admitted_by_the_bus_harness() {
    assert!(
        register_knowledge_tokens().is_ok(),
        "KN's list parses the §6.1 grammar"
    );

    let mut harness = TokenListHarness::new();
    let all: Vec<&str> = KNOWLEDGE_DURABLE_TOKENS
        .iter()
        .chain(KNOWLEDGE_FIREHOSE_TOKENS)
        .copied()
        .collect();
    let admitted = harness
        .register(&SubsystemTokenList::references_only("knowledge", &all))
        .expect("KN's complete list is admitted");
    assert_eq!(admitted, all.len());
    assert!(harness.is_registered("knowledge.block.updated"));
    assert!(harness.is_registered("knowledge.page.snapshot"));
}

#[test]
fn cdc_2_9_harness_rejects_a_malformed_knowledge_addition() {
    let mut harness = TokenListHarness::new();
    harness
        .register(&SubsystemTokenList::references_only(
            "knowledge",
            KNOWLEDGE_DURABLE_TOKENS,
        ))
        .unwrap();
    assert!(matches!(
        harness.add(
            "knowledge",
            myelin_events::RegisteredToken::references_only("knowledge.pages.created")
        ),
        Err(HarnessError::UngrammaticalToken { .. })
    ));
}
