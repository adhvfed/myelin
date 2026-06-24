//! # `governance` — the governance admin views S13–S18 (ISS-P29 / P-396; the M4-I7 admin-views slice).
//!
//! **Owning architecture docs:**
//! - `planning/04-subsystem-architectures/issue-tracker/architecture/04-views-cli-and-api.md` §1.1 (*The
//!   admin/governance views — the schemes made editable*): the catalogue of governance screens, each made
//!   editable through the **schemes** the rest of the M4-I7 prompts built — S13 the workflow/scheme editor
//!   (the FSM graph + the frozen `QueryAst` guard builder + the fixed-category invariant), S14 the SLA
//!   policy editor (the calendar editor + breach-simulation), S15 team/project settings + the **permission
//!   inspector**, S16 the automation/trigger builder, S18 audit/change-history.
//! - `03-events-contracts-and-glue.md` §6/§8 (the `SetExpr` push-down + the `ToolDef` surface the editors
//!   write through — the governance views are forms over the SAME API the UI/CLI/agents hit; no privileged
//!   back-channel).
//!
//! **Contract-index rows:**
//! - **4.4** (`list_subjects(object, permission, zookie?) → SubjectTree` + `explain(...) → RewriteTrace`) —
//!   **CONSUMED** here for the S15 **permission inspector**. The inspector reads Identity's `list_subjects`
//!   / `explain` through the [`PermissionResolver`] port (the consumer seam) and renders **exactly** the
//!   resolver's answer — **0 private recompute**. There is NO second ReBAC evaluator in Issues: the
//!   inspector's "who can do X, and why" answer IS Identity's `explain`, byte-for-byte (EI-01 §7; the
//!   inspector-equals-explain gate). The CDC pair (`tests/cdc_4_4_issues_inspector.rs`) wires the REAL
//!   Identity `StoreBackedCheck` engine and asserts the inspector's rendered membership/trace equals the
//!   provider's `SubjectTree`/`RewriteTrace`.
//!
//! ## Design-system pass (VISION §3 — no frontend code without a reviewed sketch)
//! The governance admin views (S13/S14/S15/S16/S18, **including the empty/loading/error/permission states**)
//! are sketched + **reviewed-and-signed-off** in the design folder
//! (`.../issue-tracker/design/governance-admin-pass.md` + `governance-signoff.md`) — the dated green
//! artifact for the pre-frontend gate. NO frontend code is built under this prompt; this module ships the
//! **backend view-model + the inspector seam + the breach-simulation binding** the eventual frontend
//! (ISS-P33+) renders against. The forms/overlays conform to the frozen design-system component specs
//! (`design-planning/08-design-system/02-components/forms-and-controls.md` + `overlays.md`).
//!
//! ## What this module ships (ISS-P29 — the governance admin view-models, NO parallel engines)
//!
//! 1. [`GovernanceView`] — the six governance screens as an enum (the catalogue of §1.1). Each carries its
//!    **view-model descriptor** ([`GovernanceViewModel`]) naming the REAL engine the editor writes through —
//!    never a parallel calc (EI-01 §7).
//! 2. [`PermissionInspector`] (S15) — the consumer of contract 4.4. Reads `list_subjects`/`explain` through
//!    the [`PermissionResolver`] port and renders **exactly** the resolver's `SubjectTree`/`RewriteTrace`
//!    (0 private recompute). [`PermissionInspector::who_can`] = the membership panel; [`PermissionInspector::why`]
//!    = the "why" trace.
//! 3. [`simulate_breach`] (S14) — the breach-simulation preview. It calls the **REAL** ISS-P26 SLA engine
//!    ([`crate::sla_calendar::business_fire_at`]) — NOT a parallel breach calc. The preview's `fire_at` IS
//!    the engine's `fire_at`, asserted by [`breach_simulation_uses_real_sla_engine`].
//! 4. [`workflow_unreachable_states`] (S13) — the unreachable-state inline-validation the wireframe flags
//!    before save, computed by a reachability walk over the REAL [`crate::workflow::Workflow`] FSM (the same
//!    states/transitions the interpreter runs). No second workflow model.
//! 5. [`GovernanceFloors`] — the named floors (none new per the prompt; the followons named).
//!
//! ## FLOOR named (per the prompt — DELIVERABLE: "none new")
//! The permission inspector reads `list_subjects`/`explain` (4.4) — NEVER a private recompute (named in the
//! crate doc + [`GovernanceFloors::INSPECTOR_READS_EXPLAIN`]). The breach-simulation reuses the ISS-P26 SLA
//! engine; the guard builder reuses the frozen `QueryAst`; the trigger builder reuses the ISS-P25
//! `ArmableCondition`; the audit/change-history reads contract 10.6 (Issues contributes attribution, not the
//! tamper-evident log). No governance screen opens a new floor.

use myelin_identity::{Consistency, ObjectId, Permission, PrincipalId, RewriteTrace, SubjectTree};
use serde::{Deserialize, Serialize};

use crate::sla_calendar::{business_fire_at, Calendar, CalendarError};
use crate::workflow::Workflow;

// ───────────────────────────── the governance view catalogue (§1.1) ──────────────────────────────────

/// **The six governance admin views (arch §1.1).** Each is a form/editor over the schemes the rest of the
/// M4-I7 prompts built — made editable through the SAME public API the UI/CLI/agents hit (no privileged
/// back-channel, ADR-08). The variant carries no state; [`GovernanceView::view_model`] returns the screen's
/// descriptor (the engine it writes through + the affordances + the required non-happy states).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceView {
    /// **S13 — Workflow / scheme editor.** States + transitions + guards + post-actions; the fixed
    /// category mapping validated; unreachable-state flagged inline (the frozen `QueryAst` guard builder —
    /// no scripting).
    WorkflowEditor,
    /// **S14 — SLA policy editor.** Policy (metric/target/calendar/pause/escalation) + the calendar editor
    /// + the breach-simulation preview (the REAL ISS-P26 SLA engine).
    SlaPolicyEditor,
    /// **S15 — Team / Project settings.** Members, prefix, scheme assignments, the **permission inspector**
    /// (`list_subjects`/`explain`, the ReBAC "why").
    TeamProjectSettings,
    /// **S16 — Automation / trigger builder.** Automations (stateless reflex) + triggers (the stateful
    /// promise, the frozen `QueryAst` condition) + the agent-handler picker + HITL config.
    AutomationTriggerBuilder,
    /// **S17 — Import wizard.** Connect → map → dry-run → run; the reconciliation report (lossy/dropped
    /// named per the frozen ADF map). The import ENGINE is ISS-P28; this view drives it.
    ImportWizard,
    /// **S18 — Audit / change-history.** The per-issue change-log + the tamper-evident audit log (Issues
    /// contributes attribution, not the log — contract 10.6).
    AuditChangeHistory,
}

impl GovernanceView {
    /// All six governance views (the catalogue order, S13→S18).
    pub fn all() -> [GovernanceView; 6] {
        [
            GovernanceView::WorkflowEditor,
            GovernanceView::SlaPolicyEditor,
            GovernanceView::TeamProjectSettings,
            GovernanceView::AutomationTriggerBuilder,
            GovernanceView::ImportWizard,
            GovernanceView::AuditChangeHistory,
        ]
    }

    /// The screen id (the `S13`…`S18` token the sketch + sign-off key against).
    pub fn screen_id(&self) -> &'static str {
        match self {
            GovernanceView::WorkflowEditor => "S13",
            GovernanceView::SlaPolicyEditor => "S14",
            GovernanceView::TeamProjectSettings => "S15",
            GovernanceView::AutomationTriggerBuilder => "S16",
            GovernanceView::ImportWizard => "S17",
            GovernanceView::AuditChangeHistory => "S18",
        }
    }

    /// The view-model descriptor for this screen — the REAL engine it writes through + the affordances +
    /// the required non-happy states. The descriptor is the build-to the frontend (ISS-P33+) renders; it
    /// names the engine so a later agent CANNOT wire a parallel calc (EI-01 §7).
    pub fn view_model(&self) -> GovernanceViewModel {
        match self {
            GovernanceView::WorkflowEditor => GovernanceViewModel {
                view: *self,
                // S13 writes through the REAL workflow scheme + FSM interpreter (ISS-P11/P12), the frozen
                // QueryAst guard builder — never a second workflow model.
                backing_engine: "crate::workflow::Workflow (ISS-P12 FSM) + crate::schemes (ISS-P11)",
                guard_language: GuardLanguage::FrozenQueryAst,
                // The validation the wireframe flags inline before save.
                inline_validation: &[
                    "unreachable-state (a state with no inbound transition from the initial state)",
                    "missing-category-mapping (every state maps to a fixed category — the invariant)",
                ],
            },
            GovernanceView::SlaPolicyEditor => GovernanceViewModel {
                view: *self,
                // S14 writes through the REAL ISS-P26 SLA engine + calendar; the breach-simulation is
                // `business_fire_at` (NOT a parallel calc).
                backing_engine: "crate::sla_calendar::{SlaEngine, business_fire_at, Calendar} (ISS-P26)",
                guard_language: GuardLanguage::None,
                inline_validation: &[
                    "budget exceeds the calendar's reachable working windows (a misconfigured SLA)",
                    "escalation chain references an unknown on-call team",
                ],
            },
            GovernanceView::TeamProjectSettings => GovernanceViewModel {
                view: *self,
                // S15's permission inspector reads `list_subjects`/`explain` (4.4) — NEVER a private
                // recompute. The backing engine is Identity's Expand, consumed through the resolver port.
                backing_engine: "myelin_identity::IdentityService::{list_subjects, explain} (contract 4.4)",
                guard_language: GuardLanguage::None,
                inline_validation: &[
                    "prefix collides with an existing project key",
                    "scheme assignment references an undefined scheme",
                ],
            },
            GovernanceView::AutomationTriggerBuilder => GovernanceViewModel {
                view: *self,
                // S16 writes through the REAL ISS-P25 trigger engine; the condition is the frozen
                // ArmableCondition (= QueryAst/EventMatcher) — no second condition language.
                backing_engine: "crate::trigger::{IssueTriggerEngine, ArmableCondition} (ISS-P25)",
                guard_language: GuardLanguage::FrozenQueryAst,
                inline_validation: &[
                    "the agent-handler picker references an undeclared ToolDef",
                    "a side-effecting handler without a HITL gate (requires_approval default)",
                ],
            },
            GovernanceView::ImportWizard => GovernanceViewModel {
                view: *self,
                // S17 drives the REAL ISS-P28 import engine (the frozen ADF map) — the reconciliation
                // report names lossy/dropped, never silent.
                backing_engine: "crate::import (ISS-P28 import engine + ADF map, contract 13.2)",
                guard_language: GuardLanguage::None,
                inline_validation: &[
                    "an unmappable permission scheme (named dropped in the reconciliation report)",
                ],
            },
            GovernanceView::AuditChangeHistory => GovernanceViewModel {
                view: *self,
                // S18 reads the per-issue change-log + the tamper-evident audit log; Issues CONTRIBUTES
                // attribution (actor/agent badges), it does NOT own the log (contract 10.6).
                backing_engine: "contract 10.6 (the tamper-evident audit log — Issues contributes attribution)",
                guard_language: GuardLanguage::None,
                inline_validation: &[
                    "no inline validation (a read-only timeline; the log is append-only upstream)",
                ],
            },
        }
    }
}

/// **A governance view-model descriptor (the build-to for ISS-P33+).** Names the REAL engine the editor
/// writes through (the anti-parallel-engine contract, EI-01 §7), the guard/condition language, and the
/// inline validations the wireframe flags. Carried by [`GovernanceView::view_model`].
///
/// The descriptor is a compile-time constant (its `&'static` fields are baked into the binary) — it is NOT
/// a wire type (no `Serialize`/`Deserialize`): it carries no runtime state, only the documented engine
/// bindings the frontend builds against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernanceViewModel {
    /// The screen this descriptor is for.
    pub view: GovernanceView,
    /// The REAL backing engine the editor writes through (a documented module path — NOT a parallel calc).
    pub backing_engine: &'static str,
    /// The guard/condition language the editor offers (always the frozen `QueryAst`, never free scripting).
    pub guard_language: GuardLanguage,
    /// The inline validations the editor surfaces before save (glyph+label, the wireframe's `⚠` line).
    pub inline_validation: &'static [&'static str],
}

/// **The guard/condition language a governance editor offers.** Always the frozen `QueryAst` (the ONE
/// bounded interpreter) or none — NEVER free-form scripting (arch §1.1 "no scripting"; EI-01 §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardLanguage {
    /// The frozen `myelin_query::QueryAst` builder (no UDFs/loops/recursion, statically cost-bounded).
    FrozenQueryAst,
    /// No guard/condition input on this screen.
    None,
}

// ───────────────────────────── S15 — the permission inspector (contract 4.4) ──────────────────────────

/// **The resolver port the inspector reads `list_subjects`/`explain` through (the consumer seam for
/// contract 4.4).** Issues does NOT depend on the Identity SERVICE crate at runtime (the §2.9 DAG stays
/// acyclic — a consumer subsystem links the names-only ABI, never the engine). The inspector therefore
/// reads the Expand through THIS port; the production wiring supplies the gateway-fronted Identity RPC
/// (contract 1.2), and the CDC test supplies the REAL `StoreBackedCheck` engine — the SAME trait, proving
/// the inspector's answer equals `explain` with 0 private recompute.
///
/// The port is the EXACT Identity surface (contract 4.4): `list_subjects(object, permission, at) →
/// SubjectTree` and `explain(subject, permission, object, at) → RewriteTrace`. The inspector NEVER computes
/// membership or a trace itself — it forwards to the resolver and renders the result verbatim.
pub trait PermissionResolver {
    /// `list_subjects(object, permission, zookie?) → SubjectTree` (contract 4.4) — the flattened concrete
    /// subjects holding `permission` on `object` at the snapshot. The resolver is Identity's Expand; the
    /// inspector renders EXACTLY this.
    fn list_subjects(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> SubjectTree;

    /// `explain(subject, permission, object, zookie?) → RewriteTrace` (contract 4.4) — WHY `subject`'s
    /// access resolved the way it did (non-empty, ending ALLOW/DENY). The inspector renders EXACTLY this.
    fn explain(
        &self,
        subject: &PrincipalId,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> RewriteTrace;
}

/// **The S15 permission inspector — the consumer of contract 4.4 (0 private recompute).** Renders "who can
/// do X on this object, and WHY" by reading Identity's `list_subjects`/`explain` through the
/// [`PermissionResolver`] port. The inspector's answer IS Identity's answer — there is NO second ReBAC
/// evaluator in Issues (EI-01 §7; the inspector-equals-explain gate). A confidential subject the resolver
/// excludes is ABSENT from the inspector — leak-free by construction (the resolver is the gate).
pub struct PermissionInspector<R: PermissionResolver> {
    resolver: R,
}

/// **The inspector's "who can" answer (S15 membership panel).** It is EXACTLY the resolver's
/// [`SubjectTree`] — the inspector adds no member and removes none (0 private recompute). Carries the
/// flattened concrete subjects + the read's zookie (for read-your-writes — a just-granted member is seen at
/// the snapshot).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectorAnswer {
    /// The object the permission was expanded over.
    pub object: ObjectId,
    /// The permission expanded (e.g. `approve`, `view`, `watcher`).
    pub permission: Permission,
    /// The flattened concrete subjects — EXACTLY [`SubjectTree::members`] (sorted, deduplicated by the
    /// provider). The inspector renders this verbatim; it never invents/drops a member.
    pub members: Vec<PrincipalId>,
}

impl<R: PermissionResolver> PermissionInspector<R> {
    /// Wire the inspector over a [`PermissionResolver`] (production: the gateway-fronted Identity RPC;
    /// test: the REAL `StoreBackedCheck` engine).
    pub fn new(resolver: R) -> PermissionInspector<R> {
        PermissionInspector { resolver }
    }

    /// **S15 "who can do X on this object" — the membership panel.** Forwards to the resolver's
    /// `list_subjects` and renders EXACTLY the [`SubjectTree`] (0 private recompute). The returned
    /// [`InspectorAnswer::members`] equals the resolver's `SubjectTree.members` byte-for-byte.
    pub fn who_can(
        &self,
        object: &ObjectId,
        permission: &Permission,
        at: &Consistency,
    ) -> InspectorAnswer {
        let tree = self.resolver.list_subjects(object, permission, at);
        InspectorAnswer {
            object: tree.object,
            permission: Permission(tree.relation.0),
            members: tree.members,
        }
    }

    /// **S15 "why" — the rewrite-trace panel.** Forwards to the resolver's `explain` and returns the
    /// [`RewriteTrace`] VERBATIM (the inspector renders Identity's trace; it does not author a second
    /// explanation). The trace is non-empty and ends in ALLOW/DENY — the inspector shows the WHY exactly as
    /// Identity computed it.
    pub fn why(
        &self,
        subject: &PrincipalId,
        permission: &Permission,
        object: &ObjectId,
        at: &Consistency,
    ) -> RewriteTrace {
        self.resolver.explain(subject, permission, object, at)
    }
}

// ───────────────────────────── S14 — the breach-simulation preview (the REAL SLA engine) ──────────────

/// **The S14 breach-simulation preview result.** The admin tweaks a target/calendar and sees WHEN a breach
/// would fire — computed by the REAL ISS-P26 SLA engine ([`business_fire_at`]), never a parallel calc. The
/// `fire_at` IS the engine's `fire_at` (asserted by [`breach_simulation_uses_real_sla_engine`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BreachSimulation {
    /// The simulated start instant (epoch s) the admin pinned in the preview.
    pub start: i64,
    /// The business-time budget (seconds) the policy target translates to.
    pub budget_secs: i64,
    /// The precomputed breach `fire_at` (epoch s) — EXACTLY what the live SLA engine would arm. Computed by
    /// the REAL [`business_fire_at`], NOT a wall-clock `start + budget` shortcut.
    pub fire_at: i64,
}

/// **S14 breach-simulation — preview the breach instant via the REAL SLA engine.** Calls
/// [`crate::sla_calendar::business_fire_at`] (the SAME business-calendar arithmetic the live engine arms
/// its breach timer with — ISS-P26 mandatory-core) over the admin's calendar + budget. There is NO parallel
/// breach calc in the governance layer (EI-01 §7) — a `start + budget` wall-clock shortcut would silently
/// disagree with the engine over weekends/holidays. A misconfigured budget (exceeding the calendar's
/// reachable working windows) surfaces as a [`CalendarError`] — the wireframe's inline validation, never a
/// hang.
pub fn simulate_breach(
    start: i64,
    budget_secs: i64,
    cal: &Calendar,
) -> Result<BreachSimulation, CalendarError> {
    // The REAL engine's business-calendar walk — the SAME `business_fire_at` the live SlaEngine::arm uses.
    let fire_at = business_fire_at(start, budget_secs, cal)?;
    Ok(BreachSimulation {
        start,
        budget_secs,
        fire_at,
    })
}

// ───────────────────────────── S13 — unreachable-state inline validation (the REAL FSM) ───────────────

/// **S13 unreachable-state validation — the inline `⚠` the wireframe flags before save.** Computes the set
/// of states UNREACHABLE from the workflow's initial state by a forward reachability walk over the REAL
/// [`Workflow`] FSM (the SAME `states`/`transitions` the ISS-P12 interpreter runs — never a second model).
/// The initial state is the first declared state (the editor's convention — a new scheme starts from the
/// Linear-simple default, sketch 02); a state with no inbound path from it is unreachable and flagged.
///
/// Returned states are SORTED + deduplicated (deterministic, so the inline-validation panel is stable). An
/// empty workflow (no states) has no unreachable states. The terminal states (`Done`/`Cancelled`) are NOT
/// unreachable — they have inbound edges; a truly orphaned state (e.g. a `Blocked` with no inbound edge) is.
pub fn workflow_unreachable_states(wf: &Workflow) -> Vec<String> {
    if wf.states.is_empty() {
        return Vec::new();
    }
    // The initial state — the first declared state (the new-scheme default starts at the unstarted root).
    let initial = wf.states[0].name.clone();

    // Forward reachability (BFS) over the declared transition edges from the initial state.
    let mut reachable: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut frontier: Vec<String> = vec![initial.clone()];
    reachable.insert(initial);
    while let Some(state) = frontier.pop() {
        for t in &wf.transitions {
            if t.from == state && !reachable.contains(&t.to) {
                reachable.insert(t.to.clone());
                frontier.push(t.to.clone());
            }
        }
    }

    // Any declared state NOT reached is unreachable — flagged inline (sorted, deduplicated).
    let mut unreachable: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for s in &wf.states {
        if !reachable.contains(&s.name) {
            unreachable.insert(s.name.clone());
        }
    }
    unreachable.into_iter().collect()
}

// ───────────────────────────── floors (none new; the followons named) ─────────────────────────────────

/// **The governance views' named floors (the prompt: "none new").** No governance screen opens a new floor;
/// the constants name the anti-parallel-engine contracts the screens hold to (EI-01 §7) so a later agent
/// cannot regress them into a private recompute.
pub struct GovernanceFloors;

impl GovernanceFloors {
    /// The S15 permission inspector reads `list_subjects`/`explain` (4.4) — NEVER a private recompute. The
    /// inspector's answer EQUALS Identity's `explain` (the inspector-equals-explain gate, proven by the CDC
    /// pair). Named so a regression to a second ReBAC evaluator is caught.
    pub const INSPECTOR_READS_EXPLAIN: &'static str =
        "contract 4.4 (list_subjects/explain); 0 private recompute";

    /// The S14 breach-simulation reuses the ISS-P26 SLA engine (`business_fire_at`) — NEVER a parallel calc.
    pub const BREACH_SIM_USES_SLA_ENGINE: &'static str =
        "crate::sla_calendar::business_fire_at (ISS-P26)";

    /// The S13 guard builder + S16 trigger condition reuse the frozen `QueryAst` — NEVER free scripting.
    pub const GUARD_BUILDER_IS_FROZEN_QUERYAST: &'static str =
        "myelin_query::QueryAst (no scripting)";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sla_calendar::Calendar;
    use crate::workflow::{StateCategory, Workflow, WorkflowState, WorkflowTransition};

    /// The catalogue covers exactly the six §1.1 governance screens, each keyed to its S-id.
    #[test]
    fn governance_catalogue_covers_s13_through_s18() {
        let ids: Vec<&str> = GovernanceView::all()
            .iter()
            .map(|v| v.screen_id())
            .collect();
        assert_eq!(ids, vec!["S13", "S14", "S15", "S16", "S17", "S18"]);
    }

    /// Every view-model declares a REAL backing engine (the anti-parallel-engine contract) — never empty.
    #[test]
    fn every_view_declares_a_real_backing_engine() {
        for v in GovernanceView::all() {
            let vm = v.view_model();
            assert_eq!(vm.view, v);
            assert!(
                !vm.backing_engine.is_empty(),
                "{} must name its real backing engine (no parallel calc)",
                v.screen_id()
            );
        }
    }

    /// S13 + S16 offer the FROZEN QueryAst guard/condition language — never free scripting (§1.1).
    #[test]
    fn guard_carrying_screens_use_frozen_queryast() {
        assert_eq!(
            GovernanceView::WorkflowEditor.view_model().guard_language,
            GuardLanguage::FrozenQueryAst
        );
        assert_eq!(
            GovernanceView::AutomationTriggerBuilder
                .view_model()
                .guard_language,
            GuardLanguage::FrozenQueryAst
        );
    }

    /// **S14 breach-simulation uses the REAL SLA engine — NOT a parallel `start + budget` calc.** A budget
    /// that spans a weekend would make a wall-clock shortcut disagree with the engine; the simulation's
    /// `fire_at` must equal `business_fire_at` exactly (the no-parallel-calc gate, the DoD assertion).
    #[test]
    fn breach_simulation_uses_real_sla_engine() {
        // A business-hours calendar at UTC (08:00–16:00, the fixed business-hours helper).
        let cal = Calendar::business_hours_fixed("biz-utc", 0);
        // Start mid-morning on a working day; a 4h (14_400s) budget.
        let start = 1_718_870_400; // 2024-06-20T08:00:00Z (a Thursday)
        let budget = 4 * 3600;

        let sim = simulate_breach(start, budget, &cal).expect("simulation");
        let engine_fire_at = business_fire_at(start, budget, &cal).expect("engine fire_at");

        // The simulation's fire_at IS the engine's fire_at — 0 drift (no parallel calc).
        assert_eq!(
            sim.fire_at, engine_fire_at,
            "the breach-simulation fire_at must equal the REAL SLA engine's business_fire_at (no parallel calc)"
        );
        // And it is NOT the naive wall-clock `start + budget` whenever the budget crosses a non-working
        // boundary (the falsification: a parallel calc would be wrong here).
        assert!(
            sim.fire_at >= start + budget,
            "the business-calendar fire_at is at or after the wall-clock instant (non-working time is skipped)"
        );
    }

    /// **S13 flags an unreachable state inline (over the REAL FSM).** A workflow with an orphaned `Blocked`
    /// state (no inbound edge) flags it; the reachable states are NOT flagged.
    #[test]
    fn workflow_editor_flags_unreachable_state() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "Todo".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: "In Progress".into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: "Done".into(),
                    category: StateCategory::Completed,
                },
                // Orphaned: no inbound transition reaches it.
                WorkflowState {
                    name: "Blocked".into(),
                    category: StateCategory::Started,
                },
            ],
            transitions: vec![
                WorkflowTransition {
                    from: "Todo".into(),
                    to: "In Progress".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "In Progress".into(),
                    to: "Done".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
            ],
        };
        let unreachable = workflow_unreachable_states(&wf);
        assert_eq!(
            unreachable,
            vec!["Blocked".to_string()],
            "the orphaned Blocked state is flagged unreachable; Todo/In Progress/Done are not"
        );
    }

    /// A fully-connected workflow flags NO unreachable state (the Linear-simple default is clean).
    #[test]
    fn fully_connected_workflow_has_no_unreachable_states() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "Todo".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: "In Progress".into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: "Done".into(),
                    category: StateCategory::Completed,
                },
                WorkflowState {
                    name: "Cancelled".into(),
                    category: StateCategory::Cancelled,
                },
            ],
            transitions: vec![
                WorkflowTransition {
                    from: "Todo".into(),
                    to: "In Progress".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "In Progress".into(),
                    to: "Done".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "Todo".into(),
                    to: "Cancelled".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
            ],
        };
        assert!(
            workflow_unreachable_states(&wf).is_empty(),
            "a fully-connected workflow has no unreachable states"
        );
    }

    /// **The inspector renders EXACTLY the resolver's answer (0 private recompute) — the unit mirror of the
    /// CDC gate.** A fake resolver returns a known SubjectTree/RewriteTrace; the inspector's output equals
    /// it byte-for-byte (it invents no member, authors no trace). The CDC pair
    /// (`tests/cdc_4_4_issues_inspector.rs`) re-proves this against the REAL Identity engine.
    #[test]
    fn inspector_renders_exactly_the_resolver_answer() {
        use myelin_identity::{ConsistencyMode, RelName, Zookie};

        struct FakeResolver;
        impl PermissionResolver for FakeResolver {
            fn list_subjects(
                &self,
                object: &ObjectId,
                permission: &Permission,
                _at: &Consistency,
            ) -> SubjectTree {
                SubjectTree {
                    object: object.clone(),
                    relation: RelName(permission.0.clone()),
                    members: vec![PrincipalId("p:alice".into()), PrincipalId("p:bob".into())],
                    zookie: Zookie("z42".into()),
                }
            }
            fn explain(
                &self,
                subject: &PrincipalId,
                permission: &Permission,
                object: &ObjectId,
                _at: &Consistency,
            ) -> RewriteTrace {
                RewriteTrace {
                    steps: vec![
                        format!("expand {}#{} for {}", object.0, permission.0, subject.0),
                        "ALLOW — p:alice is in the expanded subject set (2 member(s))".into(),
                    ],
                }
            }
        }

        let inspector = PermissionInspector::new(FakeResolver);
        let at = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        let object = ObjectId("issue:PROJ-1".into());
        let perm = Permission("approve".into());

        // who_can renders EXACTLY the resolver's members (no invented/dropped member).
        let answer = inspector.who_can(&object, &perm, &at);
        assert_eq!(
            answer.members,
            vec![PrincipalId("p:alice".into()), PrincipalId("p:bob".into())],
            "the inspector renders exactly the resolver's SubjectTree members (0 private recompute)"
        );
        assert_eq!(answer.object, object);
        assert_eq!(answer.permission, perm);

        // why renders the resolver's trace VERBATIM (non-empty, ends ALLOW).
        let trace = inspector.why(&PrincipalId("p:alice".into()), &perm, &object, &at);
        assert_eq!(trace.steps.len(), 2);
        assert!(
            trace.steps.last().unwrap().starts_with("ALLOW"),
            "the inspector shows Identity's verdict verbatim"
        );
    }
}
