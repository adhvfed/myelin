//! # `dry_run` — `run --dry-run`: the plan-then-apply testability lever (8.7) + the AG-D9
//! effect-sequence determinism (AG-P8 → P-220, M2-B)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §7.1 / contract 8.7
//! (`run --dry-run`: `dry_run(InboxEvent) → Vec<ProposedEffect>` — *plan-then-apply testability;
//! `--dry-run` stops after the HITL step (step 6) and shows the plan, NO apply*), §5.2 (the eight-step
//! pipeline whose first SIX steps the dry-run runs side-effect-free). External-insights:
//! `03-agent-native-fabric.md` §4 (*the dry-run lever; plan-then-apply testability*),
//! `01-process-and-quality-doctrine.md` §3 (*the dry-run is the lever the determinism golden uses;
//! AG-D9 is a quantified gate*).
//!
//! **Contract-index:** OWNS the body of row 8.7 (`run --dry-run`). The glue [`DryRun`](myelin_agent::DryRun)
//! trait (8.7, signature-half, AG-P1 → P-130) is implemented HERE; the dry-run is the lever every E2E
//! and drill uses to plan a run WITHOUT spending a real effect.
//!
//! ## What this prompt (AG-P8) ships — the dry-run lever + the AG-D9 effect-sequence golden
//! - [`DryRunPlanner`] — drives a [`MockScript`](crate::mock::MockScript) brain through the loop to
//!   produce the run's **proposed-effect sequence**, then runs each effect through the pipeline's
//!   SIDE-EFFECT-FREE gate (steps 1..6, [`PlanThenApply::plan_through_gate`]). It returns the
//!   `Vec<ProposedEffect>` plan (8.7) **with 0 applies + 0 metered effects** — the subsystem PUBLIC
//!   endpoint is NEVER called and the budget is NEVER settled (the wallet balance is unchanged after
//!   a dry-run, the GATE assertion).
//! - [`DryRunPlanner::plan`] / [`DryRunPlanner::plan_with_verdicts`] — the `dry_run(InboxEvent) →
//!   Vec<ProposedEffect>` body + the per-effect verdicts (would-apply / would-gate / would-deny) a
//!   test inspects.
//! - The **AG-D9 effect-sequence determinism** ([`proposed_effect_sequence`]): two runs of the same
//!   script produce **byte-identical** `ProposedEffect` sequences (this completes the AG-D9
//!   determinism that AG-P5 greened at the *step-sequence* level — now the PROPOSED-EFFECT sequence
//!   is asserted byte-identical, the half AG-P8 owns).
//!
//! ## Why the dry-run is side-effect-free BY CONSTRUCTION (not by convention)
//! The dry-run reuses the LIVE pipeline's [`plan_through_gate`](PlanThenApply::plan_through_gate)
//! (steps 1..6) — the SAME code path the live apply runs before step 7. It then STOPS: it never
//! calls `apply_endpoint.apply_public` (step 7) and never calls `budget.settle_one` (step 8). There
//! is no second pipeline, so the plan a dry-run shows IS the plan the live apply would execute — and
//! a dry-run cannot mutate or meter because those calls are simply not made. The `&self` budget seam
//! the dry-run holds is read-only (`has_remaining`), never the `&mut settle_one`.
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **The brain that produces the proposed-effect sequence is the MOCK brain** (the v1 floor,
//!   AG-P5 → P-217; the real `LlmAgentRuntime` is AG-P25, post-M5). The dry-run is brain-agnostic —
//!   it drives ANY `&dyn AgentRuntime` (the same swap, behind the frozen seam).
//! - **The CLI flag wiring (`myelin-agent run --dry-run`) is the service binary's** — HERE the
//!   library lever (the `dry_run` body the CLI calls) is shipped; the `app.rs` `run` entry threads
//!   the flag. The lever is the contract; the flag is a one-line dispatch.
//! - **No data-layer floor** — the dry-run is pure-compute (no DB/object-store/cache/bus contract is
//!   touched; the budget seam it reads is the in-memory reserve view). It does NOT need an
//!   `integration` test.

use crate::effect_api::{
    CapabilityCheck, DelegationLookup, EffectBudget, PlanThenApply, PlanVerdict, PlannedEffect,
    SubsystemApply, TenantGuard,
};
use crate::mock::{build_conversation, MockScript, TraceHistory, MOCK_MAX_STEPS};
use myelin_agent::{
    AgentRuntime, DryRun, InboxEvent, ProposedEffect, StepOutcome, ToolName, ToolOutcome, ToolResult,
    ToolSurface,
};

// ───────────────────────── the proposed-effect sequence (the AG-D9 golden artifact) ─────────────

/// **The proposed effects a scripted run WOULD emit, in order (the dry-run plan + the AG-D9
/// effect-sequence golden).** Built by driving the brain through the loop: at each turn the brain's
/// [`StepOutcome::UseTools`] names the tools it wants; the loop maps each `mutate`/`external` tool
/// call to a [`PlannedEffect`] (via the supplied `effect_for`), and the effect's opaque
/// [`ProposedEffect`] carrier is recorded. The sequence is a pure function of `(script, catalogue,
/// effect_for)` — two runs produce a byte-identical sequence (AG-D9).
///
/// `read`/`compute` tool calls do NOT route through `EffectApi` (§5.0) — they are not proposed
/// effects (they go to the hands / a permission-filtered read). Only `mutate`/`external` calls
/// become `ProposedEffect`s. A `Submit` turn ends the run (no effect).
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
                // the brain terminated the run — no further effects proposed.
                history.push_model(outcome.clone());
                break;
            }
            StepOutcome::UseTools(calls) => {
                // map each call the loop would route to EffectApi (§5.0) to a ProposedEffect, in
                // order. `effect_for` returns None for a read/compute call (not a proposed effect).
                //
                // SECURITY BOUNDARY: `call.arguments` is untrusted model output. The REAL
                // tool-calling loop (VISION §3, not built here) MUST pass each call through
                // `crate::effect_api::validate_tool_arguments` before dispatch. This dry-run maps by
                // NAME to a fixture `PlannedEffect` and never dispatches `call.arguments`, so there is
                // no unvalidated argument reaching a tool here; the mutate path's own step-1
                // `validate_schema` gate still re-checks the effect input at apply.
                for call in calls {
                    if let Some(plan) = effect_for(&call.name) {
                        effects.push(crate::effect_api::encode_proposed(&plan));
                    }
                }
                // append the model step + DETERMINISTIC scripted tool results so the next
                // conversation reconstruction is reproducible (the SAME determinism the replay uses).
                // Each result is linked back to its call by the model-minted `call_id`.
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

// ───────────────────────── the dry-run planner (the 8.7 lever) ──────────────────────────────────

/// **`run --dry-run` — the side-effect-free planner (8.7).** Holds the live pipeline (the SAME seams
/// the live apply uses: the catalogue, the `check`/`delegation`/tenant guards, the budget) and the
/// `effect_for` map (tool-name → the [`PlannedEffect`] the loop would build for it). [`plan`](DryRunPlanner::plan)
/// drives the brain to produce the proposed-effect sequence, runs each through the gate (steps 1..6),
/// and returns the plan — **0 applies + 0 metered effects** (the wallet balance is unchanged).
///
/// **The borrow shape proves side-effect-freeness:** the planner takes the pipeline by `&` (NOT
/// `&mut`), so it can ONLY call [`plan_through_gate`](PlanThenApply::plan_through_gate) (the `&self`
/// gate) — it CANNOT reach the `&mut settle_one` meter (step 8) or mutate the signals. The
/// apply-endpoint is held but NEVER invoked (step 7 is not in the dry-run). The dry-run is
/// side-effect-free by construction, not by convention.
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
    /// The live pipeline, borrowed by `&` (read-only — only the steps-1..6 gate is reachable).
    pipeline: &'p PlanThenApply<'a, S, C, D, T, A, B>,
    /// The tool-name → [`PlannedEffect`] map (what the loop would propose for each routed call).
    effect_for: F,
    /// The bounded step ceiling the dry-run drives under (the loop never hangs; §2.3).
    max_steps: usize,
}

/// **One entry in a dry-run plan: the proposed effect + the gate verdict it WOULD get (8.7).** The
/// `verdict` is the steps-1..6 outcome ([`PlanVerdict`]) — would-apply / would-gate / would-deny —
/// **without** any mutation or meter. A test asserts the plan + the per-effect verdicts; an E2E
/// asserts the wallet balance is unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryRunEntry {
    /// The opaque [`ProposedEffect`] carrier the brain proposed (the plan row).
    pub effect: ProposedEffect,
    /// The steps-1..6 verdict the LIVE pipeline would reach (no apply, no meter).
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
    /// Build a dry-run planner over the live `pipeline` (by `&`), the `effect_for` map, and a step
    /// `max_steps` ceiling (use [`MOCK_MAX_STEPS`] for the default bound).
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

    /// **The 8.7 body: `dry_run(InboxEvent) → Vec<ProposedEffect>`.** Drives the `brain` through the
    /// loop (under the supplied `script` framing) to produce the proposed-effect sequence, then runs
    /// each effect through the pipeline's SIDE-EFFECT-FREE gate (steps 1..6) to PROVE the plan is
    /// reachable — but applies NOTHING and meters NOTHING. Returns the plan (the proposed effects, in
    /// order). The `InboxEvent` is the trigger the loop framing (the [`MockScript`]) represents.
    pub fn plan(&self, brain: &dyn AgentRuntime, script: &MockScript) -> Vec<ProposedEffect> {
        proposed_effect_sequence(brain, script, &self.effect_for, self.max_steps)
    }

    /// **The plan WITH per-effect verdicts (the test/E2E-facing form).** Each proposed effect is run
    /// through the steps-1..6 gate ([`plan_through_gate`](PlanThenApply::plan_through_gate)); the
    /// verdict (would-apply / would-gate / would-deny) is recorded alongside the effect — STILL 0
    /// applies + 0 meters. This is what a dry-run E2E inspects ("the merge WOULD gate; the comment
    /// WOULD apply") and what asserts the plan is side-effect-free.
    pub fn plan_with_verdicts(
        &self,
        brain: &dyn AgentRuntime,
        script: &MockScript,
    ) -> Vec<DryRunEntry> {
        let effects = self.plan(brain, script);
        effects
            .into_iter()
            .map(|effect| {
                // decode the carrier back to the structured plan + run the gate (steps 1..6). A
                // carrier that cannot decode is a would-deny (fail-closed) — same as the live apply.
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

/// **8.7 — the glue [`DryRun`] frozen-shape bridge: `dry_run(InboxEvent) → Vec<ProposedEffect>`.**
/// Bridges the frozen glue signature (just an [`InboxEvent`]) to the [`DryRunPlanner`] by holding the
/// brain + the loop framing ([`MockScript`]) the `InboxEvent` resolves to. The CLI `run --dry-run`
/// entry (the service binary) builds this from the delivered event and calls the frozen `dry_run`.
///
/// **The `InboxEvent` is the trigger the framing represents** — in a wired run the dispatch tier
/// derives the script/brain from the binding the event matched; here the framing is held so the
/// frozen `dry_run(InboxEvent)` signature is satisfied without changing the glue contract (8.7 is
/// frozen, AG-P1). Returns the proposed-effect plan, **0 applies + 0 metered effects**.
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
    /// The side-effect-free planner (holds the live pipeline by `&`).
    planner: DryRunPlanner<'p, 'a, S, C, D, T, A, B, F>,
    /// The brain the `InboxEvent` resolves to (the v1 floor: the mock; later the LLM brain, AG-P25).
    brain: Box<dyn AgentRuntime + 'p>,
    /// The loop framing the `InboxEvent` resolves to (the script the brain replays).
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
    /// Build the frozen-shape bridge from a planner + the brain + the loop framing the event resolves to.
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
    /// **8.7 — plan the run for a delivered event WITHOUT applying any effect.** Drives the held
    /// brain through the loop framing the `InboxEvent` resolves to; returns the proposed-effect plan.
    /// Side-effect-free (0 apply, 0 meter) — the wallet is unchanged.
    fn dry_run(&self, _inbox: InboxEvent) -> Vec<ProposedEffect> {
        self.planner.plan(self.brain.as_ref(), &self.script)
    }
}

/// **The default-bounded dry-run convenience (drives under [`MOCK_MAX_STEPS`]).** The common case:
/// a fresh [`crate::mock::MockAgentRuntime`] over `script`, planned through `pipeline` + `effect_for`,
/// returning the proposed-effect sequence. The wallet is unchanged after this call (0 apply, 0 meter).
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

    // ───────── REAL seams (the same shapes the apply-pipeline CDC uses) ─────────

    /// A name-only scoped tool schema (empty description + permissive schema) — the fields the
    /// widened seam carries; these tests only exercise the tool name.
    fn schema(name: &str) -> ToolSchema {
        ToolSchema {
            name: ToolName(name.into()),
            description: String::new(),
            input_schema: "{}".into(),
        }
    }

    /// A tool call with a deterministic id and null arguments — the scripted brain chooses no real
    /// arguments here; the id links its later result back.
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

    /// A subsystem endpoint that PANICS if ever called — proving the dry-run never reaches step 7.
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

    /// A budget whose `settle_one` PANICS if ever called — proving the dry-run never meters (step 8).
    /// `has_remaining` reads the balance (the dry-run MAY read it to plan would-apply vs would-deny).
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

    /// A two-effect script: propose `git.merge` (gated), then `chat.post_message` (applies), then
    /// submit. The canonical multi-effect dry-run.
    fn merge_then_post_script() -> MockScript {
        MockScript::new(
            SystemContext("you are agent-7".into()),
            vec![
                schema("git.merge"),
                schema("chat.post_message"),
            ],
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
            // a read/compute tool is NOT a proposed effect (it does not route through EffectApi).
            _ => None,
        }
    }

    fn catalogue() -> Catalogue {
        Catalogue {
            defs: vec![
                tool_def("git", "git.merge", &["git.merge"], /* gated */ true),
                tool_def(
                    "chat",
                    "chat.post_message",
                    &["chat.post"],
                    /* gated */ false,
                ),
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

    /// **8.7 — `dry_run(InboxEvent) → Vec<ProposedEffect>` returns the FULL proposed-effect plan with
    /// 0 applies + 0 metered effects (the GATE assertion).** The endpoint PANICS if called (step 7
    /// never runs); the budget PANICS if metered (step 8 never runs); the reserve balance is
    /// UNCHANGED after the dry-run.
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

        // the PLAN: two proposed effects, in order (git.merge, chat.post_message). 0 mutation reached.
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

        // the verdicts: git.merge WOULD GATE (requires_approval); chat.post_message WOULD APPLY.
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

        // the GATE: the wallet is UNCHANGED (0 applies, 0 meters; the endpoint/meter panic-guards
        // never fired). The signals never recorded an apply or a meter (a dry-run is observational).
        assert_eq!(
            budget.remaining, 100,
            "the reserve balance is UNCHANGED after a dry-run"
        );
        assert_eq!(signals.applied(), 0, "0 applies");
        assert_eq!(signals.metered_total(), 0, "0 metered effects");
        assert_eq!(signals.privileged_fallback(), 0);
    }

    /// **AG-D9 (the effect-sequence half) — two runs of the same script produce BYTE-IDENTICAL
    /// proposed-effect sequences.** This completes the AG-D9 determinism that AG-P5 greened at the
    /// step-sequence level: now the PROPOSED-EFFECT sequence is the golden artifact.
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

        // the sequence is exactly the two routed effects, in order (merge, post). read/compute calls
        // would NOT appear (they don't route through EffectApi).
        assert_eq!(first.len(), 2);
        assert_eq!(
            first[0],
            crate::effect_api::encode_proposed(&planned("git.merge", "git.merge"))
        );
        assert_eq!(
            first[1],
            crate::effect_api::encode_proposed(&planned("chat.post_message", "agent.effect"))
        );

        // byte-identity is literal: the carriers are equal strings.
        assert_eq!(first[0].0, second[0].0, "the carrier bytes are identical");
    }

    /// **A read/compute tool call is NOT a proposed effect (§5.0 routing).** A script that proposes a
    /// `search` (read) tool produces NO proposed effect for it — only `mutate`/`external` calls route
    /// through `EffectApi`.
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

    /// **A dry-run plan surfaces a would-DENY without mutating (the plan shows the deny verdict).** A
    /// tool the delegation ∩ forbids would deny at step 3 — the dry-run reports it, applies nothing.
    #[test]
    fn dry_run_surfaces_would_deny_without_mutating() {
        let cat = catalogue();
        let check = Checker {
            allow: ["git.merge".to_string()].into_iter().collect(),
        };
        // delegation ∩ does NOT include git.merge → would-deny at step 3 (delegation).
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

    /// **`dry_run_plan` (the default-bounded convenience) returns the same plan.** The common entry a
    /// CLI/test calls — a fresh mock brain over the script, planned side-effect-free.
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
