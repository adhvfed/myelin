//! # CDC 8.2 (Knowledge slice) — the KN agent governance AGREES with the Fabric plan-then-apply
//! contract: HITL withhold (no mutation) + per-effect `idem_key` + reserve/settle (KN-P27 → P-317)
//!
//! Contract 8.2 (`EffectApi::apply` → `Applied | Gated | Denied`): a side-effecting Knowledge tool
//! goes through the Fabric's plan-then-apply pipeline, which calls Knowledge's public endpoint AS the
//! agent principal, which applies the op through the collab protocol with "suggested by agent"
//! attribution. The Knowledge-domain HALF (the decision the public endpoint makes before it threads an
//! op through the collab protocol — the HITL withhold, the per-effect `idem_key` dedup, the
//! reserve/settle bookend) lives in `myelin_knowledge::agent`. This CDC pins it against the FROZEN
//! Fabric rules:
//!
//! - the per-effect `idem_key` derivation (OQ-F / 9.1/9.4) is the SAME rule
//!   `myelin_agent_service::hitl_batch::per_effect_idem_key` applies (the two sides agree by
//!   construction — a double-click is one approval, a partial approval is well-defined);
//! - a withheld consequential effect maps to the Fabric's `Gated`/withhold semantics (does NOT
//!   mutate, AG-8); a denied effect maps to `Denied` (an ordinary tool error, no privileged fallback);
//! - the KN-D11 chained drill (agent plans → withheld → approve → applied once across a double-click)
//!   is the CONSUMER of these governance properties.
//!
//! **CDC pair (8.2, Knowledge slice).** PROVIDER side: `myelin_agent_service::hitl_batch` — the
//! Fabric's frozen per-effect `idem_key` derivation (the durable-signal dedup the resume rides).
//! CONSUMER side: `myelin_knowledge::agent` — the KN public endpoint's governance (withhold / dedup /
//! reserve-settle) that CONSUMES that rule + the EffectApi `Applied | Gated | Denied` outcome
//! semantics. This test asserts the consumer's idem_key + withhold + reserve-settle agree with the
//! provider's frozen rule, and runs the KN-D11 chained drill as the end-to-end consumer.

use myelin_agent_service::hitl_batch as fabric;
use myelin_knowledge::agent as kn;
use myelin_knowledge::transport::OpKind;
use std::collections::BTreeSet;

/// **The per-effect `idem_key` rule AGREES with the Fabric's `per_effect_idem_key` (OQ-F / 9.1).**
/// Knowledge CONSUMES the rule, it does NOT author a second one — a double-click resume against the KN
/// public endpoint dedups EXACTLY as the durable-signal PK does.
#[test]
fn cdc_8_2_kn_idem_key_rule_agrees_with_the_fabric_rule() {
    // single-effect card → the bare card id (a double-click is one approval).
    assert_eq!(
        kn::per_effect_idem_key("card-1", 0, 1),
        fabric::per_effect_idem_key("card-1", 0, 1)
    );
    // multi-effect card → card_id:effect_idx (a partial approval is well-defined).
    for idx in 0..3 {
        assert_eq!(
            kn::per_effect_idem_key("card-2", idx, 3),
            fabric::per_effect_idem_key("card-2", idx, 3),
            "the KN idem_key for effect {idx} agrees with the Fabric rule"
        );
    }
}

/// **A consequential KN effect is WITHHELD (does NOT mutate, AG-8) until approval — the Fabric's
/// `Gated` semantics, KN-domain half.** A reversible effect passes directly.
#[test]
fn cdc_8_2_kn_consequential_effect_is_withheld_then_applies() {
    let gate = kn::KnowledgeEffectGate::new();
    let empty = BTreeSet::new();

    // publish is consequential → withheld (the Gated/withhold leg — 0 mutation).
    let refusal = gate.decide(kn::PUBLISH_TOOL, &empty, "card-1").unwrap_err();
    assert!(matches!(refusal, kn::EffectRefusal::Withheld { .. }));

    // once approved (the HITL resume), it passes the gate.
    let approved: BTreeSet<String> = [kn::PUBLISH_TOOL.to_string()].into_iter().collect();
    assert!(gate.decide(kn::PUBLISH_TOOL, &approved, "card-1").is_ok());

    // a reversible effect needs no approval (applies directly through the pipeline).
    assert!(gate.decide(kn::DRAFT_TOOL, &empty, "card-1").is_ok());
}

/// **reserve/settle (11.7) — "no balance → no agent write"; settle debits exactly the applied
/// effect's cost.** The Fabric's universal bookend, KN-domain view.
#[test]
fn cdc_8_2_kn_reserve_settle_bookends_the_run() {
    let mut b = kn::ReserveSettle::reserve(10);
    assert!(b.has_remaining(4));
    assert_eq!(b.settle(4), 4);
    assert_eq!(b.remaining(), 6);
    // no balance → no agent write.
    let zero = kn::ReserveSettle::reserve(0);
    assert!(!zero.has_remaining(1));
}

/// **The KN-D11 chained drill is the CONSUMER: agent plans → consequential publish WITHHELD (0
/// mutation) → approve → applied ONCE across a double-click → reserve/settle passed. The dated green
/// receipt proves 0 ungoverned mutation, 0 mutation before approval, 0 double-apply.**
#[test]
fn cdc_8_2_kn_d11_chained_drill_is_green() {
    let attr = kn::AgentEditAttribution::new("agent-7", "run:R1", "summarise → publish");
    let mut run = kn::KnowledgeAgentRun::begin(attr, 100);

    // a reversible append applies directly, attributed "suggested by agent".
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

    // a consequential publish is WITHHELD (0 mutation before approval).
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

    // approve → applies; a double-click is ONE approval (no second op).
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
