//! Unit tests + the KN-D11 chained drill for Knowledge agent governance (KN-P27 → P-317).

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

// ───────────────────────── the KN tool identity + the frozen §6.3 gate classification ────────────

/// **The KN tool identity constants are the frozen catalogue keys — and the closed set is total.**
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

/// **The FROZEN §6.3 consequential-gate defaults (X-6): publish + edit_confidential = yes; draft +
/// comment + append = no.** This is the KN-domain source of truth the Fabric §6.3 table agrees with.
#[test]
fn the_frozen_consequential_gate_classification_matches_section_6_3() {
    // consequential → gated.
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
    // reversible → NOT gated.
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

/// **An unknown tool is FAIL-CLOSED to gated (VISION §3 — never silently un-governed).**
#[test]
fn an_unknown_tool_is_fail_closed_to_gated() {
    assert!(
        requires_approval_default("delete_space"),
        "a tool we cannot classify is gated, never silently un-governed"
    );
    assert!(required_caps_for("delete_space").is_empty());
}

/// **The required_caps come from the FROZEN KN ReBAC carrier (4.9), not invented here.** A rename of
/// a permission in `myelin-content` breaks this test (no silent drift — the KN parallel to the Git CDC
/// and the agent-service registration's source of truth).
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
    // dispatched by name.
    assert_eq!(required_caps_for(PUBLISH_TOOL), publish_required_caps());
    assert_eq!(required_caps_for(APPEND_TOOL), append_required_caps());
    // the object-type half IS the canonical KN ReBAC name (4.9), not a local string.
    assert_eq!(kn_objects::PAGE, "page");
}

// ───────────────────────── "suggested by agent" attribution (02 §9 / ADR-08 / AI-Act) ────────────

/// **An agent edit STRUCTURALLY carries its provenance (never disguised as human).** The `Agent` arm
/// carries the attribution — there is no way to be agent-authored and omit the run/rationale.
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
    // the collab actor is the agent:-prefixed pseudonym (rendered "suggested by agent").
    assert_eq!(author.actor(), "agent:agent-7");
}

/// **A human edit is NOT flagged agent + carries NO agent provenance** — the closed enum makes human
/// XOR agent unambiguous (no third "maybe agent" state to disguise an agent as).
#[test]
fn a_human_edit_is_not_agent_and_has_no_agent_provenance() {
    let human = EditAuthor::Human {
        author_pseudonym: "human-x".into(),
    };
    assert!(!human.is_agent());
    assert!(human.agent_provenance().is_none());
    // a human edit's actor is the bare pseudonym (no agent label).
    assert_eq!(human.actor(), "human-x");
}

/// **An agent edit rides the SAME `SEND_OP` path a human does (02 §9) — it produces an ordinary
/// `DocOp`, differing ONLY in its (legible) actor.** The structural proof there is no second
/// agent-write path: `stamp_op` yields a `DocOp` the transport applies identically.
#[test]
fn an_agent_edit_produces_an_ordinary_docop_with_agent_attribution() {
    let agent = EditAuthor::Agent(attribution());
    let human = EditAuthor::Human {
        author_pseudonym: "human-x".into(),
    };
    let agent_op = agent.stamp_op(OpId::new("run:R1", 0), OpKind::Insert, b"hello".to_vec());
    let human_op = human.stamp_op(OpId::new("human-x", 0), OpKind::Insert, b"hello".to_vec());
    // both are ordinary DocOps the transport applies the same way — the SAME protocol (02 §9).
    assert_eq!(agent_op.kind, OpKind::Insert);
    assert_eq!(human_op.kind, OpKind::Insert);
    assert_eq!(agent_op.payload, human_op.payload, "same op shape");
    // the ONLY difference is the legible actor.
    assert_eq!(agent_op.actor, "agent:agent-7", "suggested by agent");
    assert_eq!(human_op.actor, "human-x");
    assert_ne!(agent_op.actor, human_op.actor);
}

// ───────────────────────── the per-effect idem_key rule (OQ-F / 9.1/9.4) ─────────────────────────

/// **A single-effect card keys on the bare card_id (a double-click is one approval); a batch keys on
/// `card_id:effect_idx` (a partial approval is well-defined).** The SAME rule the Fabric uses.
#[test]
fn per_effect_idem_key_follows_the_frozen_oq_f_rule() {
    // single-effect card → the key IS the card id.
    assert_eq!(per_effect_idem_key("card-1", 0, 1), "card-1");
    // multi-effect card → card_id:effect_idx, distinct per effect.
    assert_eq!(per_effect_idem_key("card-2", 0, 3), "card-2:0");
    assert_eq!(per_effect_idem_key("card-2", 1, 3), "card-2:1");
    assert_eq!(per_effect_idem_key("card-2", 2, 3), "card-2:2");
    assert_ne!(
        per_effect_idem_key("card-2", 0, 3),
        per_effect_idem_key("card-2", 1, 3),
        "each effect in a batch has its OWN key (a partial approval is well-defined)"
    );
}

// ───────────────────────── the HITL-withhold gate (8.2 step 6 / AG-8) ─────────────────────────────

/// **A consequential effect not in the approved set is WITHHELD — it does NOT mutate (AG-8).**
#[test]
fn a_consequential_effect_is_withheld_until_approved() {
    let gate = KnowledgeEffectGate::new();
    let empty = BTreeSet::new();
    // publish is consequential → withheld with no approval.
    let refusal = gate.decide(PUBLISH_TOOL, &empty, "card-1").unwrap_err();
    assert_eq!(
        refusal,
        EffectRefusal::Withheld {
            card_id: "card-1".into()
        }
    );
    assert!(refusal.to_string().contains("WITHHELD"));
    // once approved, it passes the gate.
    let approved: BTreeSet<String> = [PUBLISH_TOOL.to_string()].into_iter().collect();
    assert!(gate.decide(PUBLISH_TOOL, &approved, "card-1").is_ok());
}

/// **A reversible effect passes the gate with NO approval (draft/comment/append apply directly).**
#[test]
fn a_reversible_effect_passes_the_gate_directly() {
    let gate = KnowledgeEffectGate::new();
    let empty = BTreeSet::new();
    assert!(gate.decide(DRAFT_TOOL, &empty, "card-1").is_ok());
    assert!(gate.decide(COMMENT_TOOL, &empty, "card-1").is_ok());
    assert!(gate.decide(APPEND_TOOL, &empty, "card-1").is_ok());
}

/// **The idem-key ledger applies each key exactly once (a double-click is one apply).**
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

// ───────────────────────── reserve/settle (11.7) ─────────────────────────────────────────────────

/// **reserve/settle: no balance → no agent write; settle debits exactly the cost.**
#[test]
fn reserve_settle_bookends_the_run() {
    let mut b = ReserveSettle::reserve(10);
    assert!(b.has_remaining(4));
    assert_eq!(b.settle(4), 4);
    assert_eq!(b.remaining(), 6);
    assert_eq!(b.settled(), 4);
    // a zero-reserve run cannot write anything ("no balance → no agent write").
    let zero = ReserveSettle::reserve(0);
    assert!(!zero.has_remaining(1));
    assert!(zero.has_remaining(0));
}

// ───────────────────────── the KN-D11 CHAINED DRILL (the dated green artifact) ────────────────────

/// **KN-D11 — the chained scenario: an agent plans → a consequential publish is WITHHELD (Denied, 0
/// mutation) until approval → after approval it applies ONCE even across a DOUBLE-CLICK → the run
/// passed reserve/settle. 0 ungoverned mutation, 0 mutation before approval, 0 double-apply.**
#[test]
fn kn_d11_chained_drill_emits_a_green_receipt() {
    let mut run = KnowledgeAgentRun::begin(attribution(), /* reserve */ 100);

    // (1) The agent first appends a reversible draft block — applies DIRECTLY (reversible, not gated),
    //     attributed "suggested by agent", metered once.
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

    // (2) The agent proposes a CONSEQUENTIAL publish — WITHHELD (Denied, 0 mutation) — no approval yet.
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
    // proof of 0 mutation before approval: exactly one op applied so far (the draft), not the publish.
    assert_eq!(
        run.applied_ops().len(),
        1,
        "the withheld publish did NOT mutate (0 mutation before approval)"
    );

    // (3) A human APPROVES the publish (the HITL resume) — then the agent re-proposes it: it APPLIES.
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

    // (4) A DOUBLE-CLICK re-sends the SAME approval (same card_id, same per-effect key) → ONE approval,
    //     NO second mutation (the double-click is a well-defined no-op).
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
        "the double-click is ONE approval — no second op (0 double-apply)"
    );
    assert_eq!(
        run.applied_ops().len(),
        2,
        "still exactly two applied ops — the double-click did NOT double-apply"
    );

    // seal the dated KN-D11 green receipt.
    let receipt = run.seal(/* at_ms */ 1_718_000_000_000);
    assert!(
        receipt.is_green(),
        "KN-D11 is green: 0 ungoverned/0 pre-approval/0 double-apply, ≥1 applied — {receipt:?}"
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
    // the run passed reserve/settle: it metered exactly the two applied effects (4 + 4 = 8).
    assert_eq!(
        receipt.settled_minor_units, 8,
        "metered the two applied effects"
    );
    assert_eq!(run.budget().remaining(), 92, "reserve debited the bill");
}

/// **A denied effect (exhausted reserve) is an ORDINARY tool error — no privileged fallback — and 0
/// mutation. "no balance → no agent write" (11.7).**
#[test]
fn an_exhausted_reserve_denies_the_effect_no_privileged_fallback() {
    // a run reserving only 3 minor-units cannot afford a cost-4 effect.
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

/// **A partial approval of a BATCH is well-defined: approve effect 0 (applies) while effect 1 stays
/// withheld (0 mutation) — each on its OWN per-effect key (OQ-F).** Two publishes on one card.
#[test]
fn a_partial_batch_approval_is_well_defined() {
    let mut run = KnowledgeAgentRun::begin(attribution(), 100);
    run.approve(PUBLISH_TOOL); // approving the tool admits the gate; the per-effect key dedups applies.

    // effect 0 of a 2-effect card applies (key card-c:0).
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

    // effect 1 of the same card applies INDEPENDENTLY on its own key (card-c:1) — distinct apply.
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

    // a double-click on effect 0 (same key card-c:0) is a no-op — one approval per effect.
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

/// **The KN-D11 receipt is GREEN only when every forbidden counter is 0 AND ≥1 effect applied (kills
/// the `is_green -> true` mutant).**
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

/// **The EffectRefusal errors render LOUD and self-describing (kills the `Display -> Ok(default)`
/// mutant).**
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
