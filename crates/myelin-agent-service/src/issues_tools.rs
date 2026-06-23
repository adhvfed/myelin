//! # `issues_tools` — the per-consumer **Issues** ToolDefs registered into the ONE ToolSurface
//! (AG-P20 → P-347, M4): `forecast` / `triage` / `sla_draft` (advisory, NOT gated) + `transition`
//! (the SLA-bound, approver-edged transition — the field/transition ABAC caveat, §5.2 step 2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §6.1 (ONE catalogue, two
//! front-ends — every subsystem registers typed [`ToolDef`]s into the ONE shared [`ToolSurface`]; the
//! same registry is consumed internally by the loop and externally as the MCP projection — NO second
//! governance model), §6.3 (**the FROZEN `requires_approval` defaults table** — Issues
//! `forecast`/`triage`/`sla_draft` = **no** (advisory; the human accepts the suggestion), `transition`
//! on an SLA-bound issue = **yes IF the transition has an approver edge** — the field/transition ABAC
//! caveat, §5.2 step 2, OQ-E), §5.2 step 2 (the field/transition ABAC caveat is evaluated at
//! `check`-time via [`CaveatContext`](myelin_identity::CaveatContext), OFF the hot `list_objects`
//! path).
//!
//! **VISION §3** (suggest-by-default; consequential/irreversible actions human-confirmed — the three
//! advisory tools SUGGEST, the SLA-bound transition is the governed action). **EI-03 §4 / §7** (each
//! new tool is a PROJECTION of the existing plan-then-apply path — NO new engine: a `ToolDef` is data,
//! not code; an `@agent` mention NOTIFIES, it does not auto-spawn — the dispatch tier owns that, see
//! [`crate::dispatch`]). **EI-01 §7** (the compounding payoff — this consumer surface is the SAME
//! registration shape as the Git/KN producer surfaces; no new machinery).
//!
//! **Contract-index:** OWNS the Issues slice of **8.1** (`register_tool` — the four Issues consumer
//! ToolDefs). CONSUMES **4.9** (the Issues ReBAC fragment supplies the `required_caps` — the
//! `issue.transition` / `issue_transition.perform_transition` permission names from
//! [`myelin_issues::rebac_fragment`]) + **4.2** (the EffectApi `check` step evaluates the
//! transition-ABAC caveat for the SLA-bound transition; the caveat carrier is
//! [`crate::effect_api::PlannedEffect::transition`]). The `requires_approval` column is SEEDED from the
//! frozen §6.3 table via [`crate::defaults::seed_requires_approval`] (AG-P8).
//!
//! ## What this prompt (AG-P20) ships — the Issues consumer ToolDefs (NO new engine)
//! - [`forecast_tool_def`] / [`triage_tool_def`] / [`sla_draft_tool_def`] — `effect_kind = mutate`
//!   (the advisory suggestion is recorded through the plan-then-apply path — cap-checked, metered,
//!   audited — but NOT HITL-gated, `requires_approval = no`), `required_caps = [issue.transition]`
//!   (4.9 — drafting a transition/forecast/triage suggestion is governed by the same `transition`
//!   permission the human is). They apply DIRECTLY through the pipeline (suggest-by-default).
//! - [`transition_tool_def`] — `knowledge`-style `mutate` tool: the SLA-bound `transition(issue,
//!   →done)`. `requires_approval = yes` (the frozen §6.3 conservative floor — gated). The
//!   field/transition ABAC caveat (§5.2 step 2) is the REFINEMENT: a transition WITHOUT an approver
//!   edge is admitted by the caveat at `check`-time, but the `tool_def` default is gated so the gate
//!   is the conservative floor (never a loosening). `required_caps =
//!   [issue_transition.perform_transition]` (4.9). The caveat is carried into the pipeline by
//!   [`transition_caveat`].
//! - [`register_issues_tools`] — registers ALL FOUR into a caller-supplied [`ToolSurface`] through the
//!   frozen seed + the no-silent-loosening guard ([`crate::defaults::assert_no_silent_loosening`]), so
//!   a registration that tried to silently un-gate `transition` is REJECTED LOUD (VISION §3).
//!
//! ## The transition-ABAC caveat (§5.2 step 2, OQ-E) — gated-floor + caveat-refinement
//! The frozen §6.3 default for `("issues", "transition")` is `true` (gated) — the conservative floor.
//! The approver-edge ABAC is the REFINEMENT evaluated at `check`-time (4.2): the EffectApi pipeline's
//! step 2 passes [`PlannedEffect::transition`](crate::effect_api::PlannedEffect) into the
//! [`CaveatContext`](myelin_identity::CaveatContext); a transition WITHOUT an approver edge resolves
//! `Allow` (no gate needed at the ABAC layer), a SLA-bound transition WITH an approver edge resolves
//! the gate. The caveat NEVER LOOSENS the gated floor — `Conditional` (a caveat needing missing
//! context) is treated as a DENY (fail-closed, ADR-03). [`transition_caveat`] builds the carrier the
//! dispatch/loop tier stamps onto the planned effect.
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **NONE for the Issues tools** — they are projections of the existing plan-then-apply path (the
//!   apply pipeline AG-P6, the HITL withhold AG-P9, the transition-ABAC caveat in
//!   [`crate::effect_api`], the frozen defaults AG-P8 all already exist).
//! - **The external MCP ENDPOINT** (auth + the agent-lane rate-limit) is the post-M5 follow-on
//!   (AG-P25); these consumer tools are NOT MCP-exposed at v1 (`exposed_over_mcp = false`).

use myelin_agent::{EffectKind, ToolDef, ToolName, ToolSurface};
use myelin_identity::{CaveatContext, TransitionId};
use myelin_issues::rebac_fragment::object_types as issue_objects;
use myelin_tenancy::ArtifactRef;

use crate::defaults::{assert_no_silent_loosening, seed_requires_approval, LooseningViolation};

// ───────────────────────── the frozen Issues consumer-tool identity (the §6.3 keys) ──────────────

/// **The Issues subsystem token** — the `subsystem` half of the catalogue key `(subsystem, name,
/// version)` and the key the FROZEN §6.3 defaults table is looked up under (`("issues", "transition")`
/// → gated, `("issues", "forecast")` → not). The SINGLE source of truth so a typo can't drift the
/// seed.
pub const ISSUES_SUBSYSTEM: &str = "issues";

/// **The `issues.forecast` tool name** (§6.3 — advisory, NOT gated). The seed keys on
/// `("issues", "forecast")`.
pub const FORECAST_TOOL: &str = "forecast";

/// **The `issues.triage` tool name** (§6.3 — advisory, NOT gated). The seed keys on
/// `("issues", "triage")`.
pub const TRIAGE_TOOL: &str = "triage";

/// **The `issues.sla_draft` tool name** (§6.3 — advisory, NOT gated). The seed keys on
/// `("issues", "sla_draft")`.
pub const SLA_DRAFT_TOOL: &str = "sla_draft";

/// **The `issues.transition` tool name** (§6.3 — the SLA-bound, approver-edged transition; gated
/// floor + ABAC caveat). The seed keys on `("issues", "transition")`.
pub const TRANSITION_TOOL: &str = "transition";

/// **The ToolDef version** the Issues consumer tools register at (forward-only; the catalogue key is
/// `(subsystem, name, version)`, §4.2). v1 is the first frozen shape.
pub const ISSUES_TOOL_VERSION: u32 = 1;

// ───────────────────────── the required_caps from the Issues ReBAC fragment (4.9) ────────────────

/// **The `required_caps` for the advisory Issues tools (CONSUMED from 4.9).** Drafting a forecast /
/// triage / SLA suggestion is governed by the `issue.transition` permission Issues' frozen ReBAC
/// fragment declares ([`issue_fragment`](myelin_issues::rebac_fragment::issue_fragment): `transition =
/// assignee + parent_project->write`) — an agent may only SUGGEST a transition it could perform. Built
/// from the canonical `myelin-issues` object-type constant so a rename in the fragment is a
/// compile-or-test break here, never a silent drift.
pub fn advisory_required_caps() -> Vec<String> {
    vec![format!("{}.transition", issue_objects::ISSUE)]
}

/// **The `required_caps` for `issues.transition` (CONSUMED from 4.9).** Performing the SLA-bound,
/// approver-edged transition is governed by the `issue_transition.perform_transition` permission the
/// frozen ABAC sub-object declares
/// ([`issue_transition_fragment`](myelin_issues::rebac_fragment::issue_transition_fragment):
/// `perform_transition = parent_issue->transition` + the approver-role caveat at check-time). The cap
/// the EffectApi `check` step (4.2) resolves under the [`CaveatContext`].
pub fn transition_required_caps() -> Vec<String> {
    vec![format!(
        "{}.perform_transition",
        issue_objects::ISSUE_TRANSITION
    )]
}

// ───────────────────────── the four Issues consumer ToolDefs (8.1 — the OWNED registration) ───────

/// Build one of the three advisory Issues ToolDefs (forecast / triage / sla_draft) — a reversible
/// `Mutate` tool that records its suggestion through the plan-then-apply path (cap-checked, metered)
/// but is NOT HITL-gated (suggest-by-default, §6.3 → `requires_approval = no`, seeded). `required_caps
/// = [issue.transition]` (4.9 — an agent may only suggest a transition it could perform).
fn advisory_tool_def(name: &str, input_schema: &str) -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(name.to_string()),
        subsystem: ISSUES_SUBSYSTEM.to_string(),
        version: ISSUES_TOOL_VERSION,
        input_schema: input_schema.to_string(),
        required_caps: advisory_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (a reversible advisory tool is NOT gated).
        requires_approval: false,
        exposed_over_mcp: false,
    })
}

/// **The `issues.forecast` ToolDef (8.1) — the advisory, NON-gated forecast suggestion (§6.3).** The
/// agent forecasts an issue's completion/effort; the human accepts the suggestion (suggest-by-default).
pub fn forecast_tool_def() -> ToolDef {
    advisory_tool_def(
        FORECAST_TOOL,
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"horizon_days":{"type":"integer"}}}"#,
    )
}

/// **The `issues.triage` ToolDef (8.1) — the advisory, NON-gated triage suggestion (§6.3).** The agent
/// suggests a priority/label/assignee; the human accepts it.
pub fn triage_tool_def() -> ToolDef {
    advisory_tool_def(
        TRIAGE_TOOL,
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"priority":{"type":"string"},"labels":{"type":"array"}}}"#,
    )
}

/// **The `issues.sla_draft` ToolDef (8.1) — the advisory, NON-gated SLA-draft suggestion (§6.3).** The
/// agent drafts an SLA policy/response; the human accepts it.
pub fn sla_draft_tool_def() -> ToolDef {
    advisory_tool_def(
        SLA_DRAFT_TOOL,
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"sla_class":{"type":"string"}}}"#,
    )
}

/// **The `issues.transition` ToolDef (8.1 / §5.2 step 2) — the SLA-bound, approver-edged transition
/// (the gated FLOOR + the field/transition ABAC caveat).**
///
/// - `effect_kind = Mutate` ⇒ it routes through [`EffectApi::apply`](myelin_agent::EffectApi) —
///   plan-then-apply, NEVER a direct mutation (§5.0).
/// - `requires_approval` is SEEDED from the frozen §6.3 default (`("issues", "transition")` →
///   `true`), the CONSERVATIVE FLOOR: an SLA-bound transition with an approver edge WITHHOLDS at step
///   6 → `Gated` until the HITL resume (AG-P9). The approver-edge ABAC is the REFINEMENT evaluated at
///   `check`-time (4.2), carried by [`transition_caveat`]; the caveat NEVER LOOSENS the gated floor.
/// - `required_caps = [issue_transition.perform_transition]` (4.9) — the cap the EffectApi `check`
///   step enforces under the [`CaveatContext`].
/// - `exposed_over_mcp = false` — internal-loop only at v1 (the external MCP endpoint is AG-P25).
pub fn transition_tool_def() -> ToolDef {
    seed_requires_approval(ToolDef {
        name: ToolName(TRANSITION_TOOL.to_string()),
        subsystem: ISSUES_SUBSYSTEM.to_string(),
        version: ISSUES_TOOL_VERSION,
        // The transition input the Issues public endpoint validates (the issue ref + the target state
        // + the optional approver basis). An opaque-string JSON-Schema carrier at this seam.
        input_schema: r#"{"type":"object","required":["issue","to_state"],"properties":{"issue":{"type":"string"},"to_state":{"type":"string"},"approver":{"type":"string"}}}"#.to_string(),
        required_caps: transition_required_caps(),
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        // SEEDED below from §6.3 (the gated floor for the SLA-bound transition → true).
        requires_approval: true,
        exposed_over_mcp: false,
    })
}

// ───────────────────────── the transition-ABAC caveat carrier (§5.2 step 2, 4.2) ─────────────────

/// **Build the field/transition ABAC [`CaveatContext`] for the SLA-bound `issues.transition` (§5.2
/// step 2, OQ-E, 4.2).** The dispatch/loop tier stamps this onto the
/// [`PlannedEffect`](crate::effect_api::PlannedEffect) so the EffectApi `check` step (4.2) evaluates
/// the approver-edge ABAC at `check`-time — OFF the hot `list_objects` path. The caveat carries the
/// target `issue` `object` + the [`TransitionId`] the gate is keyed on; a transition WITHOUT an
/// approver edge is admitted by the caveat (no ABAC gate), an SLA-bound one with an approver edge
/// resolves the gate. The caveat NEVER LOOSENS the frozen gated FLOOR — a `Conditional` (missing
/// approver context) is a DENY, never a silent allow (fail-closed, ADR-03). `field` is `None` (this is
/// a transition-level, not a field-level, caveat); `attrs` is empty at this seam (the predicate
/// evaluator is Identity's). This is the SAME shape [`PlanThenApply::apply`](crate::effect_api::PlanThenApply)
/// builds internally from the `PlannedEffect`, surfaced here so the consumer-tool layer can construct
/// it for the dispatch tier without re-deriving the field list.
pub fn transition_caveat(
    issue_object: ArtifactRef,
    transition_id: impl Into<String>,
) -> CaveatContext {
    CaveatContext {
        object: issue_object,
        field: None,
        transition: Some(TransitionId(transition_id.into())),
        attrs: std::collections::BTreeMap::new(),
    }
}

/// **The four Issues consumer ToolDefs, in catalogue order (forecast → triage → sla_draft →
/// transition).** The single list every registration + CDC consumes (one source of truth — a drift in
/// any def is caught once). All four are SEEDED from the frozen §6.3 defaults; only `transition` is
/// gated.
pub fn issues_tool_defs() -> Vec<ToolDef> {
    vec![
        forecast_tool_def(),
        triage_tool_def(),
        sla_draft_tool_def(),
        transition_tool_def(),
    ]
}

// ───────────────────────── the registration seam (8.1 — into the ONE ToolSurface) ────────────────

/// **Register the Issues consumer ToolDefs into the ONE [`ToolSurface`] (8.1 / §6.1) — the OWNED
/// deliverable.** Every def is passed through the VISION §3 no-silent-loosening guard FIRST
/// ([`assert_no_silent_loosening`]): a registration that tried to flip the frozen `transition`
/// `yes → no` WITHOUT a written deviation is REJECTED LOUD (`Err`), never silently un-gated. The defs
/// themselves are already seeded from the frozen table, so the strict (no-deviation) call always
/// admits them — the guard is the structural proof that a future hand-edit can't loosen the
/// SLA-bound-transition gate unnoticed. Identical in shape to
/// [`crate::git_tools::register_git_tools`] / [`crate::knowledge_tools::register_knowledge_tools`] —
/// the compounding-payoff reuse.
pub fn register_issues_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    let defs = issues_tool_defs();
    for def in &defs {
        assert_no_silent_loosening(def, &[])?;
    }
    for def in &defs {
        surface.register_tool(def.clone());
    }
    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::requires_approval_default;

    /// A `ToolSurface` over a fixed catalogue (the §4.2 in-memory registry). The ONE catalogue all
    /// four consumer tools register into.
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

    /// **The three advisory tools carry the FROZEN §6.3 `requires_approval = no` default (suggest) —
    /// seeded, not hand-set.** They apply DIRECTLY through the pipeline (no HITL gate).
    #[test]
    fn forecast_triage_sla_draft_are_advisory_not_gated() {
        for (def, tool) in [
            (forecast_tool_def(), FORECAST_TOOL),
            (triage_tool_def(), TRIAGE_TOOL),
            (sla_draft_tool_def(), SLA_DRAFT_TOOL),
        ] {
            assert!(
                !def.requires_approval,
                "issues.{tool} is advisory → NOT gated (§6.3)"
            );
            assert_eq!(
                def.requires_approval,
                requires_approval_default(ISSUES_SUBSYSTEM, tool),
                "issues.{tool}'s (non-)gating IS the frozen §6.3 default (seeded, not hand-set)"
            );
            assert_eq!(def.effect_kind, EffectKind::Mutate);
            assert!(def.side_effecting);
        }
    }

    /// **`issues.transition` carries the FROZEN §6.3 `requires_approval = yes` gated FLOOR — seeded,
    /// not hand-set.** The SLA-bound, approver-edged transition WITHHOLDS until the HITL resume; the
    /// ABAC caveat is the refinement, never a loosening of this floor.
    #[test]
    fn transition_is_the_gated_floor_by_the_frozen_default() {
        let def = transition_tool_def();
        assert!(
            def.requires_approval,
            "issues.transition is the gated floor (§6.3 SLA-bound transition)"
        );
        assert_eq!(
            def.requires_approval,
            requires_approval_default(ISSUES_SUBSYSTEM, TRANSITION_TOOL),
            "issues.transition's gating IS the frozen §6.3 default (the conservative floor)"
        );
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
    }

    /// **The `required_caps` come from the FROZEN Issues ReBAC fragment (4.9), not invented here.**
    /// The advisory tools → `issue.transition`; `transition` → `issue_transition.perform_transition`.
    /// Built from the canonical `myelin-issues` object-type constants, so a fragment rename breaks
    /// this test (no silent drift — the Issues parallel to the Git/KN CDCs).
    #[test]
    fn required_caps_are_the_issues_rebac_fragment_permissions() {
        assert_eq!(
            forecast_tool_def().required_caps,
            vec!["issue.transition".to_string()]
        );
        assert_eq!(
            transition_tool_def().required_caps,
            vec!["issue_transition.perform_transition".to_string()]
        );
        // the object-type halves ARE the canonical Issues ReBAC names (4.9), not local strings.
        assert_eq!(issue_objects::ISSUE, "issue");
        assert_eq!(issue_objects::ISSUE_TRANSITION, "issue_transition");
    }

    /// **The transition-ABAC caveat (§5.2 step 2) carries the [`TransitionId`] the gate keys on, and
    /// it is a TRANSITION-level (not field-level) caveat.** The carrier the dispatch/loop tier stamps
    /// onto the planned effect so the EffectApi `check` step evaluates the approver-edge ABAC at
    /// check-time (4.2) — never on the hot `list_objects` path.
    #[test]
    fn transition_caveat_carries_the_transition_id_not_a_field() {
        let caveat = transition_caveat(
            ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            "issue:42:open->done",
        );
        assert_eq!(
            caveat.transition,
            Some(TransitionId("issue:42:open->done".into())),
            "the caveat carries the SLA-bound transition the approver-edge ABAC gates"
        );
        assert!(
            caveat.field.is_none(),
            "this is a transition-level caveat, not a field-level one"
        );
        assert_eq!(
            caveat.object,
            ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            "the caveat carries the target issue object the check resolves against"
        );
        assert!(
            caveat.attrs.is_empty(),
            "attrs is empty at this seam (the predicate evaluator is Identity's)"
        );
    }

    /// **`register_issues_tools` registers ALL FOUR consumer ToolDefs into the ONE catalogue (8.1 /
    /// §6.1) and they resolve by name with their frozen shapes.** The registration is the whole
    /// deliverable — a `ToolDef` is a row in the ONE registry, no second governance model.
    #[test]
    fn register_issues_tools_registers_all_four_into_the_one_surface() {
        let mut cat = Catalogue { defs: vec![] };
        let registered = register_issues_tools(&mut cat).expect("seeded defs always admit");
        assert_eq!(
            registered.len(),
            4,
            "forecast + triage + sla_draft + transition"
        );

        let transition = cat
            .resolve(&ToolName(TRANSITION_TOOL.into()))
            .expect("transition registered");
        assert_eq!(transition.subsystem, ISSUES_SUBSYSTEM);
        assert!(
            transition.requires_approval,
            "the registered transition is the gated floor"
        );
        assert_eq!(
            transition.required_caps,
            vec!["issue_transition.perform_transition".to_string()]
        );

        let forecast = cat
            .resolve(&ToolName(FORECAST_TOOL.into()))
            .expect("forecast registered");
        assert!(
            !forecast.requires_approval,
            "the registered forecast is advisory (NOT gated)"
        );

        // a tool NOT registered resolves to None (the catalogue is exactly these four).
        assert!(cat.resolve(&ToolName("issues.delete".into())).is_none());
    }

    /// **The no-silent-loosening guard (VISION §3) protects the registration path.** An
    /// `issues.transition` def hand-loosened to `requires_approval = false` WITHOUT a written deviation
    /// is REJECTED LOUD — proving the registration seam can't silently un-gate the SLA-bound
    /// transition.
    #[test]
    fn a_hand_loosened_transition_registration_is_rejected_loud() {
        let mut loosened = transition_tool_def();
        loosened.requires_approval = false;
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "issues");
        assert_eq!(err.tool, "transition");
        assert!(
            err.to_string().contains("WITHOUT a written deviation"),
            "the loosening is surfaced LOUD: {err}"
        );
    }

    /// **The compounding-payoff / no-new-engine check (EI-03 §4 / EI-01 §7).** Every Issues consumer
    /// tool is PURE data: a `mutate` `ToolDef` whose gating is the frozen §6.3 seed and whose caps are
    /// the frozen 4.9 fragment — there is NO bespoke apply/gate machinery here (the routing + gating +
    /// HITL + the transition-ABAC caveat are the existing pipeline). All four route the SAME way
    /// (`Mutate` → EffectApi) and differ ONLY in their frozen `requires_approval` seed.
    #[test]
    fn the_issues_tools_are_a_projection_not_a_new_engine() {
        let defs = issues_tool_defs();
        assert_eq!(defs.len(), 4);
        for d in &defs {
            assert_eq!(
                d.effect_kind,
                EffectKind::Mutate,
                "every Issues consumer tool routes through EffectApi — no new path"
            );
            assert!(d.side_effecting);
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
        // exactly ONE Issues tool is gated: the SLA-bound transition (the consequential split).
        let gated: Vec<&str> = defs
            .iter()
            .filter(|d| d.requires_approval)
            .map(|d| d.name.0.as_str())
            .collect();
        assert_eq!(
            gated,
            vec!["transition"],
            "only the SLA-bound transition is gated; forecast/triage/sla_draft are advisory"
        );
    }
}
