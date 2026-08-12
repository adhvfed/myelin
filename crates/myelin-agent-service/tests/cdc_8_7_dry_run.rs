use myelin_agent::{
    BudgetView, DryRun, EffectKind, EventId, InboxEvent, ProposedEffect, StepOutcome, Submission,
    SystemContext, ToolCall, ToolCallId, ToolDef, ToolName, ToolSchema, ToolSurface,
};

fn schema(name: &str) -> ToolSchema {
    ToolSchema {
        name: ToolName(name.into()),
        description: String::new(),
        input_schema: "{}".into(),
    }
}

fn call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId(format!("call:{name}")),
        name: ToolName(name.into()),
        arguments: serde_json::Value::Null,
    }
}
use myelin_agent_service::{
    encode_proposed, ApplyError, CapabilityCheck, DelegationLookup, DryRunBridge, DryRunPlanner,
    EffectBudget, EffectCost, MockAgentRuntime, MockScript, PipelineSignals, PlanThenApply,
    PlanVerdict, PlannedEffect, SubsystemApply, TenantGuard, MOCK_MAX_STEPS,
};
use myelin_identity::{
    CaveatContext, Consistency, Decision, EffectivePolicy, Permission, Principal, PrincipalId,
    PrincipalKind, RuntimeRef, Zookie,
};
use myelin_storage::reserve_settle::MeteredUnit;
use myelin_tenancy::{ArtifactRef, TenantId};
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

struct AllowAll;
impl CapabilityCheck for AllowAll {
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> Decision {
        Decision::Allow
    }
}

struct Delegator {
    policy: Vec<String>,
}
impl DelegationLookup for Delegator {
    fn delegation(&self, _a: &Principal, _t: &Principal) -> EffectivePolicy {
        EffectivePolicy {
            caveats: self.policy.clone(),
        }
    }
}

struct AllowTenant;
impl TenantGuard for AllowTenant {
    fn permits(&self, _a: &Principal, _t: &ToolName, _o: &ArtifactRef) -> bool {
        true
    }
}

struct NeverApply;
impl SubsystemApply for NeverApply {
    fn apply_public(
        &self,
        _a: &Principal,
        _t: &ToolName,
        _o: &ArtifactRef,
        _i: &str,
    ) -> Result<EventId, ApplyError> {
        panic!("CDC 8.7: a dry-run must NEVER call the subsystem PUBLIC endpoint");
    }
}

struct ReadOnlyBudget {
    remaining: u64,
}
impl EffectBudget for ReadOnlyBudget {
    fn has_remaining(&self, cost: u64) -> bool {
        self.remaining >= cost
    }
    fn settle_one(&mut self, _u: &MeteredUnit) -> u64 {
        panic!("CDC 8.7: a dry-run must NEVER meter (settle_one)");
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
        PrincipalId("psn:human".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn tool_def(name: &str, caps: &[&str], requires_approval: bool) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: "issues".into(),
        version: 1,
        input_schema: r#"{"type":"object","required":["x"],"properties":{"x":{"type":"string"}}}"#
            .into(),
        required_caps: caps.iter().map(|c| c.to_string()).collect(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval,
        exposed_over_mcp: false,
    }
}

fn planned(tool: &str) -> PlannedEffect {
    PlannedEffect {
        tool: ToolName(tool.into()),
        object: ArtifactRef(format!("myelin://acme/x/{tool}")),
        input_json: r#"{"x":"v"}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "agent.effect",
            wholesale: 5,
            markup: 2,
        },
    }
}

fn effect_for(name: &ToolName) -> Option<PlannedEffect> {
    match name.0.as_str() {
        "merge" | "post" => Some(planned(&name.0)),
        _ => None,
    }
}

fn script() -> MockScript {
    MockScript::new(
        SystemContext("agent-7".into()),
        vec![schema("merge"), schema("post")],
        BudgetView(1000),
        vec![
            StepOutcome::UseTools(vec![call("merge")]),
            StepOutcome::UseTools(vec![call("post")]),
            StepOutcome::Submit(Submission("done".into())),
        ],
    )
}

#[test]
fn cdc_8_7_dry_run_returns_the_plan_with_unchanged_wallet() {
    let cat = Catalogue {
        defs: vec![
            tool_def("merge", &["merge"], true),
            tool_def("post", &["post"], false),
        ],
    };
    let check = AllowAll;
    let del = Delegator {
        policy: vec!["merge".into(), "post".into()],
    };
    let tenant = AllowTenant;
    let endpoint = NeverApply;
    let mut budget = ReadOnlyBudget { remaining: 1000 };
    let mut signals = PipelineSignals::new();
    let sc = script();
    let (plan, entries) = {
        let p = PlanThenApply {
            catalogue: &cat,
            check: &check,
            delegation: &del,
            tenant: &tenant,
            apply_endpoint: &endpoint,
            budget: &mut budget,
            agent: agent(),
            trigger_actor: human(),
            zookie: Zookie("z".into()),
            approved: BTreeSet::new(),
            signals: &mut signals,
        };
        let brain = MockAgentRuntime::new(sc.clone());
        let planner = DryRunPlanner::new(&p, effect_for, MOCK_MAX_STEPS);
        (
            planner.plan(&brain, &sc),
            planner.plan_with_verdicts(&brain, &sc),
        )
    };

    assert_eq!(
        plan.len(),
        2,
        "the dry-run plan holds both proposed effects"
    );
    assert_eq!(plan[0], encode_proposed(&planned("merge")));
    assert_eq!(plan[1], encode_proposed(&planned("post")));

    assert!(
        matches!(entries[0].verdict, PlanVerdict::WouldGate(_)),
        "merge gates (AG-8)"
    );
    assert_eq!(
        entries[1].verdict,
        PlanVerdict::WouldApply,
        "post would apply"
    );

    assert_eq!(
        budget.remaining, 1000,
        "the reserve balance is unchanged after a dry-run"
    );
    assert_eq!(signals.applied(), 0, "0 applies");
    assert_eq!(signals.metered_total(), 0, "0 metered effects");
}

#[test]
fn cdc_8_7_frozen_glue_dry_run_trait_body() {
    let cat = Catalogue {
        defs: vec![
            tool_def("merge", &["merge"], true),
            tool_def("post", &["post"], false),
        ],
    };
    let check = AllowAll;
    let del = Delegator {
        policy: vec!["merge".into(), "post".into()],
    };
    let tenant = AllowTenant;
    let endpoint = NeverApply;
    let mut budget = ReadOnlyBudget { remaining: 1000 };
    let mut signals = PipelineSignals::new();
    let sc = script();
    let plan: Vec<ProposedEffect> = {
        let p = PlanThenApply {
            catalogue: &cat,
            check: &check,
            delegation: &del,
            tenant: &tenant,
            apply_endpoint: &endpoint,
            budget: &mut budget,
            agent: agent(),
            trigger_actor: human(),
            zookie: Zookie("z".into()),
            approved: BTreeSet::new(),
            signals: &mut signals,
        };
        let planner = DryRunPlanner::new(&p, effect_for, MOCK_MAX_STEPS);
        let brain: Box<dyn myelin_agent::AgentRuntime> =
            Box::new(MockAgentRuntime::new(sc.clone()));
        let bridge = DryRunBridge::new(planner, brain, sc);
        bridge.dry_run(InboxEvent("issue.created".into()))
    };
    assert_eq!(plan.len(), 2, "the frozen DryRun body returns the plan");
    assert_eq!(
        budget.remaining, 1000,
        "side-effect-free through the frozen trait too"
    );
}

#[test]
fn cdc_8_7_ag_d9_effect_sequence_is_byte_identical_across_two_runs() {
    let cat = Catalogue {
        defs: vec![
            tool_def("merge", &["merge"], true),
            tool_def("post", &["post"], false),
        ],
    };
    let check = AllowAll;
    let del = Delegator {
        policy: vec!["merge".into(), "post".into()],
    };
    let tenant = AllowTenant;
    let endpoint = NeverApply;
    let mut budget = ReadOnlyBudget { remaining: 1000 };
    let mut signals = PipelineSignals::new();
    let p = PlanThenApply {
        catalogue: &cat,
        check: &check,
        delegation: &del,
        tenant: &tenant,
        apply_endpoint: &endpoint,
        budget: &mut budget,
        agent: agent(),
        trigger_actor: human(),
        zookie: Zookie("z".into()),
        approved: BTreeSet::new(),
        signals: &mut signals,
    };
    let sc = script();
    let brain = MockAgentRuntime::new(sc.clone());
    let planner = DryRunPlanner::new(&p, effect_for, MOCK_MAX_STEPS);

    let first = planner.plan(&brain, &sc);
    let second = planner.plan(&brain, &sc);
    assert_eq!(
        first, second,
        "AG-D9: two dry-runs of the same script are byte-identical"
    );
    assert_eq!(first[0].0, second[0].0);
    assert_eq!(first[1].0, second[1].0);
}
