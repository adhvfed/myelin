use myelin_agent::GateId;
use myelin_agent::{EffectKind, EffectResult, EventId, ToolDef, ToolName, ToolSurface};
use myelin_agent_service::{
    chat_tool_defs, ci_tool_defs, deploy_tool_def, forecast_tool_def, issues_tool_defs,
    landing_requires_approval, post_message_tool_def, react_tool_def, register_chat_tools,
    register_ci_tools, register_issues_tools, run_pipeline_tool_def, transition_caveat,
    transition_tool_def, ApplyError, ApplyLedger, ApprovedTools, BatchApprovalCard,
    BatchGatedEffect, CapabilityCheck, DecisionScript, DelegationLookup, EffectBudget, EffectCost,
    PipelineSignals, PlanThenApply, PlannedEffect, RiskSummary, SubsystemApply, TenantGuard,
    WaitDecision,
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

struct CheckProvider {
    allow: BTreeSet<String>,
    transition_needs_approver: bool,
}
impl CapabilityCheck for CheckProvider {
    fn check(
        &self,
        _s: &Principal,
        permission: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> Decision {
        let sla_bound = self.transition_needs_approver
            && caveat.map(|c| c.transition.is_some()).unwrap_or(false);
        if sla_bound {
            Decision::Conditional
        } else if self.allow.contains(&permission.0) {
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
        self.applied.borrow_mut().push(tool.0.clone());
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

fn apply_once(
    cat: &Catalogue,
    endpoint: &Endpoint,
    check: &CheckProvider,
    caps: Vec<String>,
    approved: BTreeSet<String>,
    plan: &PlannedEffect,
) -> (EffectResult, usize) {
    let del = Delegate { caps };
    let tenant = PermitAll;
    let mut budget = Budget { remaining: 10_000 };
    let mut signals = PipelineSignals::new();
    let mut p = PlanThenApply {
        catalogue: cat,
        check,
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
    let out = p.apply_planned(plan);
    let muts = endpoint.applied.borrow().len();
    (out, muts)
}

#[test]
fn consumer_tooldefs_carry_their_frozen_6_3_defaults() {
    let issues = issues_tool_defs();
    let gated_issues: Vec<&str> = issues
        .iter()
        .filter(|d| d.requires_approval)
        .map(|d| d.name.0.as_str())
        .collect();
    assert_eq!(
        gated_issues,
        vec!["transition"],
        "only the SLA-bound transition is gated; forecast/triage/sla_draft are advisory"
    );

    assert!(chat_tool_defs().iter().all(|d| !d.requires_approval));

    let ci = ci_tool_defs();
    let gated_ci: Vec<&str> = ci
        .iter()
        .filter(|d| d.requires_approval)
        .map(|d| d.name.0.as_str())
        .collect();
    assert_eq!(
        gated_ci,
        vec!["deploy", "approve_deploy", "write_secret"],
        "the three privileged CI gates; run_pipeline (non-prod) is not gated"
    );
    assert!(!run_pipeline_tool_def().requires_approval);
}

#[test]
fn cdc_8_1_4_9_all_consumer_tools_register_into_the_one_surface() {
    let mut cat = Catalogue { defs: vec![] };
    let issues = register_issues_tools(&mut cat).expect("issues seeded defs admit");
    let chat = register_chat_tools(&mut cat).expect("chat seeded defs admit");
    let ci = register_ci_tools(&mut cat).expect("ci seeded defs admit");
    assert_eq!(issues.len() + chat.len() + ci.len(), 4 + 2 + 4);

    assert_eq!(
        cat.resolve(&ToolName("transition".into()))
            .unwrap()
            .required_caps,
        vec!["issue_transition.perform_transition".to_string()]
    );
    assert_eq!(
        cat.resolve(&ToolName("post_message".into()))
            .unwrap()
            .required_caps,
        vec!["channel.post".to_string()]
    );
    assert_eq!(
        cat.resolve(&ToolName("deploy".into()))
            .unwrap()
            .required_caps,
        vec!["environment.deploy".to_string()]
    );
    assert_eq!(
        cat.resolve(&ToolName("write_secret".into()))
            .unwrap()
            .required_caps,
        vec!["ci_project.administer".to_string()]
    );
}

#[test]
fn chat_invoked_effect_is_governed_where_it_lands() {
    assert!(
        landing_requires_approval("ci", "deploy"),
        "a chat-invoked ci.deploy lands in ci → gated"
    );
    assert!(
        landing_requires_approval("issues", "transition"),
        "a chat-invoked issues.transition lands in issues → gated (the SLA floor)"
    );
    assert!(
        !landing_requires_approval("issues", "forecast"),
        "a chat-invoked issues.forecast lands in issues → advisory (NOT gated)"
    );
}

#[test]
fn iss_d12_governed_transition_withheld_then_approved_applies_once() {
    let cat = Catalogue {
        defs: vec![transition_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let check = CheckProvider {
        allow: ["issue_transition.perform_transition".to_string()]
            .into_iter()
            .collect(),
        transition_needs_approver: false,
    };
    let caps = vec!["issue_transition.perform_transition".to_string()];

    let object = ArtifactRef("myelin://acme/issues/issue/PROJ-1".into());
    let caveat = transition_caveat(object.clone(), "issue:PROJ-1:open->done");
    let plan = PlannedEffect {
        tool: ToolName("transition".into()),
        object: object.clone(),
        input_json: r#"{"issue":"PROJ-1","to_state":"done"}"#.into(),
        field: None,
        transition: caveat.transition.clone(),
        cost: EffectCost {
            unit: "issue.transition",
            wholesale: 10,
            markup: 5,
        },
    };

    let (withheld, muts0) = apply_once(
        &cat,
        &endpoint,
        &check,
        caps.clone(),
        BTreeSet::new(),
        &plan,
    );
    assert!(
        matches!(withheld, EffectResult::Gated(_)),
        "the governed transition is WITHHELD (Gated), never applied: {withheld:?}"
    );
    assert_eq!(muts0, 0, "ISS-D12: 0 mutation before approval (AG-8)");

    let approved: BTreeSet<String> = [myelin_agent_service::effect_gate_key(
        &plan.tool,
        &plan.object,
    )]
    .into_iter()
    .collect();
    let (applied, muts1) = apply_once(&cat, &endpoint, &check, caps, approved, &plan);
    assert!(
        matches!(applied, EffectResult::Applied(_)),
        "after approval the transition APPLIES: {applied:?}"
    );
    assert_eq!(muts1, 1, "ISS-D12: exactly one apply after approval");
}

#[test]
fn iss_d12_sla_bound_transition_without_approver_context_is_denied() {
    let cat = Catalogue {
        defs: vec![transition_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let check = CheckProvider {
        allow: ["issue_transition.perform_transition".to_string()]
            .into_iter()
            .collect(),
        transition_needs_approver: true,
    };
    let caps = vec!["issue_transition.perform_transition".to_string()];
    let object = ArtifactRef("myelin://acme/issues/issue/PROJ-9".into());
    let plan = PlannedEffect {
        tool: ToolName("transition".into()),
        object: object.clone(),
        input_json: r#"{"issue":"PROJ-9","to_state":"done"}"#.into(),
        field: None,
        transition: transition_caveat(object, "issue:PROJ-9:open->done").transition,
        cost: EffectCost {
            unit: "issue.transition",
            wholesale: 10,
            markup: 5,
        },
    };
    let approved: BTreeSet<String> = [myelin_agent_service::effect_gate_key(
        &plan.tool,
        &plan.object,
    )]
    .into_iter()
    .collect();
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, approved, &plan);
    assert!(
        matches!(out, EffectResult::Denied(_)),
        "Conditional (caveat unmet) is a DENY, never a silent allow: {out:?}"
    );
    assert_eq!(muts, 0, "a denied governed transition makes 0 mutation");
}

fn gated_effect(tool: &str, idx: u32, gate: &str) -> BatchGatedEffect {
    let object = ArtifactRef(format!("myelin://acme/ci/deploy/{idx}"));
    BatchGatedEffect {
        gate_id: GateId(gate.into()),
        plan: PlannedEffect {
            tool: ToolName(tool.into()),
            object: object.clone(),
            input_json: r#"{"environment":"prod"}"#.into(),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "ci.deploy",
                wholesale: 40,
                markup: 10,
            },
        },
        risk_summary: RiskSummary::for_action("agent.hitl.ci_deploy", &object),
    }
}

fn three_deploy_card() -> BatchApprovalCard {
    BatchApprovalCard {
        run_id: "run-1".into(),
        card_id: "card-1".into(),
        effects: vec![
            gated_effect("deploy", 0, "g0"),
            gated_effect("deploy", 1, "g1"),
            gated_effect("deploy", 2, "g2"),
        ],
        approver_filter: vec![PrincipalId("psn:lead".into())],
    }
}

#[test]
fn chat_d9_d10_batch_partial_approval_exactly_once_across_a_redrive() {
    let card = three_deploy_card();
    let mut script = DecisionScript::new();
    script
        .decide(card.idem_key_for(0), WaitDecision::Approve)
        .decide(card.idem_key_for(1), WaitDecision::Reject("not now".into()))
        .decide(card.idem_key_for(2), WaitDecision::Approve);

    let mut approved = ApprovedTools::new();
    let mut ledger = ApplyLedger::new();

    let out = myelin_agent_service::run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(
        out.approved_effect_count(),
        2,
        "exactly two of three effects were approved (0 and 2); 1 was withheld"
    );
    assert_eq!(
        out.ledger.applies(),
        2,
        "CHAT-D9/D10: exactly two applies (the approved effects)"
    );
    assert!(
        out.exactly_once(),
        "the apply-counter equals the approved-effect count exactly (AG-D5)"
    );
    assert!(
        !out.effects[1].applied(),
        "the declined effect makes 0 mutation (AG-8)"
    );

    let before = ledger.applies();
    let out2 =
        myelin_agent_service::run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(
        ledger.applies(),
        before,
        "CHAT-D9/D10: the double-click / kill-replay re-sends the same keys → 0 new applies"
    );
    assert!(
        out2.exactly_once(),
        "still exactly-once after the re-drive (no double-apply)"
    );
}

#[test]
fn chat_d9_single_gated_deploy_survives_a_kill_exactly_once() {
    let card = BatchApprovalCard {
        run_id: "run-2".into(),
        card_id: "card-2".into(),
        effects: vec![gated_effect("deploy", 7, "g7")],
        approver_filter: vec![PrincipalId("psn:lead".into())],
    };
    let mut script = DecisionScript::new();
    script.decide(card.idem_key_for(0), WaitDecision::Approve);

    let mut approved = ApprovedTools::new();
    let mut ledger = ApplyLedger::new();

    myelin_agent_service::run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(ledger.applies(), 1, "the approved deploy applies once");

    myelin_agent_service::run_batch_hitl_loop(&card, &script, &mut approved, &mut ledger);
    assert_eq!(
        ledger.applies(),
        1,
        "CHAT-D9: across the Chat+Workflow kill the gated deploy runs EXACTLY ONCE"
    );
}

#[test]
fn issues_advisory_forecast_applies_directly_no_gate() {
    let cat = Catalogue {
        defs: vec![forecast_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let check = CheckProvider {
        allow: ["issue.transition".to_string()].into_iter().collect(),
        transition_needs_approver: false,
    };
    let caps = vec!["issue.transition".to_string()];
    let plan = PlannedEffect {
        tool: ToolName("forecast".into()),
        object: ArtifactRef("myelin://acme/issues/issue/PROJ-2".into()),
        input_json: r#"{"issue":"PROJ-2","horizon_days":14}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "issue.forecast",
            wholesale: 2,
            markup: 1,
        },
    };
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, BTreeSet::new(), &plan);
    assert!(
        matches!(out, EffectResult::Applied(_)),
        "an advisory forecast applies directly (suggest-by-default, no gate): {out:?}"
    );
    assert_eq!(
        muts, 1,
        "exactly one apply (no withhold for the advisory tool)"
    );
}

#[test]
fn chat_post_and_react_apply_directly_no_gate() {
    for tool in [post_message_tool_def(), react_tool_def()] {
        assert!(
            !tool.requires_approval,
            "chat.{} is reversible → NOT gated",
            tool.name.0
        );
        assert_eq!(tool.effect_kind, EffectKind::Mutate);
    }
}

#[test]
fn ci_deploy_is_withheld_until_approval() {
    let cat = Catalogue {
        defs: vec![deploy_tool_def()],
    };
    let endpoint = Endpoint {
        applied: RefCell::new(vec![]),
    };
    let check = CheckProvider {
        allow: ["environment.deploy".to_string()].into_iter().collect(),
        transition_needs_approver: false,
    };
    let caps = vec!["environment.deploy".to_string()];
    let plan = PlannedEffect {
        tool: ToolName("deploy".into()),
        object: ArtifactRef("myelin://acme/ci/environment/prod".into()),
        input_json: r#"{"environment":"prod","artifact":"build-9"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "ci.deploy",
            wholesale: 40,
            markup: 10,
        },
    };
    let (out, muts) = apply_once(&cat, &endpoint, &check, caps, BTreeSet::new(), &plan);
    assert!(
        matches!(out, EffectResult::Gated(_)),
        "ci.deploy WITHHOLDS until approval (the frozen §6.3 gate): {out:?}"
    );
    assert_eq!(muts, 0, "0 mutation before the deploy approval (AG-8)");
}
