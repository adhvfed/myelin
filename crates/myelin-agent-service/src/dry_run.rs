use crate::effect_api::{
    CapabilityCheck, DelegationLookup, EffectBudget, PlanThenApply, PlanVerdict, PlannedEffect,
    SubsystemApply, TenantGuard,
};
use crate::mock::{build_conversation, MockScript, TraceHistory, MOCK_MAX_STEPS};
use myelin_agent::{
    AgentRuntime, DryRun, InboxEvent, ProposedEffect, StepOutcome, ToolName, ToolOutcome,
    ToolResult, ToolSurface,
};

pub fn proposed_effect_sequence<F>(
    brain: &dyn AgentRuntime,
    script: &MockScript,
    effect_for: &F,
    max_steps: usize,
) -> Vec<ProposedEffect>
where
    F: Fn(&ToolName) -> Option<PlannedEffect>,
{
    let mut history = TraceHistory::new();
    let mut effects = Vec::new();

    for _ in 0..max_steps {
        let conv = build_conversation(script, &history);
        let outcome = brain.step(&conv);
        match &outcome {
            StepOutcome::Submit(_) => {
                history.push_model(outcome.clone());
                break;
            }
            StepOutcome::UseTools(calls) => {
                for call in calls {
                    if let Some(plan) = effect_for(&call.name) {
                        effects.push(crate::effect_api::encode_proposed(&plan));
                    }
                }
                history.push_model(outcome.clone());
                let results: Vec<ToolOutcome> = calls
                    .iter()
                    .map(|call| ToolOutcome {
                        call_id: call.id.clone(),
                        result: ToolResult(format!("tool:{}:result", call.name.0)),
                    })
                    .collect();
                history.push_tool_results(results);
            }
        }
    }

    effects
}

pub struct DryRunPlanner<'p, 'a, S, C, D, T, A, B, F>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
    F: Fn(&ToolName) -> Option<PlannedEffect>,
{
    pipeline: &'p PlanThenApply<'a, S, C, D, T, A, B>,
    effect_for: F,
    max_steps: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryRunEntry {
    pub effect: ProposedEffect,
    pub verdict: PlanVerdict,
}

impl<'p, 'a, S, C, D, T, A, B, F> DryRunPlanner<'p, 'a, S, C, D, T, A, B, F>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
    F: Fn(&ToolName) -> Option<PlannedEffect>,
{
    pub fn new(
        pipeline: &'p PlanThenApply<'a, S, C, D, T, A, B>,
        effect_for: F,
        max_steps: usize,
    ) -> Self {
        DryRunPlanner {
            pipeline,
            effect_for,
            max_steps,
        }
    }

    pub fn plan(&self, brain: &dyn AgentRuntime, script: &MockScript) -> Vec<ProposedEffect> {
        proposed_effect_sequence(brain, script, &self.effect_for, self.max_steps)
    }

    pub fn plan_with_verdicts(
        &self,
        brain: &dyn AgentRuntime,
        script: &MockScript,
    ) -> Vec<DryRunEntry> {
        let effects = self.plan(brain, script);
        effects
            .into_iter()
            .map(|effect| {
                let verdict = match crate::effect_api::decode_proposed(&effect) {
                    Ok(plan) => self.pipeline.plan_through_gate(&plan),
                    Err(reason) => PlanVerdict::WouldDeny(
                        crate::effect_api::PipelineStep::Schema,
                        format!("malformed proposed effect: {reason}"),
                    ),
                };
                DryRunEntry { effect, verdict }
            })
            .collect()
    }
}

pub struct DryRunBridge<'p, 'a, S, C, D, T, A, B, F>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
    F: Fn(&ToolName) -> Option<PlannedEffect>,
{
    planner: DryRunPlanner<'p, 'a, S, C, D, T, A, B, F>,
    brain: Box<dyn AgentRuntime + 'p>,
    script: MockScript,
}

impl<'p, 'a, S, C, D, T, A, B, F> DryRunBridge<'p, 'a, S, C, D, T, A, B, F>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
    F: Fn(&ToolName) -> Option<PlannedEffect>,
{
    pub fn new(
        planner: DryRunPlanner<'p, 'a, S, C, D, T, A, B, F>,
        brain: Box<dyn AgentRuntime + 'p>,
        script: MockScript,
    ) -> Self {
        DryRunBridge {
            planner,
            brain,
            script,
        }
    }
}

impl<S, C, D, T, A, B, F> DryRun for DryRunBridge<'_, '_, S, C, D, T, A, B, F>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
    F: Fn(&ToolName) -> Option<PlannedEffect>,
{
    fn dry_run(&self, _inbox: InboxEvent) -> Vec<ProposedEffect> {
        self.planner.plan(self.brain.as_ref(), &self.script)
    }
}

pub fn dry_run_plan<S, C, D, T, A, B, F>(
    pipeline: &PlanThenApply<'_, S, C, D, T, A, B>,
    script: &MockScript,
    effect_for: F,
) -> Vec<ProposedEffect>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
    F: Fn(&ToolName) -> Option<PlannedEffect>,
{
    let brain = crate::mock::MockAgentRuntime::new(script.clone());
    let planner = DryRunPlanner::new(pipeline, effect_for, MOCK_MAX_STEPS);
    planner.plan(&brain, script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect_api::{ApplyError, EffectCost, PipelineSignals, PipelineStep};
    use crate::mock::MockAgentRuntime;
    use myelin_agent::{
        BudgetView, EffectKind, EventId, StepOutcome, Submission, SystemContext, ToolCall,
        ToolCallId, ToolDef, ToolSchema,
    };
    use myelin_identity::{
        CaveatContext, Consistency, Decision, EffectivePolicy, Permission, Principal, PrincipalId,
        PrincipalKind, RuntimeRef, Zookie,
    };
    use myelin_storage::reserve_settle::MeteredUnit;
    use myelin_tenancy::{ArtifactRef, TenantId};
    use std::collections::BTreeSet;

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

    struct Checker {
        allow: BTreeSet<String>,
    }
    impl CapabilityCheck for Checker {
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            _o: &ArtifactRef,
            _at: &Consistency,
            _caveat: Option<&CaveatContext>,
        ) -> Decision {
            if self.allow.contains(&permission.0) {
                Decision::Allow
            } else {
                Decision::Deny
            }
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

    struct Tenant;
    impl TenantGuard for Tenant {
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
            panic!("a dry-run must NEVER call the subsystem PUBLIC endpoint (step 7)");
        }
    }

    struct ReadOnlyBudget {
        remaining: u64,
    }
    impl EffectBudget for ReadOnlyBudget {
        fn has_remaining(&self, cost: u64) -> bool {
            self.remaining >= cost
        }
        fn settle_one(&mut self, _unit: &MeteredUnit) -> u64 {
            panic!("a dry-run must NEVER meter (step 8 settle_one)");
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

    fn tool_def(subsystem: &str, name: &str, caps: &[&str], requires_approval: bool) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: subsystem.into(),
            version: 1,
            input_schema:
                r#"{"type":"object","required":["x"],"properties":{"x":{"type":"string"}}}"#.into(),
            required_caps: caps.iter().map(|c| c.to_string()).collect(),
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval,
            exposed_over_mcp: false,
        }
    }

    fn planned(tool: &str, unit: &'static str) -> PlannedEffect {
        PlannedEffect {
            tool: ToolName(tool.into()),
            object: ArtifactRef(format!("myelin://acme/x/{tool}")),
            input_json: r#"{"x":"v"}"#.into(),
            field: None,
            transition: None,
            cost: EffectCost {
                unit,
                wholesale: 3,
                markup: 1,
            },
        }
    }

    fn merge_then_post_script() -> MockScript {
        MockScript::new(
            SystemContext("you are agent-7".into()),
            vec![schema("git.merge"), schema("chat.post_message")],
            BudgetView(100),
            vec![
                StepOutcome::UseTools(vec![call("git.merge")]),
                StepOutcome::UseTools(vec![call("chat.post_message")]),
                StepOutcome::Submit(Submission("done".into())),
            ],
        )
    }

    fn effect_for(name: &ToolName) -> Option<PlannedEffect> {
        match name.0.as_str() {
            "git.merge" => Some(planned("git.merge", "git.merge")),
            "chat.post_message" => Some(planned("chat.post_message", "agent.effect")),
            _ => None,
        }
    }

    fn catalogue() -> Catalogue {
        Catalogue {
            defs: vec![
                tool_def("git", "git.merge", &["git.merge"], true),
                tool_def("chat", "chat.post_message", &["chat.post"], false),
            ],
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pipeline<'a>(
        cat: &'a Catalogue,
        check: &'a Checker,
        del: &'a Delegator,
        tenant: &'a Tenant,
        endpoint: &'a NeverApply,
        budget: &'a mut ReadOnlyBudget,
        signals: &'a mut PipelineSignals,
    ) -> PlanThenApply<'a, Catalogue, Checker, Delegator, Tenant, NeverApply, ReadOnlyBudget> {
        PlanThenApply {
            catalogue: cat,
            check,
            delegation: del,
            tenant,
            apply_endpoint: endpoint,
            budget,
            agent: agent(),
            trigger_actor: human(),
            zookie: Zookie("z-1".into()),
            approved: BTreeSet::new(),
            signals,
        }
    }

    #[test]
    fn dry_run_returns_the_plan_with_zero_applies_and_zero_meter() {
        let cat = catalogue();
        let check = Checker {
            allow: ["git.merge".to_string(), "chat.post".to_string()]
                .into_iter()
                .collect(),
        };
        let del = Delegator {
            policy: vec!["git.merge".into(), "chat.post".into()],
        };
        let tenant = Tenant;
        let endpoint = NeverApply;
        let mut budget = ReadOnlyBudget { remaining: 100 };
        let mut signals = PipelineSignals::new();
        let p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            &mut signals,
        );

        let script = merge_then_post_script();
        let brain = MockAgentRuntime::new(script.clone());
        let planner = DryRunPlanner::new(&p, effect_for, MOCK_MAX_STEPS);

        let plan = planner.plan(&brain, &script);
        assert_eq!(plan.len(), 2, "the run proposed two effects (merge, post)");
        assert_eq!(
            plan[0],
            crate::effect_api::encode_proposed(&planned("git.merge", "git.merge"))
        );
        assert_eq!(
            plan[1],
            crate::effect_api::encode_proposed(&planned("chat.post_message", "agent.effect"))
        );

        let entries = planner.plan_with_verdicts(&brain, &script);
        assert!(
            matches!(entries[0].verdict, PlanVerdict::WouldGate(_)),
            "merge WOULD gate (AG-8)"
        );
        assert_eq!(
            entries[1].verdict,
            PlanVerdict::WouldApply,
            "post WOULD apply"
        );

        assert_eq!(
            budget.remaining, 100,
            "the reserve balance is UNCHANGED after a dry-run"
        );
        assert_eq!(signals.applied(), 0, "0 applies");
        assert_eq!(signals.metered_total(), 0, "0 metered effects");
        assert_eq!(signals.privileged_fallback(), 0);
    }

    #[test]
    fn ag_d9_proposed_effect_sequence_is_byte_identical_across_two_runs() {
        let script = merge_then_post_script();
        let brain = MockAgentRuntime::new(script.clone());

        let first = proposed_effect_sequence(&brain, &script, &effect_for, MOCK_MAX_STEPS);
        let second = proposed_effect_sequence(&brain, &script, &effect_for, MOCK_MAX_STEPS);
        assert_eq!(
            first, second,
            "AG-D9: two runs produce byte-identical proposed-effect sequences"
        );

        assert_eq!(first.len(), 2);
        assert_eq!(
            first[0],
            crate::effect_api::encode_proposed(&planned("git.merge", "git.merge"))
        );
        assert_eq!(
            first[1],
            crate::effect_api::encode_proposed(&planned("chat.post_message", "agent.effect"))
        );

        assert_eq!(first[0].0, second[0].0, "the carrier bytes are identical");
    }

    #[test]
    fn read_and_compute_calls_are_not_proposed_effects() {
        let script = MockScript::new(
            SystemContext("s".into()),
            vec![schema("search"), schema("git.merge")],
            BudgetView(0),
            vec![
                StepOutcome::UseTools(vec![call("search")]),
                StepOutcome::UseTools(vec![call("git.merge")]),
                StepOutcome::Submit(Submission("done".into())),
            ],
        );
        let brain = MockAgentRuntime::new(script.clone());
        let seq = proposed_effect_sequence(&brain, &script, &effect_for, MOCK_MAX_STEPS);
        assert_eq!(
            seq.len(),
            1,
            "only the mutate call (git.merge) is a proposed effect"
        );
        assert_eq!(
            seq[0],
            crate::effect_api::encode_proposed(&planned("git.merge", "git.merge"))
        );
    }

    #[test]
    fn dry_run_surfaces_would_deny_without_mutating() {
        let cat = catalogue();
        let check = Checker {
            allow: ["git.merge".to_string()].into_iter().collect(),
        };
        let del = Delegator {
            policy: vec!["chat.post".into()],
        };
        let tenant = Tenant;
        let endpoint = NeverApply;
        let mut budget = ReadOnlyBudget { remaining: 100 };
        let mut signals = PipelineSignals::new();
        let p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            &mut signals,
        );

        let script = MockScript::new(
            SystemContext("s".into()),
            vec![schema("git.merge")],
            BudgetView(0),
            vec![
                StepOutcome::UseTools(vec![call("git.merge")]),
                StepOutcome::Submit(Submission("done".into())),
            ],
        );
        let brain = MockAgentRuntime::new(script.clone());
        let planner = DryRunPlanner::new(&p, effect_for, MOCK_MAX_STEPS);

        let entries = planner.plan_with_verdicts(&brain, &script);
        assert_eq!(entries.len(), 1);
        match &entries[0].verdict {
            PlanVerdict::WouldDeny(PipelineStep::Delegation, reason) => {
                assert!(
                    reason.contains("intersection"),
                    "the deny verdict names the ∩: {reason}"
                )
            }
            other => panic!("expected a would-deny at delegation, got {other:?}"),
        }
        assert_eq!(
            budget.remaining, 100,
            "a would-deny dry-run still mutates/meters NOTHING"
        );
    }

    #[test]
    fn dry_run_plan_convenience_returns_the_plan() {
        let cat = catalogue();
        let check = Checker {
            allow: ["git.merge".to_string(), "chat.post".to_string()]
                .into_iter()
                .collect(),
        };
        let del = Delegator {
            policy: vec!["git.merge".into(), "chat.post".into()],
        };
        let tenant = Tenant;
        let endpoint = NeverApply;
        let mut budget = ReadOnlyBudget { remaining: 100 };
        let mut signals = PipelineSignals::new();
        let p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            &mut signals,
        );

        let script = merge_then_post_script();
        let plan = dry_run_plan(&p, &script, effect_for);
        assert_eq!(plan.len(), 2);
        assert_eq!(
            budget.remaining, 100,
            "the convenience is side-effect-free too"
        );
    }
}
