//! # `workflow` — the data-driven workflow FSM interpreter + the frozen-`QueryAst` guards (ISS-P12 / P-378, M4)
//!
//! **Owning architecture docs (byte-authoritative):**
//! - `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//!   §2 (the data-driven workflow FSM interpreter; the FIXED state-category set
//!   `unstarted/started/completed/cancelled`; the frozen `QueryAst` guards;
//!   required-fields-on-transition; the post-actions assign/set-field/link/arm-trigger; the
//!   `transition(issue, target, actor)` algorithm).
//! - Reconciliation `00-reconciliation-decisions.md` X-3 (the `QueryAst` as the ONE bounded guard
//!   predicate language — no UDFs/loops/recursion).
//! - VISION §1 (corporate workflows — governed transitions).
//! - EI-01 §7 (keep the architecture coherent — bounded guard language, no UDFs/loops/recursion;
//!   no second predicate language) + §5 (the flow-determinism lint ratchet).
//!
//! ## What ISS-P12 ships here — the BEHAVIOUR over the ISS-P11 `workflow`-scheme config
//! ISS-P11 (P-377, [`crate::schemes`]) shipped the five interpreted scheme kinds — including
//! [`crate::schemes::SchemeKind::Workflow`], whose JSONB `body` is
//! `{ states:[{name, category}], transitions:[{from, to, guard, post_actions}] }` — and the
//! deterministic, cached precedence algebra that RESOLVES which workflow scheme governs a given
//! `(type × project × team)` write context ([`crate::schemes::SchemeResolver::load_resolved`]).
//! ISS-P11 explicitly named THIS prompt as the interpreter that RUNS that workflow body.
//!
//! ISS-P12 ships:
//! - **The data-driven FSM interpreter** ([`Workflow`] / [`Workflow::plan_transition`]) over a config
//!   FSM — **not codegen** (you cannot recompile the binary per tenant) and **not user-scripting** (no
//!   Jira-Groovy footgun, EI-01 §7). A single interpreter over the resolved workflow body.
//! - **The FIXED state-category set** ([`StateCategory`] — `unstarted/started/completed/cancelled`)
//!   as the ONE mandatory governance invariant over unlimited admin-named [`WorkflowState`]s. Every
//!   state maps to exactly one of the four categories; the cross-subsystem rollup/SLA/board logic
//!   keys off the CATEGORY, never the open-ended name (arch 02 §2).
//! - **Guards as the frozen [`myelin_query::QueryAst`]** ([`WorkflowTransition::guards`]) — bounded,
//!   no UDFs/loops/recursion, statically cost-bounded, evaluated against an [`IssueContext`]. A failed
//!   guard blocks the transition with a **pre-assembled reason** ([`TransitionBlocked`]) — never a
//!   silent allow, never a silent drop.
//! - **Required-fields-on-transition** ([`WorkflowTransition::required_fields`]) — a transition into a
//!   state that demands a field absent on the issue is blocked with [`TransitionBlocked::MissingRequiredField`].
//! - **The post-actions** ([`PostAction`] — assign / set-field / link / arm-trigger) STAGED by a
//!   permitted transition, to co-commit on the ISS-P06 write transaction (arch 02 §2 `for a in
//!   t.post_actions: stage(a)`). The **arm-trigger** post-action carries an [`myelin_query::EventMatcher`]
//!   = the SAME frozen `QueryAst` core (contract 3.4) — one grammar, one cost model.
//!
//! ## The transition algorithm (arch 02 §2) — where the interpreter sits
//! The interpreter is the GOVERNANCE layer that decides WHETHER a transition is permitted and WHAT it
//! stages, BEFORE the ISS-P06 write path mutates the typed core + emits `issue.transitioned`:
//! ```text
//! transition(issue, target, actor):
//!   wf = resolve('workflow', type, project, team)   // ISS-P11, cached, off the hot path
//!   plan = wf.plan_transition(from, target, ctx)     // ← ISS-P12 (THIS module):
//!     find transition (from→to)                       //   ?? NoSuchTransition
//!     for g in guards: eval_guard(g, ctx)             //   ?? GuardFailed{reason}  (pre-assembled)
//!     for f in required_fields: present(issue, f)     //   ?? MissingRequiredField{f}
//!     → TransitionPlan{ to_category, post_actions }   //   the FIXED category + the staged actions
//!   // the transition ABAC (Id.check + the transition CaveatContext, 4.2) + the typed-core mutate +
//!   // OutboxTx::emit(issue.transitioned{from,to,category}) is the ISS-P06 write path (write_path.rs).
//! ```
//! The ReBAC + transition-`CaveatContext` ABAC (contract 4.2) is ALREADY wired on the ISS-P06 write
//! path ([`crate::write_path::apply_mutation`] runs `Id.check` with the transition `CaveatContext`
//! for a [`crate::write_path::MutationKind::Transition`]); the interpreter computes the plan the write
//! path then drives. The interpreter NEVER emits — emit is the ONE `OutboxTx::emit` verb (no-raw-publish).
//!
//! ## FLOOR named (prompt DoD)
//! - The **CI-red guard half of ISS-D12** — "can't mark Done while CI red on the linked PR" (reads the
//!   linked PR's frozen `CheckStatus` + the `trust_tier` off the fact, contract 5.9 / Δ10) — lands in
//!   **ISS-P27 (P-394)** when the X-1 check-seam closes. THIS prompt ships the guard-correctness half:
//!   "can't close while `blocked_by` an open issue" (a `QueryAst` over the `blocked_by` relation
//!   context) → transition blocked + the pre-assembled reason. The CI-status guard SHAPE is here
//!   (a `QueryAst` over a `linked_pr_check_status` context var); the live X-1 `CheckStatus` projection
//!   wiring is ISS-P27. Named at [`GuardVar::LINKED_PR_CHECK_STATUS`].
//!
//! ## Why interpreted + frozen-`QueryAst`, not scripting (EI-01 §7; arch 02 §2)
//! The same predicate language powers saved views, CLI filters, automation matchers, trigger
//! conditions, and SLA pause conditions — ONE grammar, four compile targets (OLTP, Search,
//! EventMatcher, Notif prefs; contract 3.4). One validator, one cost model, one
//! permission-aware-by-construction guarantee. A guard is authored in the S13 guard builder, not a
//! code editor; Issues defines no second guard language (the [`WorkflowTransition::guards`] are
//! `myelin_query::QueryAst`, the [`PostAction::ArmTrigger`] matcher is `myelin_query::EventMatcher` =
//! the same `QueryAst` core).

use myelin_identity::ObjectType;
use myelin_query::{EvalContext, EvalError, EventMatcher, Predicate, QueryAst};
use serde::{Deserialize, Serialize};

// =================================================================================================
// The FIXED state-category set — the ONE mandatory governance invariant (arch 02 §2).
// =================================================================================================

/// **The FIXED state-category set** (arch 02 §2 — the one mandatory governance invariant over
/// unlimited admin-named states). Every [`WorkflowState`], however an admin names it, maps to EXACTLY
/// one of these four categories. The cross-subsystem logic — the rollup, the SLA pause/resume, the
/// board column grouping, the "is this issue open?" predicate — keys off the CATEGORY, never the
/// open-ended state name. This is the closed set: a workflow scheme that declares a fifth category is
/// REJECTED at parse ([`WorkflowError::UnknownCategory`]); Issues defines no second category
/// vocabulary (EI-01 §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateCategory {
    /// Not yet picked up (e.g. "Todo", "Backlog", "Triage"). An OPEN category.
    Unstarted,
    /// In progress (e.g. "In Progress", "In Review", "Blocked"). An OPEN category.
    Started,
    /// Done — terminal-success (e.g. "Done", "Shipped", "Closed"). A CLOSED category.
    Completed,
    /// Won't-do — terminal-cancel (e.g. "Cancelled", "Duplicate", "Won't Fix"). A CLOSED category.
    Cancelled,
}

impl StateCategory {
    /// The exact `&str` the `workflow` scheme body's `category` field admits (the drift anchor: this
    /// list and the parse vocabulary are byte-identical — no second vocabulary, EI-01 §7).
    pub fn wire_token(self) -> &'static str {
        match self {
            StateCategory::Unstarted => "unstarted",
            StateCategory::Started => "started",
            StateCategory::Completed => "completed",
            StateCategory::Cancelled => "cancelled",
        }
    }

    /// The full, frozen, ordered four-category set (the closed set — a consumer asserts byte-identity
    /// over the WHOLE set, never a sampled subset).
    pub fn all() -> [StateCategory; 4] {
        [
            StateCategory::Unstarted,
            StateCategory::Started,
            StateCategory::Completed,
            StateCategory::Cancelled,
        ]
    }

    /// Parse a category token from the workflow-scheme body. An unknown token is REJECTED (a fifth
    /// category is not admissible — the fixed invariant).
    pub fn parse(token: &str) -> Result<StateCategory, WorkflowError> {
        StateCategory::all()
            .into_iter()
            .find(|c| c.wire_token() == token)
            .ok_or_else(|| WorkflowError::UnknownCategory {
                token: token.to_string(),
            })
    }

    /// Whether this category is **open** (`unstarted`/`started`) — the issue is still live. The
    /// `blocked_by an open issue` guard + the board "open lanes" key off this; the closed categories
    /// (`completed`/`cancelled`) are terminal.
    pub fn is_open(self) -> bool {
        matches!(self, StateCategory::Unstarted | StateCategory::Started)
    }
}

// =================================================================================================
// The workflow body — states + governed transitions (the ISS-P11 `workflow`-scheme `body` shape).
// =================================================================================================

/// **A named workflow state** (arch 02 §2). The admin names it freely; it maps to exactly one of the
/// FIXED [`StateCategory`] set. The `name` is the FSM node identity (the `issue.state` token); the
/// `category` is the cross-subsystem invariant the rollup/SLA/board read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowState {
    /// The admin-named state (the `issue.state` token, the FSM node id). Unlimited; e.g. "Todo".
    pub name: String,
    /// The FIXED category this state maps to (the one governance invariant — arch 02 §2).
    pub category: StateCategory,
}

/// **A governed transition** (arch 02 §2) — an FSM edge `from → to`, gated by zero-or-more frozen
/// [`QueryAst`] guards, demanding zero-or-more required fields on the target, and staging
/// zero-or-more [`PostAction`]s. A transition not declared here is [`TransitionBlocked::NoSuchTransition`]
/// (the FSM is closed — only declared edges are walkable).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkflowTransition {
    /// The source state name (the FSM `from`).
    pub from: String,
    /// The target state name (the FSM `to`).
    pub to: String,
    /// **The frozen-`QueryAst` guards** — ALL must hold (conjunction) for the transition to be
    /// permitted. Each is a bounded `myelin_query::QueryAst` predicate (no UDFs/loops/recursion,
    /// statically cost-bounded). A guard whose predicate is `false`, or references context the
    /// caller did not supply, BLOCKS the transition with a pre-assembled reason — never a silent
    /// allow (fail-closed, the `MissingContext` → block invariant, NOT → silent true).
    pub guards: Vec<WorkflowGuard>,
    /// **Required-fields-on-transition** (arch 02 §2) — the field ids that MUST be present (bound in
    /// the [`IssueContext`]) for the transition into `to`. A missing required field blocks with
    /// [`TransitionBlocked::MissingRequiredField`].
    pub required_fields: Vec<String>,
    /// **The post-actions** STAGED by a permitted transition (arch 02 §2 `for a in t.post_actions:
    /// stage(a)`). They co-commit on the ISS-P06 write transaction; the interpreter STAGES them (it
    /// never applies/emits — emit is the ONE `OutboxTx::emit` verb).
    pub post_actions: Vec<PostAction>,
}

/// **A frozen-`QueryAst` workflow guard** — a named guard carrying the bounded predicate + a
/// human-readable label the S13 builder authored. The label feeds the **pre-assembled reason** a
/// blocked transition returns (so the reason is deterministic + admin-authored, never a stringified
/// internal error). The predicate IS the frozen `myelin_query::QueryAst` (contract 13.3 / 3.4 — the
/// one engine; no second guard language).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkflowGuard {
    /// The admin-authored label (the S13 guard builder name; the pre-assembled reason prefix).
    pub label: String,
    /// The bounded predicate — the frozen [`QueryAst`] (no UDFs/loops/recursion, cost-bounded).
    pub predicate: QueryAst,
}

impl WorkflowGuard {
    /// Build a guard from a label + a compiled predicate tree (the in-process build path; the S13
    /// guard builder produces this). The predicate is validated against the static cost bound by the
    /// `QueryAst` constructor.
    pub fn compiled(
        label: impl Into<String>,
        predicate: Predicate,
    ) -> Result<WorkflowGuard, myelin_query::PredicateError> {
        Ok(WorkflowGuard {
            label: label.into(),
            predicate: QueryAst::compiled(predicate)?,
        })
    }
}

/// **A staged post-action** (arch 02 §2 `assign / set-field / link / arm-trigger`). A permitted
/// transition STAGES these to co-commit on the ISS-P06 write transaction; the interpreter does not
/// apply or emit them (emit is the ONE `OutboxTx::emit` verb — no-raw-publish).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PostAction {
    /// Assign the issue to a principal (the `assignee` relation — staged, co-commits via the write
    /// path's `write_tuples`).
    Assign {
        /// The assignee's OPAQUE pseudonym / principal token.
        assignee: String,
    },
    /// Set a typed/custom field to a value (a `props` JSONB write co-committing on the transaction).
    SetField {
        /// The field id (the `props` key, or a typed-core column).
        field_id: String,
        /// The value to set (the JSONB value).
        value: serde_json::Value,
    },
    /// Link this issue to another artifact (an `issue_relation` edge — co-commits on the transaction).
    Link {
        /// The relation kind (`blocks` / `relates` / `closes` …).
        relation: String,
        /// The target artifact ref token.
        target_ref: String,
    },
    /// **Arm a durable trigger** — the post-action that schedules a durable activity (e.g. an SLA
    /// timer, a follow-up reminder). It carries the **frozen [`EventMatcher`] = the SAME `QueryAst`
    /// core** (contract 3.4 — `EventMatcher = QueryAst`; the same bounded interpreter, one grammar,
    /// one cost model). The actual durable arming runs through `myelin-flow`'s deterministic `WfCtx`
    /// — the flow-determinism lint (1.6) holds on the arming body (see [`arm_trigger_body`]).
    ArmTrigger {
        /// The trigger name (the durable activity id).
        trigger: String,
        /// The event matcher the trigger fires on — the frozen `EventMatcher` (= `QueryAst`, 3.4).
        matcher: EventMatcher,
    },
}

/// **The resolved workflow** (arch 02 §2) — the parsed `workflow`-scheme `body` the interpreter runs.
/// Holds the named states (each mapped to a FIXED category) + the governed transitions. Parsed ONCE
/// from the resolved scheme body ([`Workflow::from_body`]) and interpreted at runtime — never compiled
/// into the binary, never user-scripted (EI-01 §7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workflow {
    /// The named states (each `{name, category}`). The FSM nodes.
    pub states: Vec<WorkflowState>,
    /// The governed transitions (the FSM edges).
    pub transitions: Vec<WorkflowTransition>,
}

impl Workflow {
    /// **The fixed-category invariant: the category of a named state** (arch 02 §2). Every state maps
    /// to exactly one of the four FIXED categories; an undeclared state name is rejected. This is the
    /// seam the write path stamps `issue.state_category` from on a permitted transition.
    pub fn category_of(&self, state: &str) -> Result<StateCategory, WorkflowError> {
        self.states
            .iter()
            .find(|s| s.name == state)
            .map(|s| s.category)
            .ok_or_else(|| WorkflowError::UnknownState {
                state: state.to_string(),
            })
    }

    /// **The data-driven FSM interpreter — plan a transition** (arch 02 §2). Given the current state
    /// `from`, the requested `target`, and the [`IssueContext`] the guards read, this:
    /// 1. finds the declared `from → target` transition (`?? NoSuchTransition` — the FSM is closed);
    /// 2. evaluates EVERY guard (conjunction) — a `false` / `MissingContext` / `TypeError` guard
    ///    BLOCKS with a **pre-assembled reason** (`?? GuardFailed{reason}`) — never a silent allow;
    /// 3. checks EVERY required field is present in the context (`?? MissingRequiredField{f}`);
    /// 4. returns the [`TransitionPlan`] — the FIXED target category (the invariant the write path
    ///    stamps) + the staged [`PostAction`]s.
    ///
    /// It does NOT mutate, check ABAC, or emit — that is the ISS-P06 write path
    /// ([`crate::write_path::apply_mutation`], which runs `Id.check` + the transition `CaveatContext`,
    /// mutates the typed core, and emits `issue.transitioned` on the ONE outbox transaction). The
    /// interpreter is the pure governance decision the write path then drives.
    pub fn plan_transition(
        &self,
        from: &str,
        target: &str,
        ctx: &IssueContext,
    ) -> Result<TransitionPlan, TransitionBlocked> {
        // 1. Find the declared transition (the FSM is closed — only declared edges are walkable).
        let t = self
            .transitions
            .iter()
            .find(|t| t.from == from && t.to == target)
            .ok_or_else(|| TransitionBlocked::NoSuchTransition {
                from: from.to_string(),
                to: target.to_string(),
            })?;

        // The target's FIXED category (the invariant). An undeclared target state is a config error
        // surfaced as a block (never an unstamped transition).
        let to_category =
            self.category_of(target)
                .map_err(|_| TransitionBlocked::NoSuchTransition {
                    from: from.to_string(),
                    to: target.to_string(),
                })?;

        // 2. Evaluate EVERY guard (conjunction). A guard that is false OR references context the
        //    caller did not supply BLOCKS — fail-closed, with a pre-assembled, admin-authored reason.
        for guard in &t.guards {
            match guard.predicate.eval(&ctx.attrs) {
                Ok(true) => {}
                // A defined-false guard: the governed condition is not met → block with the reason.
                Ok(false) => {
                    return Err(TransitionBlocked::GuardFailed {
                        reason: assemble_reason(&guard.label, from, target),
                    });
                }
                // Missing context / type error / cost / not-compiled — un-evaluable ⇒ fail-closed,
                // NEVER a silent allow (the `MissingContext` → block invariant, not → silent true).
                Err(e) => {
                    return Err(TransitionBlocked::GuardFailed {
                        reason: assemble_unevaluable_reason(&guard.label, from, target, &e),
                    });
                }
            }
        }

        // 3. Required-fields-on-transition — every required field must be present in the context.
        for f in &t.required_fields {
            if !ctx.has_field(f) {
                return Err(TransitionBlocked::MissingRequiredField { field: f.clone() });
            }
        }

        // 4. The permitted plan — the FIXED category the write path stamps + the staged post-actions.
        Ok(TransitionPlan {
            from: from.to_string(),
            to: target.to_string(),
            to_category,
            post_actions: t.post_actions.clone(),
        })
    }

    /// **Parse the resolved `workflow`-scheme `body` into a [`Workflow`]** (the ISS-P11 config →
    /// ISS-P12 interpreter seam). The `body` is the JSONB shape
    /// `{ states:[{name, category}], transitions:[{from, to, guards, required_fields, post_actions}] }`
    /// ([`crate::schemes::SchemeKind::Workflow`]). Rejects an unknown category (the fixed invariant),
    /// a transition referencing an undeclared state, or a duplicate transition edge. Parsed ONCE, off
    /// the hot path; interpreted at runtime.
    pub fn from_body(body: &serde_json::Value) -> Result<Workflow, WorkflowError> {
        let wf: Workflow =
            serde_json::from_value(body.clone()).map_err(|e| WorkflowError::Malformed {
                reason: e.to_string(),
            })?;
        wf.validate()?;
        Ok(wf)
    }

    /// Serialize the workflow back to the JSONB `body` shape (the round-trip the CDC pins).
    pub fn to_body(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("a Workflow always serializes")
    }

    /// **Validate the parsed workflow** — every transition references DECLARED states, no duplicate
    /// edge, every state's category is one of the FIXED four (enforced by the `serde` parse of
    /// [`StateCategory`]). A workflow failing this is a config error rejected at parse, NOT a runtime
    /// surprise (the S13 builder flags an unreachable-state / missing-category before save, arch 02 §2).
    pub fn validate(&self) -> Result<(), WorkflowError> {
        // Every transition's from/to is a declared state.
        for t in &self.transitions {
            self.category_of(&t.from)?;
            self.category_of(&t.to)?;
        }
        // No duplicate (from, to) edge (the FSM edge set is a function — one transition per pair).
        let mut seen = std::collections::BTreeSet::new();
        for t in &self.transitions {
            if !seen.insert((t.from.as_str(), t.to.as_str())) {
                return Err(WorkflowError::DuplicateTransition {
                    from: t.from.clone(),
                    to: t.to.clone(),
                });
            }
        }
        Ok(())
    }
}

// =================================================================================================
// The issue context the guards read (the `ctx{issue, linked_refs, actor}` of arch 02 §2).
// =================================================================================================

/// **The context a guard predicate evaluates against** (arch 02 §2 `ctx{issue, linked_refs, actor}`).
/// It binds the variables a guard reads: the issue's own attrs (severity, owner, …), the linked-ref
/// facts (the `blocked_by` open-count, a linked PR's `CheckStatus`), and the actor's attrs. A guard
/// referencing a variable NOT bound here surfaces as [`EvalError::MissingContext`] → the transition
/// is BLOCKED (fail-closed), never silently allowed.
///
/// The interpreter reads guards through the ONE [`myelin_query::EvalContext`] — the same bounded
/// interpreter Identity's caveat evaluator and the Bus matcher use (contract 3.4 / 4.2). The presence
/// of a field (for required-fields) is the same binding map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IssueContext {
    /// The variable bindings the guards read (the issue attrs + linked-ref facts + actor attrs).
    attrs: EvalContext,
    /// The set of field ids PRESENT on the issue (the required-fields check reads this; a field is
    /// present iff it is bound — but a field may be present-but-falsy, so we track presence
    /// explicitly rather than conflating it with a truthy guard binding).
    present_fields: std::collections::BTreeSet<String>,
}

impl IssueContext {
    /// An empty context (binds no variables, no present fields).
    pub fn new() -> IssueContext {
        IssueContext::default()
    }

    /// Bind a guard variable (builder style) — the value a guard's `Expr::Var` reads. Binding a
    /// variable also marks it PRESENT for the required-fields check (a bound field is present).
    pub fn bind(
        mut self,
        name: impl Into<String>,
        value: myelin_identity::Literal,
    ) -> IssueContext {
        let name = name.into();
        self.present_fields.insert(name.clone());
        self.attrs = std::mem::take(&mut self.attrs).bind(name, value);
        self
    }

    /// Mark a field PRESENT without binding a guard value (e.g. a free-text field that satisfies a
    /// required-field check but is not a guard operand). The required-fields check reads presence.
    pub fn mark_present(mut self, field_id: impl Into<String>) -> IssueContext {
        self.present_fields.insert(field_id.into());
        self
    }

    /// The bound-variable [`EvalContext`] the guards evaluate against.
    pub fn attrs(&self) -> &EvalContext {
        &self.attrs
    }

    /// Whether a required field is present on the issue (bound, or explicitly marked present).
    pub fn has_field(&self, field_id: &str) -> bool {
        self.present_fields.contains(field_id)
    }
}

/// **The well-known guard context variable names** (the `ctx{...}` keys, arch 02 §2). A guard's
/// `Expr::Var` reads one of these; the write path binds them from the issue + linked-ref facts before
/// it calls [`Workflow::plan_transition`]. Naming them as constants is the drift anchor between the
/// guard authors (S13) and the context binder (the write path).
pub struct GuardVar;

impl GuardVar {
    /// The number of OPEN issues that `block` this issue (a `blocks` edge from an open issue to here,
    /// arch 02 §2). The "can't close while `blocked_by` an open issue" guard reads
    /// `blocked_by_open_count == 0`. Bound by the write path from the `issue_rel_dst` index scan.
    pub const BLOCKED_BY_OPEN_COUNT: &'static str = "blocked_by_open_count";

    /// **FLOOR (ISS-P27 / P-394):** the linked PR's `CheckStatus` state token (the "can't mark Done
    /// while CI red" guard, contract 5.9 / Δ10). The guard reads `linked_pr_check_status == "success"`
    /// AND an acceptable `linked_pr_trust_tier`. THIS prompt ships the guard SHAPE (a `QueryAst` over
    /// these vars); the LIVE binding (the X-1 `project(ref, viewer)` `CheckStatus` read + the
    /// `trust_tier` off the fact) is ISS-P27 when the X-1 check-seam closes.
    pub const LINKED_PR_CHECK_STATUS: &'static str = "linked_pr_check_status";

    /// **FLOOR (ISS-P27 / P-394):** the linked PR's `trust_tier` (read off the fact, never recomputed
    /// — Δ10). The "can't mark Done while CI red" guard conjoins this with the check status.
    pub const LINKED_PR_TRUST_TIER: &'static str = "linked_pr_trust_tier";
}

// =================================================================================================
// The transition outcome — the permitted plan + the pre-assembled block reason (arch 02 §2).
// =================================================================================================

/// **A permitted transition plan** (arch 02 §2) — the governance decision the ISS-P06 write path
/// drives. It carries the FIXED target [`StateCategory`] (the invariant the write path stamps onto
/// `issue.state_category` + the `issue.transitioned{from,to,category}` event) + the staged
/// [`PostAction`]s (which co-commit on the write transaction). The interpreter produces this; it does
/// NOT mutate or emit.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionPlan {
    /// The source state (the FSM `from`).
    pub from: String,
    /// The target state (the FSM `to`).
    pub to: String,
    /// **The FIXED target category** (the one governance invariant — the write path stamps this onto
    /// `issue.state_category` + the `issue.transitioned` event's `category`).
    pub to_category: StateCategory,
    /// The post-actions to STAGE on the write transaction (assign / set-field / link / arm-trigger).
    pub post_actions: Vec<PostAction>,
}

/// **Why a transition was BLOCKED** (arch 02 §2 — loud, never a silent allow). Every variant carries
/// a deterministic, pre-assembled reason the caller surfaces to the actor (and an agent's gated tool
/// returns) — never a stringified internal error, never a silent no-op. This is the green artifact of
/// the ISS-D12 guard half ("can't close while `blocked_by` an open issue" → blocked + reason).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransitionBlocked {
    /// No declared `from → to` edge in the resolved workflow (the FSM is closed). The actor tried an
    /// undeclared transition.
    NoSuchTransition {
        /// the attempted source state.
        from: String,
        /// the attempted target state.
        to: String,
    },
    /// A guard's bounded `QueryAst` predicate did not hold (or was un-evaluable). Carries the
    /// **pre-assembled, admin-authored reason** (the guard's S13 label + the from→to context).
    GuardFailed {
        /// the pre-assembled reason string (deterministic, admin-authored).
        reason: String,
    },
    /// A required field for the target state was absent on the issue.
    MissingRequiredField {
        /// the missing field id.
        field: String,
    },
}

impl TransitionBlocked {
    /// The pre-assembled human-readable reason for ANY block variant (the one reason surface the
    /// caller / agent renders). Deterministic — the SAME inputs always produce the SAME string.
    pub fn reason(&self) -> String {
        match self {
            TransitionBlocked::NoSuchTransition { from, to } => {
                format!("no transition from `{from}` to `{to}` in this workflow")
            }
            TransitionBlocked::GuardFailed { reason } => reason.clone(),
            TransitionBlocked::MissingRequiredField { field } => {
                format!("the field `{field}` is required before this transition")
            }
        }
    }
}

impl std::fmt::Display for TransitionBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason())
    }
}

impl std::error::Error for TransitionBlocked {}

/// **A workflow config error** (the parse/validate surface — a malformed `workflow`-scheme body is
/// rejected at parse, off the hot path, NOT a runtime surprise; arch 02 §2 "the S13 builder flags an
/// unreachable-state / missing-category before save").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowError {
    /// The `body` did not parse into the `{states, transitions}` shape.
    Malformed {
        /// the serde error detail.
        reason: String,
    },
    /// A `category` token is not one of the FIXED four (a fifth category is inadmissible).
    UnknownCategory {
        /// the rejected token.
        token: String,
    },
    /// A transition references a state not declared in `states`.
    UnknownState {
        /// the undeclared state.
        state: String,
    },
    /// Two transitions share the same `(from, to)` edge (the FSM edge set must be a function).
    DuplicateTransition {
        /// the duplicated source.
        from: String,
        /// the duplicated target.
        to: String,
    },
}

impl std::fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkflowError::Malformed { reason } => {
                write!(f, "malformed workflow scheme body: {reason}")
            }
            WorkflowError::UnknownCategory { token } => write!(
                f,
                "unknown state category `{token}` (the fixed set is unstarted/started/completed/cancelled)"
            ),
            WorkflowError::UnknownState { state } => {
                write!(f, "transition references undeclared state `{state}`")
            }
            WorkflowError::DuplicateTransition { from, to } => {
                write!(f, "duplicate transition `{from}` → `{to}`")
            }
        }
    }
}

impl std::error::Error for WorkflowError {}

/// Pre-assemble the deterministic reason for a defined-false guard (the S13 label + the from→to
/// context). Admin-authored, never a stringified internal error.
fn assemble_reason(label: &str, from: &str, to: &str) -> String {
    format!("cannot transition `{from}` → `{to}`: {label}")
}

/// Pre-assemble the reason for an UN-EVALUABLE guard (missing context / type error / cost / not
/// compiled). Fail-closed: an un-evaluable guard is uncertainty → block, with a reason that names the
/// guard + the un-evaluable cause (never a silent allow).
fn assemble_unevaluable_reason(label: &str, from: &str, to: &str, err: &EvalError) -> String {
    format!("cannot transition `{from}` → `{to}`: {label} (guard could not be evaluated: {err})")
}

// =================================================================================================
// The arm-trigger post-action body — the flow-determinism seam (lint 1.6).
// =================================================================================================

/// **Arm the durable trigger a transition's [`PostAction::ArmTrigger`] staged — a `myelin-flow`
/// workflow body** (arch 02 §2; the flow-determinism lint, contract 1.6).
///
// @workflow-body — this fn is a deterministic `myelin-flow` workflow body. It reads time/rand/IO
// ONLY through the `WfCtx` surface (`ctx.now()` / `ctx.activity(...)`), NEVER a raw clock/rng/IO call
// — so the workflow replays deterministically (the flow-determinism lint, index 9.2/OQ-F). The
// arm-trigger post-action schedules a durable activity; arming it through `WfCtx` is the determinism
// floor the lint guards (a raw `SystemTime::now()` here would make replay diverge).
///
/// The `matcher` is the frozen [`EventMatcher`] = the SAME `QueryAst` core (contract 3.4) the arming
/// fires on. The actual durable scheduling (the SLA timer / reminder) lands on `myelin-flow`'s
/// stateful `Trigger` (ISS-P25); here the body is the determinism-clean staging seam — it derives the
/// fire token from the deterministic context, never a raw clock read.
pub fn arm_trigger_body(
    ctx_now_seconds: i64,
    trigger: &str,
    matcher: &EventMatcher,
) -> ArmedTrigger {
    // ctx_now_seconds is `ctx.now()` (the deterministic WfCtx clock — NOT SystemTime::now()): a
    // workflow body reads time through WfCtx so replay is deterministic (flow-determinism, 1.6).
    ArmedTrigger {
        trigger: trigger.to_string(),
        armed_at_seconds: ctx_now_seconds,
        matcher: matcher.clone(),
    }
}

/// The result of arming a trigger (the deterministic staging artifact — the durable scheduling is
/// ISS-P25). It carries the fire token derived from the deterministic `WfCtx` clock (never a raw
/// clock read) + the frozen `EventMatcher` the trigger fires on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArmedTrigger {
    /// The trigger name (the durable activity id).
    pub trigger: String,
    /// The arm time in SECONDS (the EventEnvelope unit anchor, contract 2.1) — from `ctx.now()`.
    pub armed_at_seconds: i64,
    /// The frozen `EventMatcher` (= `QueryAst`, 3.4) the trigger fires on.
    pub matcher: EventMatcher,
}

/// Build the canonical "can't close while `blocked_by` an open issue" guard (the ISS-D12 guard-half
/// witness, arch 02 §2). The guard is a frozen `QueryAst`: `blocked_by_open_count == 0`. A non-zero
/// open-blocker count makes the guard FALSE → the transition is blocked with the pre-assembled reason.
/// This is the canonical guard the drill + the e2e exercise.
pub fn blocked_by_guard() -> WorkflowGuard {
    use myelin_identity::Literal;
    use myelin_query::{CmpOp, Expr};
    WorkflowGuard::compiled(
        "this issue is blocked by an open issue",
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(GuardVar::BLOCKED_BY_OPEN_COUNT.into()),
            rhs: Expr::Lit(Literal::Int(0)),
        },
    )
    .expect("the blocked_by guard is a single bounded comparison (within the cost bound)")
}

/// Build the CI-status guard SHAPE (the ISS-P27 floor witness, arch 02 §2 / contract 5.9 / Δ10):
/// `linked_pr_check_status == "success"`. THIS prompt ships the guard SHAPE (a frozen `QueryAst` over
/// the linked-PR context vars); the LIVE binding (the X-1 `CheckStatus` projection + `trust_tier` off
/// the fact) is ISS-P27 when the X-1 check-seam closes. Until then this guard blocks fail-closed when
/// the context is unbound (the linked-PR status is genuine uncertainty, never a silent allow).
pub fn linked_pr_ci_green_guard() -> WorkflowGuard {
    use myelin_identity::Literal;
    use myelin_query::{CmpOp, Expr};
    WorkflowGuard::compiled(
        "the linked PR's CI is not green",
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var(GuardVar::LINKED_PR_CHECK_STATUS.into()),
            rhs: Expr::Lit(Literal::Str("success".into())),
        },
    )
    .expect("the CI-status guard is a single bounded comparison (within the cost bound)")
}

/// Build a minimal example arm-trigger post-action (an SLA-timer arm fired on the issue's own
/// `issue.transitioned` event). The matcher is the frozen `EventMatcher` (= `QueryAst`, 3.4). Used by
/// the unit tests + as the canonical post-action shape.
pub fn example_arm_trigger(trigger: impl Into<String>) -> PostAction {
    let matcher = EventMatcher::new(ObjectType("issue".into()), QueryAst::raw("state == 'Done'"));
    PostAction::ArmTrigger {
        trigger: trigger.into(),
        matcher,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::Literal;
    use myelin_query::{CmpOp, Expr};

    /// A 3-state Linear-simple workflow (the no-config default shape): Todo (unstarted) → In Progress
    /// (started) → Done (completed), with a Cancel edge to Cancelled.
    fn simple_workflow() -> Workflow {
        Workflow {
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
                    guards: vec![blocked_by_guard()],
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
        }
    }

    /// **The FIXED state-category set is the closed four** (arch 02 §2 — the one governance
    /// invariant). The tokens are byte-identical to the parse vocabulary (no second vocabulary).
    #[test]
    fn the_state_category_set_is_the_fixed_four() {
        let tokens: Vec<&str> = StateCategory::all()
            .iter()
            .map(|c| c.wire_token())
            .collect();
        assert_eq!(
            tokens,
            vec!["unstarted", "started", "completed", "cancelled"],
            "the fixed four categories (the one mandatory invariant)"
        );
        // A fifth category is inadmissible (rejected at parse — never a runtime surprise).
        assert_eq!(
            StateCategory::parse("in_review"),
            Err(WorkflowError::UnknownCategory {
                token: "in_review".into()
            }),
            "a fifth category is rejected (the fixed invariant)"
        );
        // The open/closed partition: unstarted/started are open, completed/cancelled terminal.
        assert!(StateCategory::Unstarted.is_open());
        assert!(StateCategory::Started.is_open());
        assert!(!StateCategory::Completed.is_open());
        assert!(!StateCategory::Cancelled.is_open());
    }

    /// **Every named state maps to exactly one FIXED category** (the invariant the write path stamps).
    #[test]
    fn every_state_maps_to_a_fixed_category() {
        let wf = simple_workflow();
        assert_eq!(wf.category_of("Todo").unwrap(), StateCategory::Unstarted);
        assert_eq!(
            wf.category_of("In Progress").unwrap(),
            StateCategory::Started
        );
        assert_eq!(wf.category_of("Done").unwrap(), StateCategory::Completed);
        assert_eq!(
            wf.category_of("Cancelled").unwrap(),
            StateCategory::Cancelled
        );
        // An undeclared state has no category (rejected, never an unstamped transition).
        assert_eq!(
            wf.category_of("Nope"),
            Err(WorkflowError::UnknownState {
                state: "Nope".into()
            })
        );
    }

    /// **A permitted transition returns the FIXED target category + staged post-actions** (the plan
    /// the write path drives). The unguarded Todo → In Progress is permitted; the plan stamps Started.
    #[test]
    fn a_permitted_transition_returns_the_fixed_category() {
        let wf = simple_workflow();
        let plan = wf
            .plan_transition("Todo", "In Progress", &IssueContext::new())
            .expect("the unguarded transition is permitted");
        assert_eq!(
            plan.to_category,
            StateCategory::Started,
            "the FIXED category"
        );
        assert_eq!(plan.from, "Todo");
        assert_eq!(plan.to, "In Progress");
    }

    /// **An undeclared transition is blocked (the FSM is closed)** — only declared edges are walkable.
    #[test]
    fn an_undeclared_transition_is_blocked() {
        let wf = simple_workflow();
        let blocked = wf
            .plan_transition("Todo", "Done", &IssueContext::new())
            .expect_err("Todo → Done is not a declared edge");
        assert_eq!(
            blocked,
            TransitionBlocked::NoSuchTransition {
                from: "Todo".into(),
                to: "Done".into()
            }
        );
    }

    /// **The ISS-D12 guard half: can't close while `blocked_by` an open issue → blocked + reason.**
    /// The In Progress → Done transition guards on `blocked_by_open_count == 0`. With 1 open blocker
    /// the guard is FALSE → the transition is blocked with the pre-assembled, admin-authored reason
    /// (the green artifact). With 0 blockers the guard holds → permitted.
    #[test]
    fn cannot_close_while_blocked_by_an_open_issue() {
        let wf = simple_workflow();

        // 1 open blocker → the guard is false → blocked + the pre-assembled reason.
        let ctx_blocked =
            IssueContext::new().bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(1));
        let blocked = wf
            .plan_transition("In Progress", "Done", &ctx_blocked)
            .expect_err("a transition with an open blocker is blocked");
        match &blocked {
            TransitionBlocked::GuardFailed { reason } => {
                assert!(
                    reason.contains("blocked by an open issue"),
                    "the reason names the guard: {reason}"
                );
                assert!(
                    reason.contains("In Progress") && reason.contains("Done"),
                    "the reason names the from→to context: {reason}"
                );
            }
            other => panic!("expected GuardFailed, got {other:?}"),
        }
        // The reason() surface is deterministic + non-empty.
        assert!(!blocked.reason().is_empty());

        // 0 open blockers → the guard holds → permitted (the category is stamped Completed).
        let ctx_clear = IssueContext::new().bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(0));
        let plan = wf
            .plan_transition("In Progress", "Done", &ctx_clear)
            .expect("with no open blocker the transition is permitted");
        assert_eq!(plan.to_category, StateCategory::Completed);
    }

    /// **A guard that references unbound context BLOCKS fail-closed (never a silent allow).** The
    /// `blocked_by` guard reads `blocked_by_open_count`; if the caller does not bind it, the guard is
    /// un-evaluable → the transition is BLOCKED with a reason (the `MissingContext` → block invariant,
    /// NOT → silent true). This is the fail-closed posture the X-1 CI-status floor relies on.
    #[test]
    fn an_unbound_guard_blocks_fail_closed() {
        let wf = simple_workflow();
        // The context does NOT bind blocked_by_open_count.
        let blocked = wf
            .plan_transition("In Progress", "Done", &IssueContext::new())
            .expect_err("an un-evaluable guard fails closed (blocks)");
        match &blocked {
            TransitionBlocked::GuardFailed { reason } => {
                assert!(
                    reason.contains("could not be evaluated"),
                    "the reason names the un-evaluable cause: {reason}"
                );
            }
            other => panic!("expected GuardFailed (fail-closed), got {other:?}"),
        }
    }

    /// **Required-fields-on-transition: a missing required field blocks the transition.** A transition
    /// demanding `resolution` blocks when the issue has no `resolution`, and is permitted once present.
    #[test]
    fn a_missing_required_field_blocks_the_transition() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "Open".into(),
                    category: StateCategory::Started,
                },
                WorkflowState {
                    name: "Resolved".into(),
                    category: StateCategory::Completed,
                },
            ],
            transitions: vec![WorkflowTransition {
                from: "Open".into(),
                to: "Resolved".into(),
                guards: vec![],
                required_fields: vec!["resolution".into()],
                post_actions: vec![],
            }],
        };
        // No `resolution` field present → blocked.
        let blocked = wf
            .plan_transition("Open", "Resolved", &IssueContext::new())
            .expect_err("a missing required field blocks");
        assert_eq!(
            blocked,
            TransitionBlocked::MissingRequiredField {
                field: "resolution".into()
            }
        );
        assert!(blocked.reason().contains("resolution"));
        // With the field present → permitted.
        let ctx = IssueContext::new().mark_present("resolution");
        let plan = wf
            .plan_transition("Open", "Resolved", &ctx)
            .expect("the required field is present");
        assert_eq!(plan.to_category, StateCategory::Completed);
    }

    /// **Post-actions fire (are staged) on a permitted transition.** A permitted transition returns
    /// its staged post-actions (assign / set-field / link / arm-trigger) for the write path to
    /// co-commit; a blocked transition stages NOTHING.
    #[test]
    fn post_actions_fire_on_a_permitted_transition() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "Todo".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: "Doing".into(),
                    category: StateCategory::Started,
                },
            ],
            transitions: vec![WorkflowTransition {
                from: "Todo".into(),
                to: "Doing".into(),
                guards: vec![],
                required_fields: vec![],
                post_actions: vec![
                    PostAction::Assign {
                        assignee: "alice@acme.noreply".into(),
                    },
                    PostAction::SetField {
                        field_id: "started_at".into(),
                        value: serde_json::json!(1000),
                    },
                    example_arm_trigger("sla_timer"),
                ],
            }],
        };
        let plan = wf
            .plan_transition("Todo", "Doing", &IssueContext::new())
            .expect("permitted");
        assert_eq!(plan.post_actions.len(), 3, "all three post-actions staged");
        assert!(matches!(
            &plan.post_actions[0],
            PostAction::Assign { assignee } if assignee == "alice@acme.noreply"
        ));
        assert!(
            matches!(&plan.post_actions[2], PostAction::ArmTrigger { trigger, .. } if trigger == "sla_timer")
        );
    }

    /// **The workflow body parses from + serializes back to the JSONB `body` shape (the ISS-P11
    /// config → ISS-P12 interpreter seam).** The `{states, transitions}` JSONB round-trips
    /// byte-identically — the config shape the CDC pins (a config write, never a bespoke object graph).
    #[test]
    fn the_workflow_body_round_trips() {
        let wf = simple_workflow();
        let body = wf.to_body();
        // The body carries the fixed-category tokens + the state names.
        assert!(body.to_string().contains("\"unstarted\""));
        assert!(body.to_string().contains("\"Todo\""));
        let back = Workflow::from_body(&body).expect("the body parses back");
        assert_eq!(back, wf, "the workflow body round-trips byte-identically");
    }

    /// **An unknown category in the body is rejected at parse (the fixed invariant).** A `body`
    /// declaring a fifth category fails `from_body` — never silently interpreted.
    #[test]
    fn an_unknown_category_in_the_body_is_rejected() {
        let body = serde_json::json!({
            "states": [{"name": "Review", "category": "in_review"}],
            "transitions": []
        });
        let err = Workflow::from_body(&body).expect_err("a fifth category is rejected");
        assert!(matches!(err, WorkflowError::Malformed { .. }));
    }

    /// **A transition referencing an undeclared state is rejected at parse/validate.** The FSM edges
    /// must reference declared states (the S13 builder flags this before save).
    #[test]
    fn a_transition_to_an_undeclared_state_is_rejected() {
        let wf = Workflow {
            states: vec![WorkflowState {
                name: "Todo".into(),
                category: StateCategory::Unstarted,
            }],
            transitions: vec![WorkflowTransition {
                from: "Todo".into(),
                to: "Ghost".into(),
                guards: vec![],
                required_fields: vec![],
                post_actions: vec![],
            }],
        };
        assert_eq!(
            wf.validate(),
            Err(WorkflowError::UnknownState {
                state: "Ghost".into()
            })
        );
    }

    /// **A duplicate `(from, to)` edge is rejected (the FSM edge set is a function).**
    #[test]
    fn a_duplicate_transition_edge_is_rejected() {
        let wf = Workflow {
            states: vec![
                WorkflowState {
                    name: "A".into(),
                    category: StateCategory::Unstarted,
                },
                WorkflowState {
                    name: "B".into(),
                    category: StateCategory::Started,
                },
            ],
            transitions: vec![
                WorkflowTransition {
                    from: "A".into(),
                    to: "B".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
                WorkflowTransition {
                    from: "A".into(),
                    to: "B".into(),
                    guards: vec![],
                    required_fields: vec![],
                    post_actions: vec![],
                },
            ],
        };
        assert_eq!(
            wf.validate(),
            Err(WorkflowError::DuplicateTransition {
                from: "A".into(),
                to: "B".into()
            })
        );
    }

    /// **The guard is the frozen `myelin_query::QueryAst` — no second guard language** (contract 13.3
    /// / 3.4). A guard predicate is a `Predicate` tree evaluated by the ONE interpreter; the arm-trigger
    /// post-action carries the `EventMatcher` = the SAME `QueryAst` core. A guard within the cost bound
    /// builds; an over-budget tree is rejected by the `QueryAst` constructor (the one cost model).
    #[test]
    fn the_guard_is_the_frozen_query_ast() {
        // A guard with a compound bounded predicate (severity high AND not blocked).
        let guard = WorkflowGuard::compiled(
            "high-severity issues need no open blocker",
            Predicate::And(vec![
                Predicate::Cmp {
                    op: CmpOp::Ge,
                    lhs: Expr::Var("severity".into()),
                    rhs: Expr::Lit(Literal::Int(3)),
                },
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: Expr::Var(GuardVar::BLOCKED_BY_OPEN_COUNT.into()),
                    rhs: Expr::Lit(Literal::Int(0)),
                },
            ]),
        )
        .expect("a bounded compound guard builds");
        // It evaluates through the ONE interpreter.
        let ctx = EvalContext::new()
            .bind("severity", Literal::Int(5))
            .bind(GuardVar::BLOCKED_BY_OPEN_COUNT, Literal::Int(0));
        assert_eq!(guard.predicate.eval(&ctx), Ok(true));
        // The arm-trigger matcher is the same QueryAst core (EventMatcher = QueryAst, 3.4).
        let action = example_arm_trigger("t");
        assert!(matches!(action, PostAction::ArmTrigger { .. }));
    }

    /// **The arm-trigger workflow body reads time through `WfCtx` (the flow-determinism floor).** The
    /// body derives its fire token from the deterministic `ctx.now()` (never a raw clock read) — so
    /// replay is deterministic (flow-determinism lint, contract 1.6).
    #[test]
    fn the_arm_trigger_body_is_determinism_clean() {
        let matcher = EventMatcher::new(ObjectType("issue".into()), QueryAst::raw("x == 1"));
        // ctx_now is `ctx.now()` (the WfCtx clock — deterministic, replayable).
        let armed = arm_trigger_body(1_700_000_000, "sla_timer", &matcher);
        assert_eq!(armed.armed_at_seconds, 1_700_000_000);
        assert_eq!(armed.trigger, "sla_timer");
        // Replaying with the SAME ctx.now() produces the SAME armed token (determinism).
        let replay = arm_trigger_body(1_700_000_000, "sla_timer", &matcher);
        assert_eq!(armed, replay, "the workflow body replays deterministically");
    }

    /// **The CI-status guard SHAPE is present (the ISS-P27 floor) and fails closed when unbound.** The
    /// "can't mark Done while CI red" guard is a frozen `QueryAst` over `linked_pr_check_status`; until
    /// the X-1 binding lands (ISS-P27) it blocks fail-closed when the linked-PR status is unbound.
    #[test]
    fn the_ci_status_guard_shape_fails_closed_until_iss_p27() {
        let guard = linked_pr_ci_green_guard();
        // Unbound linked-PR status → un-evaluable → fail-closed (block).
        assert_eq!(
            guard.predicate.eval(&EvalContext::new()),
            Err(EvalError::MissingContext {
                name: GuardVar::LINKED_PR_CHECK_STATUS.into()
            }),
            "until ISS-P27 binds the X-1 CheckStatus, the guard fails closed (never a silent allow)"
        );
        // The shape is real: a bound success status holds; a red status does not.
        let green = EvalContext::new().bind(
            GuardVar::LINKED_PR_CHECK_STATUS,
            Literal::Str("success".into()),
        );
        assert_eq!(guard.predicate.eval(&green), Ok(true));
        let red = EvalContext::new().bind(
            GuardVar::LINKED_PR_CHECK_STATUS,
            Literal::Str("failure".into()),
        );
        assert_eq!(guard.predicate.eval(&red), Ok(false));
    }

    /// **`IssueContext::attrs()` exposes the bound `EvalContext` the guards read** — a bound variable
    /// is visible through the accessor (the seam the write path / a guard evaluator reads).
    #[test]
    fn the_issue_context_exposes_its_bound_attrs() {
        let ctx = IssueContext::new().bind("severity", Literal::Int(7));
        // The accessor returns the SAME context a guard evaluates against (non-empty bindings).
        let ast = WorkflowGuard::compiled(
            "sev",
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var("severity".into()),
                rhs: Expr::Lit(Literal::Int(7)),
            },
        )
        .unwrap();
        assert_eq!(
            ast.predicate.eval(ctx.attrs()),
            Ok(true),
            "the attrs() accessor returns the bound guard context"
        );
    }

    /// **The block + config errors `Display` to their pre-assembled reasons** (the one human-readable
    /// surface — deterministic, non-empty). A `Display` regression (e.g. an empty render) is caught.
    #[test]
    fn the_error_displays_carry_their_reasons() {
        let blocked = TransitionBlocked::MissingRequiredField {
            field: "resolution".into(),
        };
        let shown = format!("{blocked}");
        assert!(
            shown.contains("resolution") && shown == blocked.reason(),
            "Display == reason(), naming the field: {shown}"
        );

        let nost = TransitionBlocked::NoSuchTransition {
            from: "A".into(),
            to: "B".into(),
        };
        assert!(format!("{nost}").contains("no transition from `A` to `B`"));

        let cfg = WorkflowError::UnknownCategory {
            token: "weird".into(),
        };
        assert!(
            format!("{cfg}").contains("weird") && format!("{cfg}").contains("unstarted/started"),
            "the config error names the bad token + the fixed set"
        );
        let dup = WorkflowError::DuplicateTransition {
            from: "X".into(),
            to: "Y".into(),
        };
        assert!(format!("{dup}").contains("duplicate transition `X` → `Y`"));
        let unk = WorkflowError::UnknownState { state: "Z".into() };
        assert!(format!("{unk}").contains("undeclared state `Z`"));
        let mal = WorkflowError::Malformed {
            reason: "bad".into(),
        };
        assert!(format!("{mal}").contains("bad"));
    }
}
