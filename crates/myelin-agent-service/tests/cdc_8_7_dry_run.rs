//! # The provider CDC for contract 8.7 (`run --dry-run`) + the AG-D9 effect-sequence determinism
//! golden (AG-P8 → P-220)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.7
//! (`run --dry-run`: `dry_run(InboxEvent) → Vec<ProposedEffect>` — plan-then-apply testability, NO
//! apply). Owning architecture: `agent-fabric.md` §7.1 / §5.2 (the first SIX steps run
//! side-effect-free). AG-P1 (→ P-130) froze the SIGNATURE (`myelin-agent` `DryRun` trait); THIS pair
//! pins the BODY AG-P8 owns + the AG-D9 effect-sequence determinism (two runs byte-identical).
//!
//! The PROVIDER is the dry-run lever ([`DryRunPlanner`] / the [`DryRun`] bridge); the CONSUMER is the
//! CLI `run --dry-run` / an E2E that plans a run and asserts the wallet is unchanged (0 apply, 0 meter).

use myelin_agent::{
    BudgetView, DryRun, EffectKind, EventId, InboxEvent, ProposedEffect, StepOutcome, Submission,
    SystemContext, ToolCall, ToolDef, ToolName, ToolSchema, ToolSurface,
};
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

/// The subsystem endpoint PANICS if called — proving the dry-run never reaches step 7 (the mutation).
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

/// The budget's `settle_one` PANICS if called — proving the dry-run never meters (step 8). The
/// remaining balance is the wallet the E2E asserts unchanged.
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
        vec![ToolSchema("merge".into()), ToolSchema("post".into())],
        BudgetView(1000),
        vec![
            StepOutcome::UseTools(vec![ToolCall(ToolName("merge".into()))]),
            StepOutcome::UseTools(vec![ToolCall(ToolName("post".into()))]),
            StepOutcome::Submit(Submission("done".into())),
        ],
    )
}

/// **PROVIDER+CONSUMER CDC for 8.7 — the dry-run returns the full proposed-effect plan with zero
/// applies and zero metered effects; the wallet balance is UNCHANGED.** The CONSUMER is the CLI/E2E
/// asserting the plan and the unchanged balance; the PROVIDER is the side-effect-free steps-1..6 gate.
#[test]
fn cdc_8_7_dry_run_returns_the_plan_with_unchanged_wallet() {
    let cat = Catalogue {
        defs: vec![
            tool_def("merge", &["merge"], /* gated */ true),
            tool_def("post", &["post"], /* gated */ false),
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
    // Scope the pipeline + planner so their borrow of `&mut budget` / `&mut signals` ENDS before the
    // wallet/signals assertions below (the dry-run is observational — nothing it borrows is mutated).
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

    // the PLAN — two proposed effects (merge, post), in order.
    assert_eq!(
        plan.len(),
        2,
        "the dry-run plan holds both proposed effects"
    );
    assert_eq!(plan[0], encode_proposed(&planned("merge")));
    assert_eq!(plan[1], encode_proposed(&planned("post")));

    // the per-effect verdicts — merge WOULD gate (requires_approval), post WOULD apply.
    assert!(
        matches!(entries[0].verdict, PlanVerdict::WouldGate(_)),
        "merge gates (AG-8)"
    );
    assert_eq!(
        entries[1].verdict,
        PlanVerdict::WouldApply,
        "post would apply"
    );

    // the GATE: 0 applies, 0 meter, the WALLET BALANCE IS UNCHANGED.
    assert_eq!(
        budget.remaining, 1000,
        "the reserve balance is unchanged after a dry-run"
    );
    assert_eq!(signals.applied(), 0, "0 applies");
    assert_eq!(signals.metered_total(), 0, "0 metered effects");
}

/// **CONSUMER CDC for the frozen glue `DryRun::dry_run(InboxEvent) → Vec<ProposedEffect>` body.** The
/// bridge satisfies the frozen 8.7 signature; the CONSUMER (the CLI) hands an `InboxEvent` and gets
/// the plan — side-effect-free.
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
    // Scope the pipeline + bridge so the `&mut budget` borrow ends before the wallet assertion.
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
        // the frozen glue entry: dry_run(InboxEvent) → Vec<ProposedEffect>.
        bridge.dry_run(InboxEvent("issue.created".into()))
    }; // the bridge (holding &p, holding &mut budget) drops here, releasing the borrow.
    assert_eq!(plan.len(), 2, "the frozen DryRun body returns the plan");
    assert_eq!(
        budget.remaining, 1000,
        "side-effect-free through the frozen trait too"
    );
}

/// **AG-D9 (the effect-sequence half, re-asserted) — running the SAME script TWICE through the
/// dry-run produces BYTE-IDENTICAL proposed-effect sequences.** This completes the AG-D9 determinism
/// the step-sequence half (AG-P5) greened: now the PROPOSED-EFFECT sequence is byte-identical.
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
    // literal byte-identity of the carriers.
    assert_eq!(first[0].0, second[0].0);
    assert_eq!(first[1].0, second[1].0);
}
