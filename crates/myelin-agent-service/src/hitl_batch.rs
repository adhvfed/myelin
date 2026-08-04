use crate::effect_api::PlannedEffect;
use crate::hitl::{
    resolve_decision, surface_card, ApprovedTools, Halted, HitlCard, HitlGate, RiskSummary,
    WaitDecision,
};
use myelin_agent::GateId;
use myelin_identity::PrincipalId;
use std::collections::{BTreeMap, BTreeSet};

pub fn per_effect_idem_key(card_id: &str, effect_idx: usize, total_effects: usize) -> String {
    debug_assert!(
        total_effects >= 1,
        "a card gates at least one effect (total_effects >= 1)"
    );
    debug_assert!(
        effect_idx < total_effects,
        "effect_idx ({effect_idx}) must index into the card's {total_effects} effect(s)"
    );
    if total_effects == 1 {
        card_id.to_string()
    } else {
        format!("{card_id}:{effect_idx}")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyLedger {
    applied: BTreeSet<String>,
}

impl ApplyLedger {
    pub fn new() -> ApplyLedger {
        ApplyLedger::default()
    }

    pub fn record(&mut self, idem_key: &str) -> bool {
        self.applied.insert(idem_key.to_string())
    }

    pub fn contains(&self, idem_key: &str) -> bool {
        self.applied.contains(idem_key)
    }

    pub fn applies(&self) -> usize {
        self.applied.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchGatedEffect {
    pub gate_id: GateId,
    pub plan: PlannedEffect,
    pub risk_summary: RiskSummary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchApprovalCard {
    pub run_id: String,
    pub card_id: String,
    pub effects: Vec<BatchGatedEffect>,
    pub approver_filter: Vec<PrincipalId>,
}

impl BatchApprovalCard {
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    pub fn idem_key_for(&self, idx: usize) -> String {
        per_effect_idem_key(&self.card_id, idx, self.effects.len())
    }

    fn open_gate(&self, idx: usize) -> HitlGate {
        let eff = &self.effects[idx];
        HitlGate::open(
            eff.gate_id.clone(),
            self.run_id.clone(),
            &eff.plan,
            eff.risk_summary.clone(),
            self.approver_filter.clone(),
            self.idem_key_for(idx),
        )
    }
}

pub trait BatchHitlWait {
    fn park_and_wait_effect(&self, gate: &HitlGate, idem_key: &str) -> WaitDecision;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EffectOutcome {
    Applied { idem_key: String, tool: String },
    Withheld { idem_key: String, halted: Halted },
}

impl EffectOutcome {
    pub fn idem_key(&self) -> &str {
        match self {
            EffectOutcome::Applied { idem_key, .. } | EffectOutcome::Withheld { idem_key, .. } => {
                idem_key
            }
        }
    }

    pub fn applied(&self) -> bool {
        matches!(self, EffectOutcome::Applied { .. })
    }
}

#[derive(Clone, Debug)]
pub struct BatchOutcome {
    pub effects: Vec<EffectOutcome>,
    pub approved: ApprovedTools,
    pub ledger: ApplyLedger,
}

impl BatchOutcome {
    pub fn approved_effect_count(&self) -> usize {
        self.effects.iter().filter(|o| o.applied()).count()
    }

    pub fn exactly_once(&self) -> bool {
        self.ledger.applies() == self.approved_effect_count()
    }
}

pub fn run_batch_hitl_loop<W: BatchHitlWait>(
    card: &BatchApprovalCard,
    wait: &W,
    approved: &mut ApprovedTools,
    ledger: &mut ApplyLedger,
) -> BatchOutcome {
    let _cards: Vec<HitlCard> = (0..card.len())
        .map(|idx| surface_card(&card.open_gate(idx)))
        .collect();

    let mut outcomes = Vec::with_capacity(card.len());
    for idx in 0..card.len() {
        let idem_key = card.idem_key_for(idx);
        let mut gate = card.open_gate(idx);
        let decision = wait.park_and_wait_effect(&gate, &idem_key);
        let outcome = match resolve_decision(&mut gate, decision, approved) {
            Ok(()) => {
                ledger.record(&idem_key);
                EffectOutcome::Applied {
                    idem_key,
                    tool: gate.tool_name.clone(),
                }
            }
            Err(halted) => EffectOutcome::Withheld { idem_key, halted },
        };
        outcomes.push(outcome);
    }

    BatchOutcome {
        effects: outcomes,
        approved: approved.clone(),
        ledger: ledger.clone(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct DecisionScript {
    by_key: BTreeMap<String, WaitDecision>,
}

impl DecisionScript {
    pub fn new() -> DecisionScript {
        DecisionScript::default()
    }

    pub fn decide(&mut self, idem_key: impl Into<String>, decision: WaitDecision) -> &mut Self {
        self.by_key.insert(idem_key.into(), decision);
        self
    }

    pub fn decision_for(&self, idem_key: &str) -> WaitDecision {
        self.by_key
            .get(idem_key)
            .cloned()
            .unwrap_or(WaitDecision::Expired)
    }
}

impl BatchHitlWait for DecisionScript {
    fn park_and_wait_effect(&self, _gate: &HitlGate, idem_key: &str) -> WaitDecision {
        self.decision_for(idem_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect_api::EffectCost;
    use myelin_agent::ToolName;
    use myelin_tenancy::ArtifactRef;

    fn plan(tool: &str, pr: u32) -> PlannedEffect {
        PlannedEffect {
            tool: ToolName(tool.into()),
            object: ArtifactRef(format!("myelin://acme/git/pr/{pr}")),
            input_json: format!(r#"{{"pr":{pr}}}"#),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "git.merge",
                wholesale: 30,
                markup: 20,
            },
        }
    }

    fn risk(pr: u32) -> RiskSummary {
        RiskSummary::for_action(
            "agent.hitl.merge_pr",
            &ArtifactRef(format!("myelin://acme/git/pr/{pr}")),
        )
    }

    fn gated(tool: &str, pr: u32, gate: &str) -> BatchGatedEffect {
        BatchGatedEffect {
            gate_id: GateId(gate.into()),
            plan: plan(tool, pr),
            risk_summary: risk(pr),
        }
    }

    fn approvers() -> Vec<PrincipalId> {
        vec![
            PrincipalId("psn:lead".into()),
            PrincipalId("psn:maintainer".into()),
        ]
    }

    fn three_effect_card() -> BatchApprovalCard {
        BatchApprovalCard {
            run_id: "R1".into(),
            card_id: "card-7".into(),
            effects: vec![
                gated("git.merge", 40, "gate:0"),
                gated("git.merge", 41, "gate:1"),
                gated("git.merge", 42, "gate:2"),
            ],
            approver_filter: approvers(),
        }
    }

    fn single_effect_card() -> BatchApprovalCard {
        BatchApprovalCard {
            run_id: "R1".into(),
            card_id: "card-1".into(),
            effects: vec![gated("git.merge", 42, "gate:0")],
            approver_filter: approvers(),
        }
    }

    #[test]
    fn per_effect_idem_key_follows_the_frozen_rule() {
        assert_eq!(per_effect_idem_key("card-7", 0, 1), "card-7");
        assert_eq!(per_effect_idem_key("card-7", 0, 3), "card-7:0");
        assert_eq!(per_effect_idem_key("card-7", 1, 3), "card-7:1");
        assert_eq!(per_effect_idem_key("card-7", 2, 3), "card-7:2");
    }

    #[test]
    fn card_idem_key_for_uses_the_per_effect_rule() {
        let multi = three_effect_card();
        assert_eq!(multi.idem_key_for(0), "card-7:0");
        assert_eq!(multi.idem_key_for(1), "card-7:1");
        assert_eq!(multi.idem_key_for(2), "card-7:2");
        let single = single_effect_card();
        assert_eq!(
            single.idem_key_for(0),
            "card-1",
            "single-effect card keys on the bare card id"
        );
    }

    #[test]
    fn apply_ledger_records_each_key_exactly_once() {
        let mut ledger = ApplyLedger::new();
        assert_eq!(ledger.applies(), 0);
        assert!(ledger.record("card-7:0"), "first apply of a key proceeds");
        assert!(ledger.record("card-7:2"), "a distinct key applies");
        assert!(
            !ledger.record("card-7:0"),
            "a RE-apply of the same key is a no-op (double-click)"
        );
        assert_eq!(
            ledger.applies(),
            2,
            "exactly two distinct applies (the double-click did not count)"
        );
        assert!(ledger.contains("card-7:0"));
        assert!(
            !ledger.contains("card-7:1"),
            "the declined effect 1 never applied"
        );
    }

    #[test]
    fn partial_approval_two_of_three_applies_exactly_the_approved_effects() {
        let card = three_effect_card();
        let mut script = DecisionScript::new();
        script
            .decide(card.idem_key_for(0), WaitDecision::Approve)
            .decide(
                card.idem_key_for(1),
                WaitDecision::Reject("pr 41 fails checks".into()),
            )
            .decide(card.idem_key_for(2), WaitDecision::Approve);

        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let outcome = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);

        assert!(
            matches!(outcome.effects[0], EffectOutcome::Applied { .. }),
            "effect 0 approved → applied"
        );
        assert!(
            matches!(&outcome.effects[1], EffectOutcome::Withheld { halted: Halted::Rejected(r), .. } if r == "pr 41 fails checks"),
            "effect 1 declined → WITHHELD with the reason (0 mutation, AG-8): {:?}",
            outcome.effects[1]
        );
        assert!(
            matches!(outcome.effects[2], EffectOutcome::Applied { .. }),
            "effect 2 approved → applied"
        );

        assert_eq!(
            outcome.ledger.applies(),
            2,
            "exactly 2 applies (effects 0 and 2)"
        );
        assert_eq!(outcome.approved_effect_count(), 2);
        assert!(
            outcome.exactly_once(),
            "the apply-counter == the approved-effect count (AG-D5)"
        );
        assert!(outcome.ledger.contains("card-7:0"));
        assert!(
            !outcome.ledger.contains("card-7:1"),
            "the declined effect 1 made 0 mutation (AG-8)"
        );
        assert!(outcome.ledger.contains("card-7:2"));
    }

    #[test]
    fn double_click_approve_all_applies_each_effect_once() {
        let card = three_effect_card();
        let mut script = DecisionScript::new();
        for idx in 0..card.len() {
            script.decide(card.idem_key_for(idx), WaitDecision::Approve);
        }

        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let first = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(
            first.ledger.applies(),
            3,
            "the first click applies all three effects"
        );

        let second = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(
            second.ledger.applies(),
            3,
            "a double-click on approve-all adds 0 applies (the per-effect keys dedup the apply)"
        );
        assert_eq!(second.approved_effect_count(), 3);
        assert!(
            second.exactly_once(),
            "exactly 3 applies (1 per effect), NOT 6 - the double-click is one approval"
        );
    }

    #[test]
    fn single_effect_card_double_click_is_one_apply() {
        let card = single_effect_card();
        let mut script = DecisionScript::new();
        script.decide(card.idem_key_for(0), WaitDecision::Approve);

        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let first = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(first.ledger.applies(), 1);
        assert!(
            first.ledger.contains("card-1"),
            "the single effect keys on the bare card id"
        );

        let second = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(
            second.ledger.applies(),
            1,
            "a single-effect double-click is ONE apply"
        );
        assert!(second.exactly_once());
    }

    #[test]
    fn all_declined_batch_makes_zero_mutation() {
        let card = three_effect_card();
        let mut script = DecisionScript::new();
        for idx in 0..card.len() {
            script.decide(card.idem_key_for(idx), WaitDecision::Reject("no".into()));
        }
        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let outcome = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(
            outcome.ledger.applies(),
            0,
            "0 applies - every effect declined (AG-8)"
        );
        assert_eq!(outcome.approved_effect_count(), 0);
        assert!(
            outcome.exactly_once(),
            "0 applies == 0 approved effects (trivially exactly-once)"
        );
        assert!(outcome
            .effects
            .iter()
            .all(|o| matches!(o, EffectOutcome::Withheld { .. })));
        assert!(
            approved.as_set().is_empty(),
            "no tool admitted (0 mutation)"
        );
    }

    #[test]
    fn an_undecided_effect_auto_denies_zero_mutation() {
        let card = three_effect_card();
        let mut script = DecisionScript::new();
        script.decide(card.idem_key_for(0), WaitDecision::Approve);

        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let outcome = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert!(
            matches!(outcome.effects[0], EffectOutcome::Applied { .. }),
            "effect 0 approved → applied"
        );
        assert!(
            matches!(
                &outcome.effects[1],
                EffectOutcome::Withheld {
                    halted: Halted::Expired,
                    ..
                }
            ),
            "effect 1 undecided → auto-deny (Expired), 0 mutation: {:?}",
            outcome.effects[1]
        );
        assert!(matches!(
            &outcome.effects[2],
            EffectOutcome::Withheld {
                halted: Halted::Expired,
                ..
            }
        ));
        assert_eq!(
            outcome.ledger.applies(),
            1,
            "exactly 1 apply (only the decided-approve effect 0)"
        );
        assert!(outcome.exactly_once());
    }

    #[test]
    fn each_outcome_carries_its_per_effect_key() {
        let card = three_effect_card();
        let mut script = DecisionScript::new();
        script
            .decide(card.idem_key_for(0), WaitDecision::Approve)
            .decide(card.idem_key_for(1), WaitDecision::Reject("x".into()))
            .decide(card.idem_key_for(2), WaitDecision::Approve);
        let mut approved = ApprovedTools::new();
        let mut ledger = ApplyLedger::new();
        let outcome = run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
        assert_eq!(outcome.effects[0].idem_key(), "card-7:0");
        assert_eq!(outcome.effects[1].idem_key(), "card-7:1");
        assert_eq!(outcome.effects[2].idem_key(), "card-7:2");
    }
}
