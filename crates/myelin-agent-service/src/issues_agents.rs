//! # `issues_agents` — the FULL Issues ToolDef catalogue + the MOCK forecast / triage agents
//! (ISS-P23 → P-390, M4-I6)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §8 (**the complete Issues `ToolDef` catalogue** — `create`/`update`/`transition`/`comment`/`link`/
//! `estimate`/`reorder`/`assign`/`close` + the agent tools `forecast`/`triage`/`sla_draft`; each
//! declares `required_caps`, `effect_kind`, `side_effecting`, `requires_approval`, `exposed_over_mcp`;
//! **all side-effecting tools apply via `EffectApi::apply` — schema → capability → delegation →
//! tenant → budget → HITL gate → apply via the PUBLIC endpoint, NO carve-out → meter**; a withheld
//! gated tool does NOT mutate, AG-8), §9 (reserve/settle on every spend-bearing agent run — ISS-P24).
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §6.1 (the ONE catalogue),
//! §6.3 (the FROZEN `requires_approval` defaults), §3.2 (the **MockAgentRuntime** — the v1 floor),
//! §7.1 (`run --dry-run` — the triage agent's S9 suggestion strip).
//!
//! **VISION §3** (agent-native from the ground up; the MOCK runtime is the v1 floor — the real
//! `LlmAgentRuntime` is the post-M5 config/impl swap behind the frozen [`AgentRuntime`] seam, NEVER
//! a rewrite). **EI-03 §3/§4** (prove the WHOLE agent story on a mock brain first — deterministic,
//! zero-cost, on the SAME `--use-mock` code path; every tool is a PROJECTION of the existing
//! plan-then-apply path — NO new engine). **EI-01 §7** (the compounding payoff — this is the SAME
//! registration shape as the Git / Knowledge / Chat / CI surfaces; no new machinery).
//!
//! **Contract-index:** CONSUMES **8.1** (register the FULL Issues catalogue with the frozen §6.3
//! defaults), **8.2** (EffectApi plan-then-apply — every side-effecting Issues tool routes through
//! [`crate::effect_api::PlanThenApply`], no carve-out), **8.3** (the MOCK runtime — the forecast /
//! triage agents are scripted [`MockAgentRuntime`] brains), **8.7** (`run --dry-run` — the triage
//! agent proposes effects WITHOUT applying), **4.5/4.7** (delegation + the per-run token are the
//! pipeline's, consumed by construction). The four uniform sandbox guarantees (X-6) are INHERITED
//! from the unified runner — Issues re-implements none.
//!
//! ## Reconciliation with AG-P20 (→ P-347) — EXTEND, never duplicate (coherence, EI-01 §7)
//! AG-P20 ([`crate::issues_tools`]) already registered the FOUR agent-facing Issues ToolDefs
//! (`forecast`/`triage`/`sla_draft`/`transition`) and greened ISS-D12 (the governed-transition HITL
//! withhold). This module **does NOT re-define those** — it REUSES
//! [`crate::issues_tools::issues_tool_defs`] for the four agent tools and ADDS the human/CLI CRUD
//! tools (`create`/`update`/`comment`/`link`/`estimate`/`reorder`/`assign`/`close`) so the FULL
//! arch-§8 catalogue is registered into the ONE [`ToolSurface`] (UI=CLI=agent parity, no privileged
//! back-channel). It then ships the MOCK forecast/triage AGENTS (the scripted runtimes AG-P20 left as
//! a floor) and applies AG-D5 (HITL withhold) + AG-D9 (mock determinism) to Issues' agent tools.
//!
//! ## FLOORS named (VISION §3, EI-01 §1)
//! - **Agent runtime = MOCK** ([`crate::mock::MockAgentRuntime`], the v1 floor). The real-LLM runtime
//!   is `LlmAgentRuntime` — the **post-M5 follow-on (AG-P25 / ISS-P32, R-10)**, a config/impl swap
//!   behind the frozen [`AgentRuntime`] seam, never a rewrite. NO model/SDK/prompt/model-name string
//!   appears here (the `no-llm-in-platform` ratchet, contract 1.6).
//! - **Forecast = LINEAR** (`remaining ÷ velocity`, [`LinearForecast`]). The Monte-Carlo forecast
//!   agent is the **follow-on (R-5, ISS-P32)** — it reads the SAME OLAP this linear one does (the
//!   swap is the forecast function, not the tool/runtime seam). Named here per VISION §3.
//! - **Reserve/settle** on every spend-bearing run is **ISS-P24 (→ P-391)** — the BUDGET step of the
//!   pipeline reads the reserve HERE (consumed); the wallet wiring on every agent dispatch is P-391.
//! - **The stateful Trigger** ("remind me when unblocked") is **ISS-P25 (→ P-392)**.
//! - **The external MCP ENDPOINT** (its auth + the agent-lane rate-limit) is the post-M5 follow-on
//!   (AG-P25); the Issues tools are NOT MCP-exposed at v1 (`exposed_over_mcp = false`).

use myelin_agent::{
    BudgetView, EffectKind, ProposedEffect, StepOutcome, Submission, SystemContext, ToolCall,
    ToolDef, ToolName, ToolSchema, ToolSurface,
};
use myelin_issues::rebac_fragment::object_types as issue_objects;

use crate::defaults::{assert_no_silent_loosening, seed_requires_approval, LooseningViolation};
use crate::dry_run::proposed_effect_sequence;
use crate::effect_api::{EffectCost, PlannedEffect};
use crate::issues_tools::{issues_tool_defs, ISSUES_SUBSYSTEM, ISSUES_TOOL_VERSION};
use crate::mock::{MockAgentRuntime, MockScript};

// ───────────────────────── the human/CLI CRUD Issues tool names (arch §8) ────────────────────────

/// The `issues.create` tool name (arch §8 — create an issue; reversible, NOT gated).
pub const CREATE_TOOL: &str = "create";
/// The `issues.update` tool name (arch §8 — edit fields; a governed field is field-ABAC-gated).
pub const UPDATE_TOOL: &str = "update";
/// The `issues.comment` tool name (arch §8 — add a comment; reversible, NOT gated).
pub const COMMENT_TOOL: &str = "comment";
/// The `issues.link` tool name (arch §8 — create a typed relation edge; reversible, NOT gated).
pub const LINK_TOOL: &str = "link";
/// The `issues.estimate` tool name (arch §8 — set an estimate/story-point; reversible, NOT gated).
pub const ESTIMATE_TOOL: &str = "estimate";
/// The `issues.reorder` tool name (arch §8 — rank CAS; the SAME path as a human, NOT gated).
pub const REORDER_TOOL: &str = "reorder";
/// The `issues.assign` tool name (arch §8 — (re)assign; reversible, NOT gated).
pub const ASSIGN_TOOL: &str = "assign";
/// The `issues.close` tool name (arch §8 — close; gated IF confidential or governed, the floor).
pub const CLOSE_TOOL: &str = "close";

// ───────────────────────── the required_caps from the Issues ReBAC fragment (4.9) ────────────────

/// **The `required_caps` for the create tool (4.9).** `issue.create` is governed by the create
/// permission on the parent project (arch §8 table: `issue.create` on project). Built from the
/// canonical Issues object-type constant (a fragment rename breaks this, never a silent drift).
pub fn create_required_caps() -> Vec<String> {
    vec![format!("{}.create", issue_objects::ISSUE)]
}

/// **The `required_caps` for the field-writing tools — update/comment/link/estimate/reorder/assign
/// (4.9).** Arch §8 maps these to `issue.update` / `issue.comment` / `issue.transition`; we use the
/// canonical `issue.update` permission for the field/rank-writing tools (the assignment/transition
/// edge is governed by the transition cap, declared by [`assign_required_caps`]).
pub fn update_required_caps() -> Vec<String> {
    vec![format!("{}.update", issue_objects::ISSUE)]
}

/// **The `required_caps` for `comment` (4.9).** Arch §8: governed by `issue.comment` (the comment
/// permission the frozen fragment declares: `comment = view`).
pub fn comment_required_caps() -> Vec<String> {
    vec![format!("{}.comment", issue_objects::ISSUE)]
}

/// **The `required_caps` for `assign` and `close` (4.9).** Arch §8 maps both to `issue.transition`
/// (assignment/closing crosses a governed edge): `assign`/`close` are governed by the
/// `issue.transition` permission a human holds (an agent may only assign/close where a human could).
pub fn assign_required_caps() -> Vec<String> {
    vec![format!("{}.transition", issue_objects::ISSUE)]
}

// ───────────────────────── the human/CLI CRUD ToolDefs (8.1 — the FULL arch-§8 catalogue) ─────────

/// Build one of the reversible CRUD Issues ToolDefs — a `Mutate` tool that routes through the
/// plan-then-apply pipeline (cap-checked, metered, audited) and whose `requires_approval` is SEEDED
/// from the frozen §6.3 default. The CRUD tools are reversible → NOT gated (suggest-by-default).
fn crud_tool_def(name: &str, caps: Vec<String>, input_schema: &str) -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(name.to_string()),
        subsystem: ISSUES_SUBSYSTEM.to_string(),
        version: ISSUES_TOOL_VERSION,
        input_schema: input_schema.to_string(),
        required_caps: caps,
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 — the value passed here is overwritten by the frozen default.
        requires_approval: false,
        exposed_over_mcp: false,
    })
}

/// **`issues.create` (8.1 / arch §8) — create an issue (reversible, NOT gated).**
pub fn create_tool_def() -> ToolDef {
    crud_tool_def(
        CREATE_TOOL,
        create_required_caps(),
        r#"{"type":"object","required":["project","title"],"properties":{"project":{"type":"string"},"title":{"type":"string"},"type":{"type":"string"}}}"#,
    )
}

/// **`issues.update` (8.1 / arch §8) — edit fields (reversible; a governed field → field-ABAC).**
pub fn update_tool_def() -> ToolDef {
    crud_tool_def(
        UPDATE_TOOL,
        update_required_caps(),
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"fields":{"type":"object"}}}"#,
    )
}

/// **`issues.comment` (8.1 / arch §8) — add a comment (reversible, NOT gated).**
pub fn comment_tool_def() -> ToolDef {
    crud_tool_def(
        COMMENT_TOOL,
        comment_required_caps(),
        r#"{"type":"object","required":["issue","body"],"properties":{"issue":{"type":"string"},"body":{"type":"string"}}}"#,
    )
}

/// **`issues.link` (8.1 / arch §8) — create a typed relation edge (reversible, NOT gated).**
pub fn link_tool_def() -> ToolDef {
    crud_tool_def(
        LINK_TOOL,
        update_required_caps(),
        r#"{"type":"object","required":["issue","target","relation"],"properties":{"issue":{"type":"string"},"target":{"type":"string"},"relation":{"type":"string"}}}"#,
    )
}

/// **`issues.estimate` (8.1 / arch §8) — set an estimate/story-point (reversible, NOT gated).**
pub fn estimate_tool_def() -> ToolDef {
    crud_tool_def(
        ESTIMATE_TOOL,
        update_required_caps(),
        r#"{"type":"object","required":["issue","points"],"properties":{"issue":{"type":"string"},"points":{"type":"number"}}}"#,
    )
}

/// **`issues.reorder` (8.1 / arch §8) — rank CAS (the SAME path as a human, NOT gated).**
pub fn reorder_tool_def() -> ToolDef {
    crud_tool_def(
        REORDER_TOOL,
        update_required_caps(),
        r#"{"type":"object","required":["issue","order_key"],"properties":{"issue":{"type":"string"},"order_key":{"type":"string"}}}"#,
    )
}

/// **`issues.assign` (8.1 / arch §8) — (re)assign (reversible, NOT gated).**
pub fn assign_tool_def() -> ToolDef {
    crud_tool_def(
        ASSIGN_TOOL,
        assign_required_caps(),
        r#"{"type":"object","required":["issue","assignee"],"properties":{"issue":{"type":"string"},"assignee":{"type":"string"}}}"#,
    )
}

/// **`issues.close` (8.1 / arch §8) — close (GATED if confidential or governed; the frozen floor).**
/// The static §6.3 default is `true` (gated) — the conservative floor; a non-confidential,
/// non-governed close is admitted by the confidential/governed ABAC caveat at check-time, but the
/// tool_def default is gated so the floor is never silently loosened (the SAME posture as
/// [`crate::issues_tools::transition_tool_def`]).
pub fn close_tool_def() -> ToolDef {
    crud_tool_def(
        CLOSE_TOOL,
        assign_required_caps(),
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"reason":{"type":"string"}}}"#,
    )
}

// ───────────────────────── the FULL catalogue + the registration seam (8.1) ──────────────────────

/// **The FULL Issues ToolDef catalogue, in arch-§8 order (8.1) — the OWNED ISS-P23 deliverable.**
/// The eight human/CLI CRUD tools (`create`/`update`/`comment`/`link`/`estimate`/`reorder`/`assign`/
/// `close`) FOLLOWED BY the four agent tools (`forecast`/`triage`/`sla_draft`/`transition`, reused
/// VERBATIM from AG-P20's [`issues_tool_defs`] — no duplication). The SINGLE list every registration
/// and CDC consume (one source of truth). Exactly TWO tools are gated: `close` and `transition` (the
/// consequential split — both the conservative floor, refined by their ABAC caveat at check-time).
pub fn full_issues_tool_defs() -> Vec<ToolDef> {
    let mut defs = vec![
        create_tool_def(),
        update_tool_def(),
        comment_tool_def(),
        link_tool_def(),
        estimate_tool_def(),
        reorder_tool_def(),
        assign_tool_def(),
        close_tool_def(),
    ];
    // the four agent tools, reused verbatim from AG-P20 (forecast/triage/sla_draft/transition).
    defs.extend(issues_tool_defs());
    defs
}

/// **Register the FULL Issues catalogue into the ONE [`ToolSurface`] (8.1 / §6.1) — the OWNED
/// deliverable.** Every def is passed through the VISION §3 no-silent-loosening guard FIRST
/// ([`assert_no_silent_loosening`]): a registration that tried to flip a frozen `yes → no` (the
/// `close`/`transition` gates) WITHOUT a written deviation is REJECTED LOUD. The defs are already
/// seeded from the frozen table, so the strict (no-deviation) call always admits them — the guard is
/// the structural proof a future hand-edit can't loosen a gate unnoticed. Identical in shape to
/// [`crate::git_tools::register_git_tools`] / [`crate::issues_tools::register_issues_tools`] (the
/// compounding-payoff reuse — no second governance model).
pub fn register_full_issues_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    let defs = full_issues_tool_defs();
    for def in &defs {
        assert_no_silent_loosening(def, &[])?;
    }
    for def in &defs {
        surface.register_tool(def.clone());
    }
    Ok(defs)
}

// ───────────────────────── the LINEAR forecast (the named R-5 floor) ─────────────────────────────

/// **The linear-forecast input the agent reads off the OLAP store (the compute-only forecast,
/// arch §8 / §1 `rollup.recomputed` feed).** `remaining` work items (or story-points) ÷ `velocity`
/// (the throughput Storage's frozen OLAP aggregate reports,
/// [`myelin_issues::olap_feed::IssueOlapAnalytics::velocity`]) ⇒ the estimated periods to completion.
/// A pure value so the forecast is deterministic + testable WITHOUT a DB (the OLAP read that produces
/// it is the live path; the math is here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForecastInput {
    /// The remaining work (open items / unfinished story-points) the forecast projects to completion.
    pub remaining: u64,
    /// The velocity — the throughput per period (the OLAP `velocity` aggregate, restriction-honouring).
    pub velocity_per_period: u64,
    /// The at-risk threshold (periods) — crossing it emits `initiative.health_changed` (config, §8).
    pub at_risk_threshold_periods: u64,
}

/// **The forecast the agent produces (the advisory suggestion the human accepts; arch §8).** The
/// estimated periods-to-completion (`None` when velocity is 0 — no throughput, no defensible date,
/// NEVER a divide-by-zero or a fabricated date) + whether the forecast crossed the at-risk threshold
/// (the `initiative.health_changed` trigger, §1 — emitted by the live consumer, not here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForecastOutput {
    /// Estimated periods to completion (`ceil(remaining / velocity)`), or `None` if velocity is 0.
    pub periods_to_completion: Option<u64>,
    /// Whether the forecast is at-risk (periods_to_completion crossed the threshold). A `None`
    /// forecast (no velocity) is at-risk by construction (an unbounded date is the worst case).
    pub at_risk: bool,
}

/// **The LINEAR forecast — `ceil(remaining / velocity)` (the named R-5 floor; the Monte-Carlo
/// follow-on is ISS-P32).** Compute-only (no mutation): it reads the OLAP aggregate and produces the
/// advisory [`ForecastOutput`]. Deterministic (a pure function of its input) — two runs over the same
/// input produce the byte-identical forecast (the AG-D9 determinism the agent inherits).
///
/// **Floor (named):** this is the LINEAR forecast. The Monte-Carlo agent (R-5, ISS-P32) replaces THIS
/// function reading the SAME OLAP — the tool/runtime/apply seam is unchanged, only the math swaps.
pub struct LinearForecast;

impl LinearForecast {
    /// Compute the linear forecast from the OLAP-read input. `velocity == 0` ⇒ `None` periods (no
    /// defensible date) + at-risk (the worst case), NEVER a divide-by-zero.
    pub fn forecast(input: &ForecastInput) -> ForecastOutput {
        let periods = if input.velocity_per_period == 0 {
            None
        } else {
            // ceil(remaining / velocity) — a partial period still needs a whole period to finish.
            Some(input.remaining.div_ceil(input.velocity_per_period))
        };
        let at_risk = match periods {
            // a finite forecast is at-risk iff it crosses the configured threshold.
            Some(p) => p > input.at_risk_threshold_periods,
            // no velocity ⇒ no defensible completion date ⇒ at-risk by construction.
            None => true,
        };
        ForecastOutput {
            periods_to_completion: periods,
            at_risk,
        }
    }
}

// ───────────────────────── the MOCK forecast / triage agents (the named R-10 floor) ──────────────

/// **The MOCK forecast agent (8.3 — the scripted, deterministic brain; the v1 floor).** A
/// compute-only agent: it reads the OLAP, computes the [`LinearForecast`], and SUBMITS the advisory
/// forecast (the human accepts the suggestion — `requires_approval = no`, §6.3). The runtime is the
/// MOCK ([`MockAgentRuntime`]) on the `--use-mock` real code path; the real `LlmAgentRuntime` is the
/// post-M5 swap (AG-P25, R-10). The forecast agent runs the SAME [`crate::skeleton::SkeletonAgent`]
/// loop every other agent does — no privileged path.
///
/// This builds the scripted brain whose single terminal `Submit` carries the forecast summary — a
/// compute-only agent makes NO mutation (it suggests; the human's accept becomes an `update`/an
/// `initiative.health_changed` emission through the normal pipeline). Deterministic by construction
/// (the same input ⇒ the same script ⇒ the byte-identical replay, AG-D9).
pub fn mock_forecast_agent(input: &ForecastInput) -> MockAgentRuntime {
    let out = LinearForecast::forecast(input);
    let summary = match out.periods_to_completion {
        Some(p) => format!(
            "forecast(linear): ~{p} period(s) to completion (remaining={}, velocity={}/period); at_risk={}",
            input.remaining, input.velocity_per_period, out.at_risk
        ),
        None => format!(
            "forecast(linear): no defensible date (velocity=0, remaining={}); at_risk=true",
            input.remaining
        ),
    };
    // a compute-only agent: a single terminal Submit carrying the advisory forecast (no tool call,
    // no mutation — the suggestion is the deliverable; the human accepts it).
    MockAgentRuntime::new(MockScript::submit_only(
        "issues.forecast agent (mock, linear; labelled as an agent)",
        summary,
    ))
}

/// **The MOCK triage agent (8.3 + 8.7 — the S9 suggestion strip via `run --dry-run`; the v1 floor).**
/// The triage agent PROPOSES effects (a suggested `triage` + `assign` + `update`) WITHOUT applying
/// them — the S9 suggestion strip a human accepts (arch §8 / §7.1). It is a scripted [`MockScript`]
/// brain whose [`StepOutcome::UseTools`] names the tools it would call; the dispatch tier runs it via
/// [`crate::dry_run::DryRunPlanner`] (steps 1..6, NO apply, NO meter) to produce the proposed-effect
/// strip. Deterministic (the same script ⇒ the byte-identical proposed-effect sequence, AG-D9).
///
/// The advisory `triage` tool is NOT gated (suggest-by-default, §6.3); the dry-run is side-effect-free
/// BY CONSTRUCTION (it never reaches step 7) — so the triage agent makes 0 mutation until the human
/// accepts a specific suggestion (which then applies once through the normal pipeline).
pub fn mock_triage_agent(issue_ref: &str) -> MockAgentRuntime {
    // a one-turn triage: propose the `triage` advisory tool, then submit the suggestion strip. The
    // tool the brain names is the advisory `triage` (mapped to a ProposedEffect by the dispatch tier).
    let script = MockScript::new(
        SystemContext("issues.triage agent (mock; labelled as an agent)".into()),
        vec![ToolSchema(crate::issues_tools::TRIAGE_TOOL.to_string())],
        BudgetView(0),
        vec![
            StepOutcome::UseTools(vec![ToolCall(ToolName(
                crate::issues_tools::TRIAGE_TOOL.to_string(),
            ))]),
            StepOutcome::Submit(Submission(format!(
                "triage(suggestion strip): proposed triage for {issue_ref} (S9 — the human accepts)"
            ))),
        ],
    );
    MockAgentRuntime::new(script)
}

/// **Map a `triage`/`forecast` advisory tool call to a [`PlannedEffect`] for the dry-run planner
/// (the §5.0 routing entry the triage agent's dry-run uses).** The triage agent's dry-run drives the
/// brain and, for each `mutate` tool call, builds the structured [`PlannedEffect`] the pipeline plans
/// (steps 1..6, NO apply). Returns `None` for a tool that does not route through `EffectApi` (a
/// read/compute call is not a proposed effect — §5.0). The cost is a small advisory metered unit
/// (the BUDGET step reads it; the dry-run NEVER settles — ISS-P24 wires the live reserve).
pub fn triage_effect_for(name: &ToolName, issue_ref: &str) -> Option<PlannedEffect> {
    // only the advisory `triage` tool the agent proposes routes through EffectApi here.
    if name.0 == crate::issues_tools::TRIAGE_TOOL {
        Some(PlannedEffect {
            tool: name.clone(),
            object: myelin_tenancy::ArtifactRef(format!("myelin://acme/issue/issue/{issue_ref}")),
            input_json: format!(r#"{{"issue":"{issue_ref}","priority":"high"}}"#),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "issue.transition",
                wholesale: 1,
                markup: 0,
            },
        })
    } else {
        None
    }
}

/// **The triage agent's S9 proposed-effect strip (8.7 — the dry-run plan, side-effect-free).** Drives
/// the [`mock_triage_agent`] brain through the loop and records the proposed effects it WOULD apply
/// (the suggestion strip), via the SAME [`crate::dry_run`] machinery the rest of the fabric uses —
/// NO apply, NO meter (the wallet is untouched). Deterministic: two runs over the same `issue_ref`
/// produce the BYTE-IDENTICAL strip (the AG-D9 effect-sequence determinism applied to Issues).
pub fn triage_suggestion_strip(issue_ref: &str) -> Vec<ProposedEffect> {
    let brain = mock_triage_agent(issue_ref);
    let script = brain.script().clone();
    proposed_effect_sequence(
        &brain,
        &script,
        &|name: &ToolName| triage_effect_for(name, issue_ref),
        crate::mock::MOCK_MAX_STEPS,
    )
}

/// **Replay the forecast agent's decision stream (8.3 — the AG-D9 mock-determinism artifact applied
/// to Issues' forecast agent).** Two replays over the same forecast input produce a BYTE-IDENTICAL
/// [`crate::mock::ReplayRecord`] (the identical-sequence the AG-D9 green artifact is the hash of).
pub fn replay_forecast_agent(input: &ForecastInput) -> crate::mock::ReplayRecord {
    let brain = mock_forecast_agent(input);
    let script = brain.script().clone();
    crate::mock::replay_bounded(&brain, &script, crate::mock::MOCK_MAX_STEPS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::requires_approval_default;

    /// A `ToolSurface` over a fixed catalogue (the §4.2 in-memory registry).
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

    /// **The FULL arch-§8 catalogue is registered: 12 tools (8 CRUD + 4 agent), in order.** The
    /// registration is the whole 8.1 deliverable — a ToolDef is a row in the ONE registry.
    #[test]
    fn full_catalogue_registers_all_twelve_arch_8_tools() {
        let defs = full_issues_tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name.0.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "create",
                "update",
                "comment",
                "link",
                "estimate",
                "reorder",
                "assign",
                "close",
                "forecast",
                "triage",
                "sla_draft",
                "transition",
            ],
            "the FULL arch-§8 Issues catalogue, in order (8 CRUD + 4 agent)"
        );

        let mut cat = Catalogue { defs: vec![] };
        let registered = register_full_issues_tools(&mut cat).expect("seeded defs admit");
        assert_eq!(registered.len(), 12);
        // every registered tool resolves by name with its frozen shape.
        for name in &names {
            assert!(
                cat.resolve(&ToolName(name.to_string())).is_some(),
                "{name} registered into the ONE surface"
            );
        }
        // an un-registered tool resolves to None (the catalogue is exactly these twelve).
        assert!(cat.resolve(&ToolName("delete".into())).is_none());
    }

    /// **Exactly TWO Issues tools are GATED: `close` + `transition` (the consequential split).** Every
    /// other tool is advisory/reversible (suggest-by-default, §6.3). The gating IS the frozen seed —
    /// not hand-set (a drift in the §6.3 table flips this).
    #[test]
    fn exactly_close_and_transition_are_gated_by_the_frozen_default() {
        let defs = full_issues_tool_defs();
        let gated: Vec<&str> = defs
            .iter()
            .filter(|d| d.requires_approval)
            .map(|d| d.name.0.as_str())
            .collect();
        assert_eq!(
            gated,
            vec!["close", "transition"],
            "only close + the SLA-bound transition are gated; the rest are advisory/reversible"
        );
        // the gating of EVERY tool IS the frozen §6.3 seed (no tool is hand-set).
        for d in &defs {
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating IS the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
            // every Issues tool is a Mutate that routes through EffectApi — no new path.
            assert_eq!(d.effect_kind, EffectKind::Mutate);
            assert!(d.side_effecting);
            assert_eq!(d.version, ISSUES_TOOL_VERSION);
            assert!(!d.exposed_over_mcp, "v1 Issues tools are not MCP-exposed");
        }
    }

    /// **The CRUD caps come from the FROZEN Issues ReBAC fragment (4.9), not invented here.** create →
    /// `issue.create`; update/link/estimate/reorder → `issue.update`; comment → `issue.comment`;
    /// assign/close → `issue.transition`. A fragment rename breaks this (no silent drift).
    #[test]
    fn crud_caps_are_the_issues_rebac_fragment_permissions() {
        assert_eq!(create_tool_def().required_caps, vec!["issue.create"]);
        assert_eq!(update_tool_def().required_caps, vec!["issue.update"]);
        assert_eq!(comment_tool_def().required_caps, vec!["issue.comment"]);
        assert_eq!(link_tool_def().required_caps, vec!["issue.update"]);
        assert_eq!(estimate_tool_def().required_caps, vec!["issue.update"]);
        assert_eq!(reorder_tool_def().required_caps, vec!["issue.update"]);
        assert_eq!(assign_tool_def().required_caps, vec!["issue.transition"]);
        assert_eq!(close_tool_def().required_caps, vec!["issue.transition"]);
        // the object-type half IS the canonical Issues ReBAC name (4.9), not a local string.
        assert_eq!(issue_objects::ISSUE, "issue");
    }

    /// **A hand-loosened `close` registration is REJECTED LOUD (VISION §3 no-silent-loosening).** The
    /// `close` gate is the conservative floor — un-gating it without a written deviation is a structural
    /// refusal (the SAME guard that protects the SLA transition).
    #[test]
    fn a_hand_loosened_close_registration_is_rejected_loud() {
        let mut loosened = close_tool_def();
        loosened.requires_approval = false;
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "issues");
        assert_eq!(err.tool, "close");
        assert!(err.to_string().contains("WITHOUT a written deviation"));
    }

    /// **The LINEAR forecast is `ceil(remaining / velocity)` (the named R-5 floor).** A partial period
    /// rounds up; velocity 0 ⇒ no defensible date (`None`) + at-risk (never a divide-by-zero).
    #[test]
    fn linear_forecast_is_ceil_remaining_over_velocity() {
        // 100 remaining ÷ 10/period = 10 periods exactly; threshold 12 → not at-risk.
        let f = LinearForecast::forecast(&ForecastInput {
            remaining: 100,
            velocity_per_period: 10,
            at_risk_threshold_periods: 12,
        });
        assert_eq!(f.periods_to_completion, Some(10));
        assert!(!f.at_risk, "10 ≤ 12 → not at-risk");

        // 101 ÷ 10 = 10.1 → ceil = 11 periods; threshold 10 → at-risk (crossed).
        let f2 = LinearForecast::forecast(&ForecastInput {
            remaining: 101,
            velocity_per_period: 10,
            at_risk_threshold_periods: 10,
        });
        assert_eq!(
            f2.periods_to_completion,
            Some(11),
            "a partial period rounds up"
        );
        assert!(f2.at_risk, "11 > 10 → at-risk");

        // velocity 0 ⇒ no defensible date + at-risk by construction (no divide-by-zero).
        let f3 = LinearForecast::forecast(&ForecastInput {
            remaining: 50,
            velocity_per_period: 0,
            at_risk_threshold_periods: 5,
        });
        assert_eq!(
            f3.periods_to_completion, None,
            "velocity 0 → no defensible date"
        );
        assert!(f3.at_risk, "no velocity → at-risk (the worst case)");
    }

    /// **AG-D9 (mock-determinism, the forecast agent) — two replays over the same forecast input are
    /// BYTE-IDENTICAL.** The deterministic scripted brain makes the forecast agent golden- and
    /// mutation-testable (the identical-sequence is the AG-D9 green artifact).
    #[test]
    fn ag_d9_forecast_agent_replay_is_byte_identical() {
        let input = ForecastInput {
            remaining: 100,
            velocity_per_period: 10,
            at_risk_threshold_periods: 12,
        };
        let a = replay_forecast_agent(&input);
        let b = replay_forecast_agent(&input);
        assert_eq!(a, b, "AG-D9: two forecast-agent replays are byte-identical");
        assert!(
            a.terminated,
            "the forecast agent terminates (a single Submit)"
        );
        // the terminal submission carries the linear forecast summary (compute-only, no mutation).
        let s = a.submission.expect("the forecast agent submits");
        assert!(
            s.0.contains("forecast(linear): ~10 period(s)"),
            "the submission carries the linear forecast: {}",
            s.0
        );
    }

    /// **AG-D9 (mock-determinism, the triage agent) — two dry-run suggestion strips over the same
    /// issue are BYTE-IDENTICAL proposed-effect sequences (the effect-sequence determinism applied to
    /// Issues' triage agent).** The strip is the S9 suggestion (8.7) — proposed, NOT applied.
    #[test]
    fn ag_d9_triage_suggestion_strip_is_byte_identical_and_proposes_one_effect() {
        let a = triage_suggestion_strip("ENG-42");
        let b = triage_suggestion_strip("ENG-42");
        assert_eq!(
            a, b,
            "AG-D9: two triage dry-run strips are byte-identical (effect-sequence determinism)"
        );
        // the triage agent proposes exactly ONE advisory effect (the `triage` suggestion).
        assert_eq!(a.len(), 1, "the triage agent proposes one advisory effect");
        // it is the `triage` tool, for the named issue (a ProposedEffect carrier, NOT an apply).
        let carrier = &a[0].0;
        assert!(
            carrier.contains("tool=triage"),
            "the proposed effect is triage: {carrier}"
        );
        assert!(carrier.contains("ENG-42"), "for the named issue: {carrier}");
    }
}
