use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    gate_id_of, run_batch_hitl_loop, ApplyError, ApplyLedger, ApprovedTools, BatchApprovalCard,
    BatchGatedEffect, CapabilityCheck, DecisionScript, DelegationLookup, EffectBudget, EffectCost,
    EffectOutcome, PipelineSignals, PlanThenApply, PlannedEffect, RiskSummary, SubsystemApply,
    TenantGuard, WaitDecision,
};
use myelin_identity::{
    CaveatContext, Consistency, Decision, EffectivePolicy, Permission, Principal, PrincipalId,
    PrincipalKind, RuntimeRef, Zookie,
};
use myelin_storage::reserve_settle::MeteredUnit;
use myelin_tenancy::{ArtifactRef, TenantId};
use std::cell::RefCell;
use std::collections::BTreeSet;

struct Catalogue {
    defs: Vec<ToolDef>,
}
impl ToolSurface for Catalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

struct AllowAll {
    allow: BTreeSet<String>,
}
impl CapabilityCheck for AllowAll {
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> Decision {
        if self.allow.contains(&permission.0) {
            Decision::Allow
        } else {
            Decision::Deny
        }
    }
}

struct Delegate {
    caps: Vec<String>,
}
impl DelegationLookup for Delegate {
    fn delegation(&self, _a: &Principal, _t: &Principal) -> EffectivePolicy {
        EffectivePolicy {
            caveats: self.caps.clone(),
        }
    }
}

struct PermitAll;
impl TenantGuard for PermitAll {
    fn permits(&self, _a: &Principal, _t: &ToolName, _o: &ArtifactRef) -> bool {
        true
    }
}

struct Endpoint {
    applied: RefCell<Vec<String>>,
}
impl SubsystemApply for Endpoint {
    fn apply_public(
        &self,
        _a: &Principal,
        tool: &ToolName,
        object: &ArtifactRef,
        _input: &str,
    ) -> Result<EventId, ApplyError> {
        self.applied.borrow_mut().push(object.0.clone());
        Ok(EventId(format!("evt:{}:{}", tool.0, object.0)))
    }
}

struct Budget {
    remaining: u64,
}
impl EffectBudget for Budget {
    fn has_remaining(&self, cost: u64) -> bool {
        self.remaining >= cost
    }
    fn settle_one(&mut self, unit: &MeteredUnit) -> u64 {
        let total = unit.total().map(|m| m.0).unwrap_or(0);
        self.remaining = self.remaining.saturating_sub(total);
        total
    }
}

fn agent() -> Principal {
    Principal::stub(
        PrincipalId("psn:agent-7".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("mock".into()),
            on_behalf_of: None,
        },
        TenantId("acme".into()),
    )
}
fn human() -> Principal {
    Principal::stub(
        PrincipalId("psn:human-x".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn merge_tool() -> ToolDef {
    ToolDef {
        name: ToolName("git.merge".into()),
        subsystem: "git".into(),
        version: 1,
        input_schema:
            r#"{"type":"object","required":["pr"],"properties":{"pr":{"type":"integer"}}}"#.into(),
        required_caps: vec!["git.merge".into()],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval: true,
        exposed_over_mcp: false,
    }
}

fn merge_plan(pr: u32) -> PlannedEffect {
    PlannedEffect {
        tool: ToolName("git.merge".into()),
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

fn approvers() -> Vec<PrincipalId> {
    vec![
        PrincipalId("psn:lead".into()),
        PrincipalId("psn:maintainer".into()),
    ]
}

fn apply_once(
    cat: &Catalogue,
    endpoint: &Endpoint,
    plan: &PlannedEffect,
    approved: BTreeSet<String>,
) -> EffectResult {
    let check = AllowAll {
        allow: ["git.merge".to_string()].into_iter().collect(),
    };
    let del = Delegate {
        caps: vec!["git.merge".into()],
    };
    let tenant = PermitAll;
    let mut budget = Budget { remaining: 10_000 };
    let mut signals = PipelineSignals::new();
    let mut p = PlanThenApply {
        catalogue: cat,
        check: &check,
        delegation: &del,
        tenant: &tenant,
        apply_endpoint: endpoint,
        budget: &mut budget,
        agent: agent(),
        trigger_actor: human(),
        zookie: Zookie("z-1".into()),
        approved,
        signals: &mut signals,
    };
    p.apply_planned(plan)
}

fn withhold_three(cat: &Catalogue, endpoint: &Endpoint) -> BatchApprovalCard {
    let mut effects = Vec::new();
    for pr in [40u32, 41, 42] {
        let plan = merge_plan(pr);
        let result = apply_once(cat, endpoint, &plan, BTreeSet::new());
        let gate_id = gate_id_of(&result).expect("a requires_approval tool GATES (AG-8)");
        assert!(
            matches!(result, EffectResult::Gated(_)),
            "effect for pr {pr} is WITHHELD: {result:?}"
        );
        effects.push(BatchGatedEffect {
            gate_id,
            risk_summary: RiskSummary::for_action("agent.hitl.merge_pr", &plan.object),
            plan,
        });
    }
    assert_eq!(
        endpoint.applied.borrow().len(),
        0,
        "0 MUTATIONS before approval (AG-D5: the gated effects did NOT mutate)"
    );
    BatchApprovalCard {
        run_id: "R1".into(),
        card_id: "card-7".into(),
        effects,
        approver_filter: approvers(),
    }
}

#[test]
fn chained_partial_approval_applies_exactly_the_approved_effects() {
    let cat = Catalogue {
        defs: vec![merge_tool()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };

    let card = withhold_three(&cat, &endpoint);

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

    assert_eq!(
        endpoint.applied.borrow().len(),
        0,
        "the batch loop made 0 mutation (the apply is the re-run)"
    );
    assert!(matches!(outcome.effects[0], EffectOutcome::Applied { .. }));
    assert!(
        matches!(outcome.effects[1], EffectOutcome::Withheld { .. }),
        "effect 1 declined → WITHHELD"
    );
    assert!(matches!(outcome.effects[2], EffectOutcome::Applied { .. }));
    assert_eq!(
        outcome.approved_effect_count(),
        2,
        "exactly 2 approved effects (0 and 2)"
    );
    assert!(
        outcome.exactly_once(),
        "the apply-counter (ledger) == the approved-effect count (AG-D5)"
    );

    for (idx, eff) in card.effects.iter().enumerate() {
        let idem_key = card.idem_key_for(idx);
        if outcome.ledger.contains(&idem_key) {
            let result = apply_once(&cat, &endpoint, &eff.plan, approved.as_set());
            assert!(
                matches!(result, EffectResult::Applied(_)),
                "the approved effect {idx} APPLIES on the re-run"
            );
        } else {
            let result = apply_once(&cat, &endpoint, &eff.plan, BTreeSet::new());
            assert!(
                matches!(result, EffectResult::Gated(_)),
                "the declined effect {idx} still GATES (never applied, AG-8)"
            );
        }
    }
    let applied = endpoint.applied.borrow();
    assert_eq!(
        applied.len(),
        2,
        "1 apply per approved effect - exactly 2 (AG-D5: apply-counter == approved count)"
    );
    assert!(applied.contains(&"myelin://acme/git/pr/40".to_string()));
    assert!(applied.contains(&"myelin://acme/git/pr/42".to_string()));
    assert!(
        !applied.contains(&"myelin://acme/git/pr/41".to_string()),
        "the DECLINED effect (pr 41) made 0 mutation - apply was NEVER reached (AG-8)"
    );
}

#[test]
fn a_declined_sibling_sharing_a_tool_name_does_not_apply_on_re_drive() {
    let cat = Catalogue {
        defs: vec![merge_tool()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let card = withhold_three(&cat, &endpoint);

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
        matches!(outcome.effects[1], EffectOutcome::Withheld { .. }),
        "effect 1 was DECLINED"
    );

    let result = apply_once(&cat, &endpoint, &card.effects[1].plan, approved.as_set());
    assert!(
        !matches!(result, EffectResult::Applied(_)),
        "the DECLINED effect 1 (git.merge on pr 41) must NOT apply just because an approved sibling \
         shares its tool name - the step-6 gate must be per-effect, not tool-name-keyed: {result:?}"
    );
    assert!(
        !endpoint
            .applied
            .borrow()
            .iter()
            .any(|o| o.contains("pr/41")),
        "pr 41 made 0 mutation (AG-8) - the declined effect never reached apply"
    );

    for idx in [0usize, 2] {
        let r = apply_once(&cat, &endpoint, &card.effects[idx].plan, approved.as_set());
        assert!(
            matches!(r, EffectResult::Applied(_)),
            "the APPROVED effect {idx} still applies on the re-drive: {r:?}"
        );
    }
}

#[test]
fn chained_double_click_approve_all_applies_each_effect_once() {
    let cat = Catalogue {
        defs: vec![merge_tool()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let card = withhold_three(&cat, &endpoint);

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
        "a double-click adds 0 applies - exactly 3 (1 per effect)"
    );
    assert!(
        second.exactly_once(),
        "the apply-counter == the approved count (3), never 6 - one approval"
    );

    for eff in &card.effects {
        let result = apply_once(&cat, &endpoint, &eff.plan, approved.as_set());
        assert!(matches!(result, EffectResult::Applied(_)));
    }
    let applied = endpoint.applied.borrow();
    assert_eq!(
        applied.len(),
        3,
        "exactly 3 applies (1 per approved effect), NOT 6 - the double-click is one approval"
    );
    let distinct: BTreeSet<&String> = applied.iter().collect();
    assert_eq!(
        distinct.len(),
        3,
        "the three applies are the three distinct effects (pr 40/41/42)"
    );
}
