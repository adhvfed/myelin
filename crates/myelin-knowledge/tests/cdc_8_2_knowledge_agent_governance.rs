use myelin_agent_service::hitl_batch as fabric;
use myelin_knowledge::agent as kn;
use myelin_knowledge::transport::OpKind;
use std::collections::BTreeSet;

#[test]
fn cdc_8_2_kn_idem_key_rule_agrees_with_the_fabric_rule() {
    assert_eq!(
        kn::per_effect_idem_key("card-1", 0, 1),
        fabric::per_effect_idem_key("card-1", 0, 1)
    );
    for idx in 0..3 {
        assert_eq!(
            kn::per_effect_idem_key("card-2", idx, 3),
            fabric::per_effect_idem_key("card-2", idx, 3),
            "the KN idem_key for effect {idx} agrees with the Fabric rule"
        );
    }
}

#[test]
fn cdc_8_2_kn_consequential_effect_is_withheld_then_applies() {
    let gate = kn::KnowledgeEffectGate::new();
    let empty = BTreeSet::new();

    let refusal = gate.decide(kn::PUBLISH_TOOL, &empty, "card-1").unwrap_err();
    assert!(matches!(refusal, kn::EffectRefusal::Withheld { .. }));

    let approved: BTreeSet<String> = [kn::PUBLISH_TOOL.to_string()].into_iter().collect();
    assert!(gate.decide(kn::PUBLISH_TOOL, &approved, "card-1").is_ok());

    assert!(gate.decide(kn::DRAFT_TOOL, &empty, "card-1").is_ok());
}

#[test]
fn cdc_8_2_kn_reserve_settle_bookends_the_run() {
    let mut b = kn::ReserveSettle::reserve(10);
    assert!(b.has_remaining(4));
    assert_eq!(b.settle(4), 4);
    assert_eq!(b.remaining(), 6);
    let zero = kn::ReserveSettle::reserve(0);
    assert!(!zero.has_remaining(1));
}

#[test]
fn cdc_8_2_kn_d11_chained_drill_is_green() {
    let attr = kn::AgentEditAttribution::new("agent-7", "run:R1", "summarise → publish");
    let mut run = kn::KnowledgeAgentRun::begin(attr, 100);

    let op = run
        .propose(
            kn::APPEND_TOOL,
            OpKind::BlockIns,
            b"draft".to_vec(),
            4,
            "card-a",
            0,
            1,
        )
        .expect("reversible applies")
        .expect("produced an op");
    assert_eq!(op.actor, "agent:agent-7", "suggested by agent");

    assert!(matches!(
        run.propose(
            kn::PUBLISH_TOOL,
            OpKind::SetProp,
            b"p".to_vec(),
            4,
            "card-b",
            0,
            1
        ),
        Err(kn::EffectRefusal::Withheld { .. })
    ));
    assert_eq!(
        run.applied_ops().len(),
        1,
        "the withheld publish did not mutate"
    );

    run.approve(kn::PUBLISH_TOOL);
    assert!(run
        .propose(
            kn::PUBLISH_TOOL,
            OpKind::SetProp,
            b"p".to_vec(),
            4,
            "card-b",
            0,
            1
        )
        .unwrap()
        .is_some());
    assert!(
        run.propose(
            kn::PUBLISH_TOOL,
            OpKind::SetProp,
            b"p".to_vec(),
            4,
            "card-b",
            0,
            1
        )
        .unwrap()
        .is_none(),
        "the double-click is one approval"
    );
    assert_eq!(run.applied_ops().len(), 2, "0 double-apply");

    let receipt = run.seal(1_718_000_000_000);
    assert!(receipt.is_green(), "{receipt:?}");
    assert_eq!(receipt.ungoverned_mutations, 0);
    assert_eq!(receipt.mutations_before_approval, 0);
    assert_eq!(receipt.double_applies, 0);
    assert_eq!(
        receipt.settled_minor_units, 8,
        "metered the two applied effects"
    );
}
