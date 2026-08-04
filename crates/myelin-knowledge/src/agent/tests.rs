use super::*;
use crate::transport::OpKind;
use std::collections::BTreeSet;

fn attribution() -> AgentEditAttribution {
    AgentEditAttribution::new(
        "agent-7",
        "run:R1",
        "summarised the action items into a draft",
    )
}

#[test]
fn the_kn_tool_identity_constants_are_the_frozen_keys() {
    assert_eq!(KNOWLEDGE_SUBSYSTEM, "knowledge");
    assert_eq!(PUBLISH_TOOL, "publish");
    assert_eq!(EDIT_CONFIDENTIAL_TOOL, "edit_confidential");
    assert_eq!(DRAFT_TOOL, "draft");
    assert_eq!(COMMENT_TOOL, "comment");
    assert_eq!(APPEND_TOOL, "append");
    assert_eq!(ALL_TOOLS.len(), 5);
    for t in [
        PUBLISH_TOOL,
        EDIT_CONFIDENTIAL_TOOL,
        DRAFT_TOOL,
        COMMENT_TOOL,
        APPEND_TOOL,
    ] {
        assert!(ALL_TOOLS.contains(&t), "{t} is in the closed KN tool set");
    }
}

#[test]
fn the_frozen_consequential_gate_classification_matches_section_6_3() {
    assert!(
        requires_approval_default(PUBLISH_TOOL),
        "publish is consequential (an approver set) → gated"
    );
    assert!(
        requires_approval_default(EDIT_CONFIDENTIAL_TOOL),
        "a confidential edit is consequential → gated"
    );
    assert!(is_consequential(PUBLISH_TOOL));
    assert!(is_consequential(EDIT_CONFIDENTIAL_TOOL));
    assert!(
        !requires_approval_default(DRAFT_TOOL),
        "draft is reversible"
    );
    assert!(
        !requires_approval_default(COMMENT_TOOL),
        "comment is reversible"
    );
    assert!(
        !requires_approval_default(APPEND_TOOL),
        "append is reversible"
    );
    assert!(!is_consequential(DRAFT_TOOL));
}

#[test]
fn an_unknown_tool_is_fail_closed_to_gated() {
    assert!(
        requires_approval_default("delete_space"),
        "a tool we cannot classify is gated, never silently un-governed"
    );
    assert!(required_caps_for("delete_space").is_empty());
}

#[test]
fn required_caps_are_the_frozen_kn_rebac_carrier_permissions() {
    assert_eq!(publish_required_caps(), vec!["page.publish".to_string()]);
    assert_eq!(
        edit_confidential_required_caps(),
        vec!["page.edit".to_string()]
    );
    assert_eq!(draft_required_caps(), vec!["page.draft".to_string()]);
    assert_eq!(comment_required_caps(), vec!["page.comment".to_string()]);
    assert_eq!(append_required_caps(), vec!["page.edit".to_string()]);
    assert_eq!(required_caps_for(PUBLISH_TOOL), publish_required_caps());
    assert_eq!(required_caps_for(APPEND_TOOL), append_required_caps());
    assert_eq!(kn_objects::PAGE, "page");
}

#[test]
fn an_agent_edit_is_legible_with_provenance_never_disguised() {
    let author = EditAuthor::Agent(attribution());
    assert!(author.is_agent(), "an agent edit is legibly flagged");
    let prov = author
        .agent_provenance()
        .expect("agent provenance is REQUIRED (AI-Act)");
    assert_eq!(prov.agent_pseudonym, "agent-7", "which agent");
    assert_eq!(prov.run_id, "run:R1", "which run (traceable provenance)");
    assert!(prov.rationale.contains("action items"), "the why");
    assert_eq!(author.actor(), "agent:agent-7");
}

#[test]
fn a_human_edit_is_not_agent_and_has_no_agent_provenance() {
    let human = EditAuthor::Human {
        author_pseudonym: "human-x".into(),
    };
    assert!(!human.is_agent());
    assert!(human.agent_provenance().is_none());
    assert_eq!(human.actor(), "human-x");
}

#[test]
fn an_agent_edit_produces_an_ordinary_docop_with_agent_attribution() {
    let agent = EditAuthor::Agent(attribution());
    let human = EditAuthor::Human {
        author_pseudonym: "human-x".into(),
    };
    let agent_op = agent.stamp_op(OpId::new("run:R1", 0), OpKind::Insert, b"hello".to_vec());
    let human_op = human.stamp_op(OpId::new("human-x", 0), OpKind::Insert, b"hello".to_vec());
    assert_eq!(agent_op.kind, OpKind::Insert);
    assert_eq!(human_op.kind, OpKind::Insert);
    assert_eq!(agent_op.payload, human_op.payload, "same op shape");
    assert_eq!(agent_op.actor, "agent:agent-7", "suggested by agent");
    assert_eq!(human_op.actor, "human-x");
    assert_ne!(agent_op.actor, human_op.actor);
}

#[test]
fn per_effect_idem_key_follows_the_frozen_oq_f_rule() {
    assert_eq!(per_effect_idem_key("card-1", 0, 1), "card-1");
    assert_eq!(per_effect_idem_key("card-2", 0, 3), "card-2:0");
    assert_eq!(per_effect_idem_key("card-2", 1, 3), "card-2:1");
    assert_eq!(per_effect_idem_key("card-2", 2, 3), "card-2:2");
    assert_ne!(
        per_effect_idem_key("card-2", 0, 3),
        per_effect_idem_key("card-2", 1, 3),
        "each effect in a batch has its OWN key (a partial approval is well-defined)"
    );
}

#[test]
fn a_consequential_effect_is_withheld_until_approved() {
    let gate = KnowledgeEffectGate::new();
    let empty = BTreeSet::new();
    let refusal = gate.decide(PUBLISH_TOOL, &empty, "card-1").unwrap_err();
    assert_eq!(
        refusal,
        EffectRefusal::Withheld {
            card_id: "card-1".into()
        }
    );
    assert!(refusal.to_string().contains("WITHHELD"));
    let approved: BTreeSet<String> = [PUBLISH_TOOL.to_string()].into_iter().collect();
    assert!(gate.decide(PUBLISH_TOOL, &approved, "card-1").is_ok());
}

#[test]
fn a_reversible_effect_passes_the_gate_directly() {
    let gate = KnowledgeEffectGate::new();
    let empty = BTreeSet::new();
    assert!(gate.decide(DRAFT_TOOL, &empty, "card-1").is_ok());
    assert!(gate.decide(COMMENT_TOOL, &empty, "card-1").is_ok());
    assert!(gate.decide(APPEND_TOOL, &empty, "card-1").is_ok());
}

#[test]
fn the_idem_key_ledger_applies_each_key_exactly_once() {
    let mut gate = KnowledgeEffectGate::new();
    assert!(gate.apply_once("k1"), "first apply is fresh");
    assert!(
        !gate.apply_once("k1"),
        "second apply of the same key is a no-op"
    );
    assert!(gate.apply_once("k2"), "a distinct key applies");
    assert_eq!(gate.applied_count(), 2, "two distinct effects applied");
    assert!(gate.has_applied("k1"));
    assert!(!gate.has_applied("k3"));
}

#[test]
fn reserve_settle_bookends_the_run() {
    let mut b = ReserveSettle::reserve(10);
    assert!(b.has_remaining(4));
    assert_eq!(b.settle(4), 4);
    assert_eq!(b.remaining(), 6);
    assert_eq!(b.settled(), 4);
    let zero = ReserveSettle::reserve(0);
    assert!(!zero.has_remaining(1));
    assert!(zero.has_remaining(0));
}

#[test]
fn kn_d11_chained_drill_emits_a_green_receipt() {
    let mut run = KnowledgeAgentRun::begin(attribution(),  100);

    let draft_op = run
        .propose(
            APPEND_TOOL,
            OpKind::BlockIns,
            b"draft".to_vec(),
            4,
            "card-a",
            0,
            1,
        )
        .expect("a reversible append applies")
        .expect("the reversible append produced an attributed op");
    assert_eq!(draft_op.actor, "agent:agent-7", "suggested by agent");

    let withheld = run
        .propose(
            PUBLISH_TOOL,
            OpKind::SetProp,
            b"publish".to_vec(),
            4,
            "card-b",
            0,
            1,
        )
        .unwrap_err();
    assert!(
        matches!(withheld, EffectRefusal::Withheld { .. }),
        "the consequential publish is WITHHELD until approval (AG-8): {withheld}"
    );
    assert_eq!(
        run.applied_ops().len(),
        1,
        "the withheld publish did NOT mutate (0 mutation before approval)"
    );

    run.approve(PUBLISH_TOOL);
    let publish_op = run
        .propose(
            PUBLISH_TOOL,
            OpKind::SetProp,
            b"publish".to_vec(),
            4,
            "card-b",
            0,
            1,
        )
        .expect("the approved publish applies")
        .expect("the approved publish produced an attributed op");
    assert_eq!(publish_op.actor, "agent:agent-7");
    assert_eq!(run.applied_ops().len(), 2, "the publish now applied");

    let double_click = run
        .propose(
            PUBLISH_TOOL,
            OpKind::SetProp,
            b"publish".to_vec(),
            4,
            "card-b",
            0,
            1,
        )
        .expect("the double-click is governed");
    assert!(
        double_click.is_none(),
        "the double-click is ONE approval - no second op (0 double-apply)"
    );
    assert_eq!(
        run.applied_ops().len(),
        2,
        "still exactly two applied ops - the double-click did NOT double-apply"
    );

    let receipt = run.seal( 1_718_000_000_000);
    assert!(
        receipt.is_green(),
        "KN-D11 is green: 0 ungoverned/0 pre-approval/0 double-apply, ≥1 applied - {receipt:?}"
    );
    assert_eq!(
        receipt.applied, 2,
        "draft + publish applied (distinct keys)"
    );
    assert_eq!(receipt.withheld, 1, "the publish was withheld once");
    assert_eq!(
        receipt.ungoverned_mutations, 0,
        "AG-D1: 0 ungoverned mutation"
    );
    assert_eq!(
        receipt.mutations_before_approval, 0,
        "AG-8: 0 mutation before approval"
    );
    assert_eq!(receipt.double_applies, 0, "OQ-F: 0 double-apply");
    assert_eq!(
        receipt.settled_minor_units, 8,
        "metered the two applied effects"
    );
    assert_eq!(run.budget().remaining(), 92, "reserve debited the bill");
}

#[test]
fn an_exhausted_reserve_denies_the_effect_no_privileged_fallback() {
    let mut run = KnowledgeAgentRun::begin(attribution(), 3);
    let denied = run
        .propose(
            DRAFT_TOOL,
            OpKind::BlockIns,
            b"x".to_vec(),
            4,
            "card-a",
            0,
            1,
        )
        .unwrap_err();
    assert!(
        matches!(denied, EffectRefusal::Denied(_)),
        "an exhausted reserve is an ordinary Denied (no privileged fallback): {denied}"
    );
    assert_eq!(
        run.applied_ops().len(),
        0,
        "the denied effect did NOT mutate"
    );
    let receipt = run.seal(1);
    assert!(
        !receipt.is_green(),
        "a run that applied nothing is NOT a green KN-D11 (the scenario did not complete)"
    );
    assert_eq!(receipt.denied, 1);
    assert_eq!(receipt.applied, 0);
}

#[test]
fn a_partial_batch_approval_is_well_defined() {
    let mut run = KnowledgeAgentRun::begin(attribution(), 100);
    run.approve(PUBLISH_TOOL);

    let e0 = run
        .propose(
            PUBLISH_TOOL,
            OpKind::SetProp,
            b"p0".to_vec(),
            4,
            "card-c",
            0,
            2,
        )
        .expect("governed")
        .expect("effect 0 applied");
    assert_eq!(e0.actor, "agent:agent-7");

    let e1 = run
        .propose(
            PUBLISH_TOOL,
            OpKind::SetProp,
            b"p1".to_vec(),
            4,
            "card-c",
            1,
            2,
        )
        .expect("governed")
        .expect("effect 1 applied");
    assert_eq!(e1.payload, b"p1".to_vec());
    assert_eq!(
        run.applied_ops().len(),
        2,
        "two distinct batch effects applied"
    );

    let dup = run
        .propose(
            PUBLISH_TOOL,
            OpKind::SetProp,
            b"p0".to_vec(),
            4,
            "card-c",
            0,
            2,
        )
        .expect("governed");
    assert!(dup.is_none(), "the per-effect double-click is one apply");
    assert_eq!(
        run.applied_ops().len(),
        2,
        "0 double-apply across the batch"
    );

    let receipt = run.seal(1);
    assert!(receipt.is_green());
    assert_eq!(receipt.applied, 2, "the two approved batch effects (0 + 1)");
    assert_eq!(receipt.double_applies, 0);
}

#[test]
fn the_receipt_is_green_only_when_all_invariants_hold() {
    let green = KnD11Receipt {
        withheld: 1,
        denied: 0,
        applied: 2,
        mutations_before_approval: 0,
        double_applies: 0,
        ungoverned_mutations: 0,
        settled_minor_units: 8,
        at_ms: 1,
    };
    assert!(green.is_green());
    assert!(
        !KnD11Receipt {
            mutations_before_approval: 1,
            ..green.clone()
        }
        .is_green(),
        "a mutation before approval is RED"
    );
    assert!(
        !KnD11Receipt {
            double_applies: 1,
            ..green.clone()
        }
        .is_green(),
        "a double-apply is RED"
    );
    assert!(
        !KnD11Receipt {
            ungoverned_mutations: 1,
            ..green.clone()
        }
        .is_green(),
        "an ungoverned mutation is RED"
    );
    assert!(
        !KnD11Receipt {
            applied: 0,
            ..green.clone()
        }
        .is_green(),
        "a run that applied nothing is not a completed green scenario"
    );
}

#[test]
fn the_effect_refusals_render_loud() {
    assert!(EffectRefusal::Withheld {
        card_id: "card-1".into()
    }
    .to_string()
    .contains("WITHHELD"));
    assert!(EffectRefusal::Denied("reserve exhausted".into())
        .to_string()
        .contains("DENIED"));
}
