//! # `hitl` — the withhold → surface → resume loop + the `hitl_gate` state machine
//!
//! HITL (human-in-the-loop): **withhold** (a gated effect returns `Gated`, does not mutate) →
//! **surface** (a durable-workflow wait, surfaced as a chat approval card showing the pending
//! action + risk + the **LIVE cost estimate**, with the approver set = `list_subjects(object,
//! approve_perm)`) → **decide** (minutes or days; the wait holds no runtime) → **resume** (the
//! workflow signal re-runs the step with the tool name added to "approved"; step 6 of the pipeline
//! now passes; the effect applies). Rejection settles `Halted::Rejected` with the reason in the
//! trace + audit.
//!
//! The `hitl_gate` row: `gate_id`, `run_id`, `effect_id`, `risk_summary`, `cost_estimate`,
//! `approver_filter`, `state`, `card_ref`. The durable HITL signal holds NO runtime while
//! `state=waiting`; an approval/cancel signal arrives hours/days later, re-leases + replays +
//! consumes — that machinery is `myelin-flow`'s `request_approval_and_wait` (reconciled below);
//! `list_subjects` → `SubjectTree` supplies the approver set.
//!
//! ## The machinery on top of `EffectApi::apply`'s `Gated`
//!
//! [`crate::effect_api::PlanThenApply`] already returns
//! [`EffectResult::Gated`](myelin_agent::EffectResult) at step 6 when a `requires_approval` tool is
//! not yet in the run's `approved` set — it opens nothing, it only signals the gate (a withheld
//! gated effect does NOT mutate). THIS module is the machinery that **resumes** that result:
//!
//! 1. **WITHHOLD → OPEN A GATE.** [`open_gate`] turns a `Gated` verdict + the [`PlannedEffect`] into a
//!    [`HitlGate`] row in state [`HitlGateState::Waiting`] — carrying the humanised `risk_summary`
//!    slot, the LIVE `cost_estimate`, the `approver_filter` (= `list_subjects(object, approve_perm)`),
//!    and the `card_ref` the durable wait surfaces as. A withheld gate holds **no runtime**.
//! 2. **SURFACE.** [`surface_card`] projects the gate into a [`HitlCard`] — the pending action, the
//!    risk, the LIVE cost estimate, and the approver set — the chat approval card a viewer sees. The
//!    durable wait itself (`myelin_flow::approval::request_approval_and_wait`) is `myelin-flow`'s,
//!    consumed via the [`HitlWait`] seam (no production dep — the DAG stays acyclic).
//! 3. **DECIDE.** A human clicks Approve/Reject minutes-or-days later. The run is PARKED
//!    (`state=waiting`) holding no runtime; the worker is free. The decide step is observational here
//!    — the durable wait does the multi-day park.
//! 4. **RESUME.** [`HitlGate::approve`] transitions `Waiting → Approved` and adds the effect's
//!    per-(tool, object) gate key ([`crate::effect_api::effect_gate_key`] — never the bare tool
//!    name) to the run's `approved` set ([`ApprovedTools`]); a re-run of
//!    [`crate::effect_api::PlanThenApply::apply_planned`] now passes step 6 for THAT effect and
//!    applies. [`HitlGate::reject`] transitions `Waiting → Rejected` and settles [`Halted::Rejected`]
//!    with the reason in the trace + audit — the effect is never applied (0 mutation).
//!
//! ## The state machine ([`HitlGateState`])
//!
//! ```text
//!                approve(tool)        ┌──────────┐
//!   ┌─────────┐ ───────────────────▶ │ Approved │  (re-run step 6 passes → APPLIES)
//!   │ Waiting │                       └──────────┘
//!   └─────────┘ ───────────────────▶ ┌──────────┐
//!        │       reject(reason)       │ Rejected │  (Halted::Rejected, reason in trace; 0 mutation)
//!        │ expire(deadline)           └──────────┘
//!        ▼
//!   ┌──────────┐
//!   │ Expired  │  (the approval window lapsed → auto-deny; 0 mutation)
//!   └──────────┘
//! ```
//!
//! Every transition is from `Waiting` ONLY (a terminal gate never re-transitions — a double-click on
//! an already-decided gate is a no-op, [`HitlGate::approve`]/[`reject`] return `Err` on a non-Waiting
//! gate). The **0-mutation-pre-approval** invariant is structural: the gate carries the effect but
//! NEVER applies it — the apply happens only when the re-run of the pipeline (with the tool now in
//! `approved`) reaches step 7. A Rejected/Expired gate never adds the tool to `approved`, so the
//! re-run gates again (or the loop withholds it) — the declined effect makes ZERO mutation.
//!
//! ## Reconciliation with the durable-workflow HITL round-trip (`myelin-flow::approval`)
//!
//! The **durable wait** half of the loop — emit `agent.approval.requested` via the outbox ONCE → park
//! `state=waiting` holding no runtime → resume on the `approval` signal days later, consume-exactly-
//! once across a re-drive, the per-effect `idem_key` rule — already lives in `myelin-flow::approval`
//! (`request_approval_and_wait`, `per_effect_idem_key`). THIS module does NOT re-implement it: it is
//! the **agent-fabric side** the durable wait drives — the `hitl_gate` state machine, the card
//! projection (action + risk + LIVE cost), the approver-set derivation (`list_subjects`), and the
//! resume that threads the tool into the run's `approved` set so the `EffectApi` step passes. The
//! durable wait is consumed through the [`HitlWait`] seam (a caller-supplied driver), keeping
//! `myelin-agent-service` free of a production dep on `myelin-flow`.
//!
//! ## Follow-on floors
//! - **Per-effect resume idempotency.** A batch card gating MULTIPLE effects keys each on
//!   `card_id ":" effect_idx`; a partial approval + double-click is well-defined exactly-once. The
//!   per-effect key rule itself lives in `per_effect_idem_key`. HERE the gate is single-effect (one
//!   gate per withheld effect) and the `card_id` is the single-effect key (the degenerate per-effect
//!   case).
//! - **The humanise card-text surface.** The `risk_summary` is a [`RiskSummary`] = `(template_key,
//!   args)` pair (NOT a raw string); Notif `humanise` (the ONE templating surface) renders it. HERE
//!   the card carries the humanised SLOT; the render is a follow-on.
//! - **Implicit auto-dispatch on a casual mention** is handled at the Chat dispatch layer, not here
//!   (a mention notifies; it does not auto-spawn a costed run).

use crate::effect_api::{EffectCost, PlannedEffect};
use myelin_agent::{EffectResult, GateId};
use myelin_identity::{Consistency, Permission, PrincipalId, SubjectTree};
use myelin_tenancy::ArtifactRef;

// ───────────────────────── the hitl_gate state machine — the state enum ─────────────────────

/// **The `hitl_gate.state` state machine.** A gate opens `Waiting`, then
/// transitions ONCE to a terminal state: `Approved` (the tool is added to the run's `approved` set; a
/// re-run of the pipeline step 6 passes and the effect applies) | `Rejected` (settles
/// [`Halted::Rejected`] with the reason; the effect is never applied — 0 mutation) | `Expired` (the
/// approval window lapsed → auto-deny; 0 mutation). A terminal gate never re-transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HitlGateState {
    /// the gate is open and PARKED — the durable wait holds no runtime; a human has not yet decided.
    Waiting,
    /// the gate was APPROVED — the tool is added to `approved`; the re-run applies the effect.
    Approved,
    /// the gate was REJECTED — settles [`Halted::Rejected`]; the effect is never applied (0 mutation).
    Rejected,
    /// the gate EXPIRED — the approval window lapsed → auto-deny (0 mutation).
    Expired,
}

impl HitlGateState {
    /// The wire/audit token for the `hitl_gate.state` column — a stable, lowercase taxonomy
    /// token, not PII.
    pub fn as_str(self) -> &'static str {
        match self {
            HitlGateState::Waiting => "waiting",
            HitlGateState::Approved => "approved",
            HitlGateState::Rejected => "rejected",
            HitlGateState::Expired => "expired",
        }
    }

    /// Whether the gate is in a terminal state (a decided gate never re-transitions — a double-click
    /// on an already-decided gate is a no-op).
    pub fn is_terminal(self) -> bool {
        !matches!(self, HitlGateState::Waiting)
    }
}

// ───────────────────────── the humanised risk_summary slot ──────────

/// **The humanised `risk_summary` SLOT — a `(template_key, args)` pair, NEVER a raw string.** The
/// card's `risk_summary` is never an agent-authored raw string: it is a stable `template_key` +
/// ordered `(name, ArtifactRef)` args that Notif `humanise` (the ONE templating surface) renders
/// per-viewer/locale, permission- + erasure-safe.
///
/// **Floor:** the humanise WIRING (the `humanise((template_key, args), viewer, locale)` call that
/// renders this slot into card text) is a follow-on. HERE the gate carries the SLOT; the render is
/// that follow-on. Modeling it as `(key, args)` now means the follow-on wires a render, it does not
/// re-shape the data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskSummary {
    /// the stable template key (e.g. `agent.hitl.merge_pr`) — a taxonomy token, never free text.
    pub template_key: String,
    /// the ordered humanise args — `(arg_name, ArtifactRef)` (references-not-payloads). The
    /// rendered text is permission-/erasure-safe because each arg is a REFERENCE Notif resolves
    /// per-viewer, never an inline PII body.
    pub args: Vec<(String, ArtifactRef)>,
}

impl RiskSummary {
    /// A risk summary for a tool acting on an object (the common single-action card).
    pub fn for_action(template_key: impl Into<String>, object: &ArtifactRef) -> RiskSummary {
        RiskSummary {
            template_key: template_key.into(),
            args: vec![("object".to_string(), object.clone())],
        }
    }
}

// ───────────────────────── the hitl_gate row + the state machine ─────────────────────────────

/// **A `Halt` settlement of a run.** A REJECTED gate settles `Halted::Rejected(reason)` — the
/// run halts with the reason recorded in the trace + audit; the effect is never applied (0 mutation).
/// (The reason is a machine token / template ref, not PII; the loud audit trail proves WHY the
/// run halted.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Halted {
    /// the HITL gate was rejected — the reason is recorded in the trace + audit (0 mutation).
    Rejected(String),
    /// the HITL approval window lapsed (auto-deny) — recorded distinctly from an explicit reject.
    Expired,
}

/// **An invalid `hitl_gate` transition (a terminal gate cannot re-transition).** Returned by
/// [`HitlGate::approve`]/[`reject`]/[`expire`] when the gate is NOT `Waiting` — a double-click on an
/// already-decided gate is a no-op (the second click sees the gate already terminal, never a
/// double-apply). Carries the state the gate is ALREADY in (the loud, observable fact).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTransition {
    /// the terminal state the gate is already in (a re-decide is refused — a no-op, not a re-apply).
    pub already: HitlGateState,
}

impl core::fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "hitl_gate is already terminal ({}) — a decided gate does not re-transition",
            self.already.as_str()
        )
    }
}

impl std::error::Error for InvalidTransition {}

/// **The `hitl_gate` row + its state machine.**
/// Fields: `gate_id`, `run_id`, `effect_id`, `risk_summary` (the humanised SLOT),
/// `cost_estimate` (the LIVE estimate, integer minor-units, never floats), `approver_filter` (=
/// `list_subjects(object, approve_perm)`), `state`, `card_ref`. A gate holds NO runtime while it
/// waits — the durable wait parks the run; this struct is the durable row the wait re-leases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitlGate {
    /// the opaque gate id (the [`GateId`] the `Gated` verdict carried).
    pub gate_id: GateId,
    /// the run this gate belongs to (FK to `run.run_id`).
    pub run_id: String,
    /// the proposed effect this gate withholds (the tool + object the re-run re-applies).
    pub tool_name: String,
    /// the object the effect targets (the `list_subjects` object + the apply target).
    pub object: ArtifactRef,
    /// the humanised risk summary SLOT the card shows (`(template_key, args)`).
    pub risk_summary: RiskSummary,
    /// the LIVE cost estimate the card shows, integer minor-units (never floats) — the metered cost of
    /// the withheld effect (wholesale + markup, the BUDGET would debit on apply).
    pub cost_estimate: u64,
    /// the APPROVER set = `list_subjects(object, approve_perm)` — a set of OPAQUE principal
    /// pseudonyms, never raw names. Who MAY approve this gate.
    pub approver_filter: Vec<PrincipalId>,
    /// the gate state (the state machine).
    pub state: HitlGateState,
    /// the `card_ref` the gate surfaces as (the chat approval card; the durable-wait `idem_key` base).
    pub card_ref: String,
}

impl HitlGate {
    /// **WITHHOLD → OPEN: build a `Waiting` gate from a [`PlannedEffect`] + the [`GateId`] the step-6
    /// `Gated` verdict carried.** The gate carries the humanised risk slot, the LIVE cost estimate
    /// (the effect's metered cost — what the BUDGET would debit on apply), the approver set, and the
    /// `card_ref` the durable wait surfaces as. Opening a gate makes NO mutation — it is a durable
    /// row, not an apply.
    pub fn open(
        gate_id: GateId,
        run_id: impl Into<String>,
        plan: &PlannedEffect,
        risk_summary: RiskSummary,
        approver_filter: Vec<PrincipalId>,
        card_ref: impl Into<String>,
    ) -> HitlGate {
        HitlGate {
            gate_id,
            run_id: run_id.into(),
            tool_name: plan.tool.0.clone(),
            object: plan.object.clone(),
            risk_summary,
            // the LIVE cost estimate is the effect's metered total (wholesale + markup) — the card
            // shows what the reserve WOULD debit, integer minor-units (never a float).
            cost_estimate: live_cost_estimate(&plan.cost),
            approver_filter,
            state: HitlGateState::Waiting,
            card_ref: card_ref.into(),
        }
    }

    /// **RESUME → APPROVE: transition `Waiting → Approved`.** The caller then adds
    /// [`Self::tool_name`] to the run's [`ApprovedTools`] set; a re-run of
    /// [`crate::effect_api::PlanThenApply::apply_planned`] now passes step 6 and the effect applies.
    /// Refused (a no-op) if the gate is already terminal (a double-click is one approval — the second
    /// click sees `Approved` and returns `Err`, never a second apply).
    pub fn approve(&mut self) -> Result<(), InvalidTransition> {
        if self.state.is_terminal() {
            return Err(InvalidTransition {
                already: self.state,
            });
        }
        self.state = HitlGateState::Approved;
        Ok(())
    }

    /// **RESUME → REJECT: transition `Waiting → Rejected` + settle [`Halted::Rejected`].** The reason
    /// is recorded in the trace + audit; the tool is NEVER added to `approved`, so the effect is never
    /// applied (0 mutation). Refused (a no-op) if already terminal.
    pub fn reject(&mut self, reason: impl Into<String>) -> Result<Halted, InvalidTransition> {
        if self.state.is_terminal() {
            return Err(InvalidTransition {
                already: self.state,
            });
        }
        self.state = HitlGateState::Rejected;
        Ok(Halted::Rejected(reason.into()))
    }

    /// **The approval window lapsed: transition `Waiting → Expired` (auto-deny).** The effect is never
    /// applied (0 mutation); settles [`Halted::Expired`]. Refused (a no-op) if already terminal.
    pub fn expire(&mut self) -> Result<Halted, InvalidTransition> {
        if self.state.is_terminal() {
            return Err(InvalidTransition {
                already: self.state,
            });
        }
        self.state = HitlGateState::Expired;
        Ok(Halted::Expired)
    }

    /// Whether this gate has been APPROVED (the tool may now be added to the run's `approved` set so
    /// the re-run applies it). `false` for a `Waiting`/`Rejected`/`Expired` gate (those never apply).
    pub fn is_approved(&self) -> bool {
        matches!(self.state, HitlGateState::Approved)
    }
}

/// **The LIVE cost estimate the card shows — the withheld effect's metered total, integer
/// minor-units.** It is the cost the BUDGET would debit on apply (`wholesale + markup`), so the human
/// approves with the real bill in view (no surprise charge). A pure function of the effect's
/// [`EffectCost`] — never a float, never a guess.
pub fn live_cost_estimate(cost: &EffectCost) -> u64 {
    cost.total()
}

// ───────────────────────── the surfaced card (action + risk + LIVE cost + approvers) ─────────────

/// **The chat approval card the gate surfaces as.** Projects a [`HitlGate`] into the card a viewer
/// sees: the pending ACTION (tool + object), the humanised RISK summary slot, the LIVE COST estimate,
/// and the APPROVER set (who may decide). The card's TEXT is rendered by Notif `humanise` (the ONE
/// templating surface) from [`Self::risk_summary`] — HERE the card carries the slot + the structured
/// fields, not a rendered string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitlCard {
    /// the gate this card surfaces (the durable row the durable wait re-leases).
    pub gate_id: GateId,
    /// the pending action (the tool the effect invokes).
    pub action_tool: String,
    /// the object the action targets.
    pub action_object: ArtifactRef,
    /// the humanised risk summary SLOT (`(template_key, args)`, rendered per-viewer).
    pub risk_summary: RiskSummary,
    /// the LIVE cost estimate, integer minor-units (the bill the human approves with in view).
    pub cost_estimate: u64,
    /// the approver set (who MAY decide) = `list_subjects(object, approve_perm)`.
    pub approvers: Vec<PrincipalId>,
    /// the `card_ref` (the durable-wait `idem_key` base; the chat card identity).
    pub card_ref: String,
}

/// **SURFACE: project a `Waiting` [`HitlGate`] into the [`HitlCard`] a viewer sees.**
/// The card shows the pending action + risk + the LIVE cost estimate + the approver set — exactly the
/// three things a withhold card carries (action + risk + cost). A purely observational projection (no
/// mutation, no state change) — the surface is the read side of the durable wait.
pub fn surface_card(gate: &HitlGate) -> HitlCard {
    HitlCard {
        gate_id: gate.gate_id.clone(),
        action_tool: gate.tool_name.clone(),
        action_object: gate.object.clone(),
        risk_summary: gate.risk_summary.clone(),
        cost_estimate: gate.cost_estimate,
        approvers: gate.approver_filter.clone(),
        card_ref: gate.card_ref.clone(),
    }
}

// ───────────────────────── the consumer seams (list_subjects, durable wait) ──────────────

/// **The `list_subjects` surface, as the HITL machinery consumes it.** A seam so
/// `myelin-agent-service` does NOT depend on `myelin-identity-service` (the engine body) — the same
/// decoupling [`crate::effect_api::CapabilityCheck`] uses. The approver set = `list_subjects(object,
/// approve_perm)`: the subjects who hold the approval permission on the gated object, at the run's
/// zookie snapshot. The CDC pairs this consumer with the real Identity `list_subjects` provider
/// (`tests/cdc_4_4_list_subjects.rs`).
pub trait ApproverSet {
    /// **`list_subjects`** — the subject userset tree of who holds `approve_perm` on `object` at
    /// the consistency `at`. The HITL card's `approver_filter` is its [`SubjectTree::members`].
    fn list_subjects(
        &self,
        object: &ArtifactRef,
        approve_perm: &Permission,
        at: &Consistency,
    ) -> SubjectTree;
}

/// **Derive the approver set for a gated effect (`list_subjects(object, approve_perm)`).**
/// The set of principals who MAY approve this gate = who holds `approve_perm` on the gated object at
/// the run's zookie snapshot. Returns the opaque pseudonyms, never raw names — the `card.approvers`
/// + the `hitl_gate.approver_filter`.
pub fn derive_approver_set<A: ApproverSet>(
    approvers: &A,
    object: &ArtifactRef,
    approve_perm: &Permission,
    at: &Consistency,
) -> Vec<PrincipalId> {
    approvers.list_subjects(object, approve_perm, at).members
}

/// **The durable HITL wait, as the agent fabric consumes it.** A seam over the durable-workflow
/// `request_approval_and_wait` round-trip (`myelin_flow::approval`): emit `agent.approval.requested`
/// via the outbox ONCE → park `state=waiting` holding NO runtime → resume on the `approval` signal
/// days later (consume-exactly-once across a re-drive). Modeled as a seam so `myelin-agent-service`
/// does NOT take a production dep on `myelin-flow` (the DAG stays acyclic — `myelin-flow` already
/// depends on the agent-fabric effect set; the reverse edge would be a cycle). The production wiring
/// hands the real `WfCtx`-driven round-trip; the CDC pairs this consumer with the real `myelin-flow`
/// provider (`tests/cdc_9_4_durable_hitl.rs`).
pub trait HitlWait {
    /// **Park on the durable HITL wait for `gate`.** Returns the human's [`WaitDecision`] when
    /// the `approval` signal arrives (days later) — `Approve`, `Reject(reason)`, or `Expired` (the
    /// window lapsed). While parked, the run holds NO runtime (the worker is free). A re-drive replays
    /// the prefix + consumes the signal EXACTLY once.
    fn park_and_wait(&self, gate: &HitlGate) -> WaitDecision;
}

/// **The human's decision returned by the durable HITL wait.** `Approve` (the effect applies),
/// `Reject(reason)` (the effect is withheld → 0 mutation), or `Expired` (the window lapsed →
/// auto-deny). The decision drives the [`HitlGate`] state-machine transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitDecision {
    /// the human APPROVED → the gate transitions `Waiting → Approved`; the re-run applies the effect.
    Approve,
    /// the human REJECTED → the gate transitions `Waiting → Rejected`; settles `Halted::Rejected`.
    Reject(String),
    /// the approval window lapsed → the gate transitions `Waiting → Expired` (auto-deny, 0 mutation).
    Expired,
}

// ───────────────────────── the run's approved-tool set (the resume threading) ────────────────────

/// **The run's `approved` PER-EFFECT gate-key set — the slot the resume threads a decided gate
/// into.** `EffectApi::apply`'s step 6 reads exactly this set, keyed by
/// [`crate::effect_api::effect_gate_key`] (`gate:{tool}:{object}` — the SAME key the step-6 `GateId`
/// is minted from): a `requires_approval` effect whose OWN key is in the set passes the gate and
/// applies. A fresh run's set is empty (every gated effect withholds); the HITL resume adds an
/// APPROVED gate's per-effect key so the re-run applies THAT effect. This is the bridge between the
/// `hitl_gate` machine (this module) and [`crate::effect_api::PlanThenApply::approved`].
///
/// **Why per-(tool, object) keys, not bare tool names:** a bare-tool-name set would let approving one
/// `git.merge` admit every sibling `git.merge` in a batch, including a DECLINED one re-driven through
/// `apply_planned`. Per-(tool, object) keys mean a declined sibling's key is never present and it
/// gates again (0 mutation). The type keeps its name (it is the run's approved set everywhere) but a
/// bare tool name is no longer an approval key anywhere.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApprovedTools(pub std::collections::BTreeSet<String>);

impl ApprovedTools {
    /// A fresh (empty) approved set — every `requires_approval` tool withholds until a gate approves.
    pub fn new() -> ApprovedTools {
        ApprovedTools::default()
    }

    /// **RESUME: add an APPROVED gate's PER-EFFECT key to the set.** After this, a re-run of the
    /// pipeline passes step 6 for exactly that `(tool, object)` effect — never for a sibling sharing
    /// the tool name. Refused (a no-op, returns `false`) if the gate is NOT approved (a
    /// Rejected/Expired/Waiting gate NEVER threads its key into `approved` — the effect is never
    /// applied). Idempotent: re-admitting an approved gate is a no-op (a double-click is one approval
    /// — the set is the truth, not the click count).
    pub fn admit(&mut self, gate: &HitlGate) -> bool {
        if !gate.is_approved() {
            return false;
        }
        self.0.insert(crate::effect_api::effect_gate_key_str(
            &gate.tool_name,
            &gate.object.0,
        ));
        true
    }

    /// Whether the per-effect `key` ([`crate::effect_api::effect_gate_key`]) is approved for this
    /// run (the step-6 read).
    pub fn contains(&self, key: &str) -> bool {
        self.0.contains(key)
    }

    /// Whether the `(tool, object)` effect is approved for this run (the key-derived convenience
    /// read — same derivation as the step-6 consult).
    pub fn contains_effect(&self, tool: &str, object: &str) -> bool {
        self.0
            .contains(&crate::effect_api::effect_gate_key_str(tool, object))
    }

    /// The set as the [`crate::effect_api::PlanThenApply::approved`] field expects it (a `BTreeSet`).
    pub fn as_set(&self) -> std::collections::BTreeSet<String> {
        self.0.clone()
    }
}

// ───────────────────────── the end-to-end withhold→surface→resume loop driver ────────────────────

/// **The outcome of the full withhold → surface → resume loop for one gated effect.** Either the
/// effect was APPROVED (the tool admitted to `approved`; the caller re-runs the pipeline → applies)
/// or it was HALTED (rejected/expired → settles [`Halted`]; 0 mutation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HitlOutcome {
    /// the gate was APPROVED — the tool is now in the run's [`ApprovedTools`]; the caller re-runs the
    /// pipeline step (which now passes step 6 → applies). Carries the approved gate.
    Approved(HitlGate),
    /// the gate was HALTED (rejected or expired) — settles [`Halted`]; the effect is never applied
    /// (0 mutation). Carries the halt settlement (the reason recorded in the trace + audit).
    Halted(Halted),
}

/// **RESUME: settle a `Waiting` gate on the human's [`WaitDecision`] — the ONE place the
/// approve→admit ORDER lives.** This is the shared three-way decision block both loop drivers
/// (single-effect [`run_hitl_loop`] and batch `run_batch_hitl_loop`) call, so the security-critical
/// ordering — `approve()` transitions the gate THEN `admit()` threads its per-(tool, object) key into
/// `approved` — is written once, not duplicated:
///
/// - `Ok(())` — the gate was APPROVED and its key ADMITTED to `approved` (the re-run applies THAT
///   effect). The caller does any post-approval bookkeeping (e.g. the batch records the per-effect
///   apply in its ledger) and builds its own approved-outcome.
/// - `Err(halted)` — the gate was HALTED (`Reject` → [`Halted::Rejected`], `Expired` →
///   [`Halted::Expired`]); the key is NEVER admitted, so the effect is never applied (0 mutation, AG-8).
///
/// The gate is freshly `Waiting`, so every transition is infallible — the `.expect(...)` messages are
/// the load-bearing invariant assertions (a non-`Waiting` gate here is a caller bug).
pub(crate) fn resolve_decision(
    gate: &mut HitlGate,
    decision: WaitDecision,
    approved: &mut ApprovedTools,
) -> Result<(), Halted> {
    match decision {
        WaitDecision::Approve => {
            // approve is infallible here (the gate is freshly Waiting); admit the tool so the re-run
            // applies the effect (the caller re-runs apply_planned with the populated approved set).
            gate.approve().expect("a freshly-opened gate is Waiting");
            approved.admit(gate); // thread the tool into `approved` (idempotent).
            Ok(())
        }
        WaitDecision::Reject(reason) => {
            let halted = gate
                .reject(reason)
                .expect("a freshly-opened gate is Waiting");
            Err(halted)
        }
        WaitDecision::Expired => {
            let halted = gate.expire().expect("a freshly-opened gate is Waiting");
            Err(halted)
        }
    }
}

/// **Drive the withhold → surface → resume loop for ONE gated effect.** Given the `Gated` verdict's
/// [`GateId`], the [`PlannedEffect`], the approver set, and the durable-wait + approved-set seams:
///
/// 1. **WITHHOLD → OPEN** a `Waiting` gate ([`HitlGate::open`]) — 0 mutation.
/// 2. **SURFACE** the card ([`surface_card`]) — the durable wait emits `agent.approval.requested`
///    carrying the card.
/// 3. **DECIDE** — park on the durable wait ([`HitlWait::park_and_wait`]); the run holds no runtime
///    until a human decides (days later).
/// 4. **RESUME** — on `Approve`, transition the gate `→ Approved` + admit the tool to `approved`
///    ([`ApprovedTools::admit`]) so a re-run applies the effect → [`HitlOutcome::Approved`]; on
///    `Reject`/`Expired`, transition `→ Rejected`/`Expired` + settle [`Halted`] → [`HitlOutcome::Halted`]
///    (0 mutation).
///
/// **The 0-mutation-pre-approval guarantee:** this driver NEVER applies the effect — it only opens the
/// gate + threads the decision. The apply happens ONLY when the caller re-runs
/// [`crate::effect_api::PlanThenApply::apply_planned`] with the now-populated `approved` set (and ONLY
/// for an `Approved` outcome). A Halted outcome never admits the tool, so the re-run gates again / the
/// loop withholds — 0 mutation.
#[allow(clippy::too_many_arguments)]
pub fn run_hitl_loop<W: HitlWait>(
    gate_id: GateId,
    run_id: &str,
    plan: &PlannedEffect,
    risk_summary: RiskSummary,
    approver_filter: Vec<PrincipalId>,
    card_ref: &str,
    wait: &W,
    approved: &mut ApprovedTools,
) -> HitlOutcome {
    // 1. WITHHOLD → OPEN the gate (0 mutation — a durable row, not an apply).
    let mut gate = HitlGate::open(
        gate_id,
        run_id,
        plan,
        risk_summary,
        approver_filter,
        card_ref,
    );

    // 2 + 3. SURFACE + DECIDE — park on the durable wait; the run holds NO runtime until a
    //        human decides (days later). `surface_card` is the card the wait emits.
    let _card = surface_card(&gate);
    let decision = wait.park_and_wait(&gate);

    // 4. RESUME — transition the gate per the decision + thread the result (the approve→admit order
    //    lives in the shared `resolve_decision`).
    match resolve_decision(&mut gate, decision, approved) {
        Ok(()) => HitlOutcome::Approved(gate),
        Err(halted) => HitlOutcome::Halted(halted),
    }
}

// ───────────────────────── HitlGate persistence over agent_hitl_gate ───────────────────────

use myelin_storage::hitl_gate_durable::{
    GateDecideError, GateOpenError, GateRecord, GateState as DurableGateState, HitlVerdictStore,
};
use myelin_storage::TenantScope;

impl HitlGate {
    /// **Project this gate onto the durable `agent_hitl_gate` row shape.** The `effect_id`
    /// is the PER-EFFECT key ([`crate::effect_api::effect_gate_key_str`]) — the same key the step-6
    /// consult and [`ApprovedTools::admit`] use, so the durable verdict and the in-run approval are
    /// keyed identically by construction. `requested_by` is the agent principal whose effect
    /// tripped the gate (the distinct-approver anchor); it is structurally EXCLUDED from the
    /// persisted `approver_filter` so the requester is never an eligible approver of its own gate.
    pub fn to_gate_record(&self, requested_by: &str) -> GateRecord {
        let opened_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .expect("system clock must be after the Unix epoch");
        GateRecord {
            gate_id: self.gate_id.0.clone(),
            run_id: self.run_id.clone(),
            effect_id: crate::effect_api::effect_gate_key_str(&self.tool_name, &self.object.0),
            // The humanised SLOT serialized as bytes (references-not-payloads: template key +
            // ArtifactRef args, never inline PII; the DEK envelope is the storage layer's).
            risk_summary: serde_json::to_vec(&serde_json::json!({
                "template_key": self.risk_summary.template_key,
                "args": self
                    .risk_summary
                    .args
                    .iter()
                    .map(|(n, r)| serde_json::json!([n, r.0]))
                    .collect::<Vec<_>>(),
            }))
            .unwrap_or_default(),
            cost_estimate: self.cost_estimate,
            approver_filter: self
                .approver_filter
                .iter()
                .map(|p| p.0.clone())
                .filter(|p| p != requested_by)
                .collect(),
            state: match self.state {
                HitlGateState::Waiting => DurableGateState::Waiting,
                HitlGateState::Approved => DurableGateState::Approved,
                HitlGateState::Rejected => DurableGateState::Rejected,
                HitlGateState::Expired => DurableGateState::Expired,
            },
            card_ref: Some(self.card_ref.clone()),
            requested_by: requested_by.to_string(),
            decided_by: None,
            opened_at_unix,
            decided_at_unix: None,
            expires_at_unix: opened_at_unix
                .saturating_add(myelin_storage::hitl_gate_durable::DEFAULT_HITL_GATE_TTL_SECS),
            approval_consumed_at_unix: None,
        }
    }
}

/// **WITHHOLD → PERSIST: INSERT the pending gate row (the durable server-side gate).** A
/// freshly opened `Waiting` [`HitlGate`] becomes an `agent_hitl_gate` row lookup-able by its
/// `gate_id` across requests/processes; the later decision UPDATEs it via
/// [`persist_gate_decision`] (or directly through the store's approve/reject/expire, which enforce
/// the distinct-approver rule server-side).
pub fn persist_gate_open(
    store: &mut HitlVerdictStore,
    scope: &TenantScope,
    gate: &HitlGate,
    requested_by: &str,
) -> Result<(), GateOpenError> {
    store.open(scope, gate.to_gate_record(requested_by))
}

/// **RESUME → PERSIST: UPDATE the durable row to this gate's decided state.**
/// Dispatches on the in-process state machine's terminal state: `Approved` records `decided_by` and
/// the store re-enforces the full rule — **the approver must be a `Human` principal**, be eligible,
/// and be distinct from the requester (a machine/self approval refuses even here); `Rejected` records
/// the decider; `Expired` records none. The `decided_by` is the AUTHENTICATED approver `Principal`
/// (its `kind` is the human check, its id is what is persisted). A still-`Waiting` gate has no
/// decision to persist — refused typed (`NotFound`), with a debug-assert naming the caller bug loudly
/// in dev.
pub fn persist_gate_decision(
    store: &mut HitlVerdictStore,
    scope: &TenantScope,
    gate: &HitlGate,
    decided_by: Option<&myelin_identity::Principal>,
) -> Result<(), GateDecideError> {
    let decided_at_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .expect("system clock must be after the Unix epoch");
    debug_assert!(
        gate.state.is_terminal(),
        "persist_gate_decision is for a DECIDED gate (caller bug: still Waiting)"
    );
    match gate.state {
        HitlGateState::Approved => {
            let approver = decided_by.ok_or(GateDecideError::NotEligible)?;
            store.approve_at(
                scope,
                &gate.gate_id.0,
                &approver.principal_id.0,
                approver.kind.clone(),
                decided_at_unix,
            )
        }
        HitlGateState::Rejected => {
            let decider = decided_by.ok_or(GateDecideError::NotEligible)?;
            store.reject_at(
                scope,
                &gate.gate_id.0,
                &decider.principal_id.0,
                decider.kind.clone(),
                decided_at_unix,
            )
        }
        HitlGateState::Expired => store.expire_at(scope, &gate.gate_id.0, decided_at_unix),
        HitlGateState::Waiting => Err(GateDecideError::NotFound),
    }
}

/// **Extract the [`GateId`] from a step-6 [`EffectResult::Gated`] verdict (the withhold entry).** The
/// withhold → surface → resume loop is driven ONLY by a `Gated` result; an `Applied`/`Denied` result
/// never opens a gate (an applied effect already mutated; a denied effect is an ordinary tool error,
/// not a withhold). Returns `None` for a non-`Gated` result so the caller never opens a spurious gate.
pub fn gate_id_of(result: &EffectResult) -> Option<GateId> {
    match result {
        EffectResult::Gated(g) => Some(g.clone()),
        EffectResult::Applied(_) | EffectResult::Denied(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{ConsistencyMode, Zookie};

    fn plan(tool: &str) -> PlannedEffect {
        PlannedEffect {
            tool: myelin_agent::ToolName(tool.into()),
            object: ArtifactRef("myelin://acme/git/pr/42".into()),
            input_json: r#"{"pr":42}"#.into(),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "git.merge",
                wholesale: 30,
                markup: 20,
            },
        }
    }

    fn risk() -> RiskSummary {
        RiskSummary::for_action(
            "agent.hitl.merge_pr",
            &ArtifactRef("myelin://acme/git/pr/42".into()),
        )
    }

    fn approvers() -> Vec<PrincipalId> {
        vec![
            PrincipalId("psn:lead".into()),
            PrincipalId("psn:maintainer".into()),
        ]
    }

    fn open_waiting() -> HitlGate {
        HitlGate::open(
            GateId("gate:git.merge:myelin://acme/git/pr/42".into()),
            "R1",
            &plan("git.merge"),
            risk(),
            approvers(),
            "card:R1:0",
        )
    }

    // ───────── the hitl_gate state machine: Waiting → Approved / Rejected / Expired ─────────

    /// **A gate opens `Waiting` carrying the §4.4 fields: humanised risk slot, LIVE cost, approver
    /// set, card_ref — and makes NO mutation (it is a durable row, AG-8).**
    #[test]
    fn open_gate_is_waiting_and_carries_the_card_fields() {
        let g = open_waiting();
        assert_eq!(g.state, HitlGateState::Waiting);
        assert!(!g.state.is_terminal());
        // the LIVE cost estimate = the effect's metered total (wholesale 30 + markup 20 = 50).
        assert_eq!(
            g.cost_estimate, 50,
            "the card shows the LIVE cost the reserve would debit"
        );
        assert_eq!(g.tool_name, "git.merge");
        assert_eq!(g.object, ArtifactRef("myelin://acme/git/pr/42".into()));
        // the risk_summary is a (template_key, args) SLOT, NEVER a raw string (C9 — AG-P11 renders it).
        assert_eq!(g.risk_summary.template_key, "agent.hitl.merge_pr");
        assert_eq!(
            g.approver_filter.len(),
            2,
            "the approver set = list_subjects(object, approve_perm)"
        );
        assert_eq!(g.card_ref, "card:R1:0");
    }

    /// **`Waiting → Approved` (the resume leg): approve transitions once + is_approved holds.**
    #[test]
    fn waiting_approves_to_approved() {
        let mut g = open_waiting();
        assert!(!g.is_approved());
        g.approve().expect("waiting → approved");
        assert_eq!(g.state, HitlGateState::Approved);
        assert!(g.is_approved());
        assert!(g.state.is_terminal());
    }

    /// **`Waiting → Rejected` settles `Halted::Rejected(reason)` — the reason rides into the trace +
    /// audit; the gate is terminal; the tool is never approved (0 mutation, AG-8).**
    #[test]
    fn waiting_rejects_and_settles_halted_rejected() {
        let mut g = open_waiting();
        let halted = g.reject("not safe to merge").expect("waiting → rejected");
        assert_eq!(halted, Halted::Rejected("not safe to merge".into()));
        assert_eq!(g.state, HitlGateState::Rejected);
        assert!(
            !g.is_approved(),
            "a rejected gate never approves the tool (0 mutation, AG-8)"
        );
    }

    /// **`Waiting → Expired` settles `Halted::Expired` (auto-deny; 0 mutation).**
    #[test]
    fn waiting_expires_to_expired() {
        let mut g = open_waiting();
        let halted = g.expire().expect("waiting → expired");
        assert_eq!(halted, Halted::Expired);
        assert_eq!(g.state, HitlGateState::Expired);
        assert!(!g.is_approved());
    }

    /// **A terminal gate NEVER re-transitions (a double-click on a decided gate is a no-op).** An
    /// already-Approved gate refuses a second approve / a reject; an already-Rejected gate refuses an
    /// approve — the second click sees the terminal state, never a double-apply.
    #[test]
    fn a_terminal_gate_refuses_re_transition() {
        let mut g = open_waiting();
        g.approve().unwrap();
        // a double-click "approve" is refused (a no-op) — the gate is already Approved.
        assert_eq!(
            g.approve(),
            Err(InvalidTransition {
                already: HitlGateState::Approved
            })
        );
        // a late "reject" after an approve is also refused (the decision is settled).
        assert_eq!(
            g.reject("late"),
            Err(InvalidTransition {
                already: HitlGateState::Approved
            })
        );

        let mut r = open_waiting();
        r.reject("no").unwrap();
        assert_eq!(
            r.approve(),
            Err(InvalidTransition {
                already: HitlGateState::Rejected
            })
        );
        assert_eq!(
            r.expire(),
            Err(InvalidTransition {
                already: HitlGateState::Rejected
            })
        );
    }

    /// **The state tokens are the frozen §4.4 lowercase taxonomy (a renamed token fails the audit).**
    #[test]
    fn state_tokens_are_frozen() {
        assert_eq!(HitlGateState::Waiting.as_str(), "waiting");
        assert_eq!(HitlGateState::Approved.as_str(), "approved");
        assert_eq!(HitlGateState::Rejected.as_str(), "rejected");
        assert_eq!(HitlGateState::Expired.as_str(), "expired");
    }

    // ───────── surface: the card carries action + risk + LIVE cost + approvers (AG-D5) ─────────

    /// **SURFACE: the card projects action + risk + LIVE cost + approver set (exactly what AG-D5
    /// asserts a withhold card shows) — observationally, with no state change.**
    #[test]
    fn surface_card_shows_action_risk_cost_and_approvers() {
        let g = open_waiting();
        let card = surface_card(&g);
        assert_eq!(
            card.action_tool, "git.merge",
            "the card shows the pending ACTION"
        );
        assert_eq!(
            card.action_object,
            ArtifactRef("myelin://acme/git/pr/42".into())
        );
        assert_eq!(
            card.risk_summary.template_key, "agent.hitl.merge_pr",
            "the card shows the RISK slot"
        );
        assert_eq!(
            card.cost_estimate, 50,
            "the card shows the LIVE COST estimate"
        );
        assert_eq!(card.approvers.len(), 2, "the card shows the APPROVER set");
        // surfacing did not change the gate state (a read-side projection).
        assert_eq!(g.state, HitlGateState::Waiting);
    }

    // ───────── the approver-set derivation (4.4 list_subjects) ─────────

    struct FakeSubjects {
        members: Vec<PrincipalId>,
    }
    impl ApproverSet for FakeSubjects {
        fn list_subjects(
            &self,
            object: &ArtifactRef,
            approve_perm: &Permission,
            at: &Consistency,
        ) -> SubjectTree {
            SubjectTree {
                object: myelin_identity::ObjectId(object.0.clone()),
                relation: myelin_identity::RelName(approve_perm.0.clone()),
                members: self.members.clone(),
                zookie: at.at_least.clone(),
            }
        }
    }

    /// **The approver set = `list_subjects(object, approve_perm)` (4.4) — the members of who holds the
    /// approval permission on the gated object, at the run's zookie.**
    #[test]
    fn approver_set_is_list_subjects_members() {
        let subjects = FakeSubjects {
            members: approvers(),
        };
        let at = Consistency {
            at_least: Zookie("z-7".into()),
            mode: ConsistencyMode::Strong,
        };
        let set = derive_approver_set(
            &subjects,
            &ArtifactRef("myelin://acme/git/pr/42".into()),
            &Permission("git.approve".into()),
            &at,
        );
        assert_eq!(
            set,
            approvers(),
            "the approver_filter is list_subjects(object, approve_perm).members"
        );
    }

    // ───────── the approved-tool set (the resume threading into EffectApi step 6) ─────────

    /// **RESUME threads an APPROVED gate's PER-EFFECT key into `approved`; a Rejected/Expired gate
    /// NEVER does (0 mutation, AG-8); admit is idempotent (a double-click is one approval); and —
    /// R2.4 — the key is per-(tool, object), never a bare tool name.**
    #[test]
    fn approved_set_admits_only_approved_gates_idempotently() {
        const PR42: &str = "myelin://acme/git/pr/42";
        let mut approved = ApprovedTools::new();
        assert!(!approved.contains_effect("git.merge", PR42));

        // a Waiting gate is NOT admitted (no decision yet).
        let waiting = open_waiting();
        assert!(!approved.admit(&waiting), "a Waiting gate threads nothing");
        assert!(!approved.contains_effect("git.merge", PR42));

        // an Approved gate IS admitted → the re-run applies THAT effect.
        let mut g = open_waiting();
        g.approve().unwrap();
        assert!(
            approved.admit(&g),
            "an Approved gate threads its per-effect key into approved"
        );
        assert!(approved.contains_effect("git.merge", PR42));
        // R2.4: the key is per-(tool, object) — the bare tool name is NOT an approval key, and a
        // sibling object is NOT admitted.
        assert!(
            !approved.contains("git.merge"),
            "a bare tool name is never an approval key (Defect B)"
        );
        assert!(
            !approved.contains_effect("git.merge", "myelin://acme/git/pr/41"),
            "an approval never transfers to a sibling object sharing the tool name"
        );
        // idempotent: a double-click (re-admit) is a no-op.
        assert!(approved.admit(&g));
        assert_eq!(
            approved.as_set().len(),
            1,
            "a double-click is one approval (one entry)"
        );

        // a Rejected gate NEVER threads its key (0 mutation, AG-8).
        let mut r = open_waiting();
        r.tool_name = "git.force_push".into();
        r.reject("no").unwrap();
        assert!(
            !approved.admit(&r),
            "a Rejected gate NEVER approves the effect (AG-8)"
        );
        assert!(!approved.contains_effect("git.force_push", PR42));
    }

    // ───────── the end-to-end withhold → surface → resume loop driver ─────────

    struct ApproveWait;
    impl HitlWait for ApproveWait {
        fn park_and_wait(&self, _gate: &HitlGate) -> WaitDecision {
            WaitDecision::Approve
        }
    }
    struct RejectWait(String);
    impl HitlWait for RejectWait {
        fn park_and_wait(&self, _gate: &HitlGate) -> WaitDecision {
            WaitDecision::Reject(self.0.clone())
        }
    }

    /// **The withhold → surface → resume loop, APPROVE leg: open `Waiting`, park, resume on Approve,
    /// admit the tool → the caller's re-run applies. 0 mutation in the loop itself.**
    #[test]
    fn loop_approve_admits_tool_for_the_re_run() {
        let mut approved = ApprovedTools::new();
        let outcome = run_hitl_loop(
            GateId("gate:git.merge:pr42".into()),
            "R1",
            &plan("git.merge"),
            risk(),
            approvers(),
            "card:R1:0",
            &ApproveWait,
            &mut approved,
        );
        match outcome {
            HitlOutcome::Approved(g) => {
                assert_eq!(g.state, HitlGateState::Approved);
            }
            other => panic!("expected Approved, got {other:?}"),
        }
        // the resume threaded THIS effect's key into `approved` → a re-run of apply_planned now
        // passes step 6 for exactly this (tool, object).
        assert!(
            approved.contains_effect("git.merge", "myelin://acme/git/pr/42"),
            "the approved effect's key is now in the run's approved set"
        );
    }

    /// **The withhold → surface → resume loop, REJECT leg: open `Waiting`, park, resume on Reject →
    /// `Halted::Rejected`; the tool is NEVER admitted (0 mutation, AG-8).**
    #[test]
    fn loop_reject_halts_and_never_admits() {
        let mut approved = ApprovedTools::new();
        let outcome = run_hitl_loop(
            GateId("gate:git.merge:pr42".into()),
            "R1",
            &plan("git.merge"),
            risk(),
            approvers(),
            "card:R1:0",
            &RejectWait("not safe".into()),
            &mut approved,
        );
        assert_eq!(
            outcome,
            HitlOutcome::Halted(Halted::Rejected("not safe".into()))
        );
        // the rejected gate NEVER threaded its key → the re-run gates again / the loop withholds.
        assert!(
            !approved.contains_effect("git.merge", "myelin://acme/git/pr/42"),
            "a rejected gate makes 0 mutation (the effect stays unapproved, AG-8)"
        );
    }

    // ───────── R2.4: HitlGate persistence over the agent_hitl_gate verdict store ─────────

    fn scope() -> TenantScope {
        TenantScope::from_verified_token(
            &human_principal("psn:human-x"),
            myelin_tenancy::Region("eu-west".into()),
        )
    }

    fn human_principal(id: &str) -> myelin_identity::Principal {
        myelin_identity::Principal::stub(
            PrincipalId(id.into()),
            myelin_identity::PrincipalKind::Human,
            myelin_tenancy::TenantId("acme".into()),
        )
    }

    fn agent_principal(id: &str) -> myelin_identity::Principal {
        myelin_identity::Principal::stub(
            PrincipalId(id.into()),
            myelin_identity::PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("rt".into()),
                on_behalf_of: None,
            },
            myelin_tenancy::TenantId("acme".into()),
        )
    }

    /// **A withheld gate PERSISTS as a `waiting` `agent_hitl_gate` row, lookup-able by its gate_id
    /// (R2.4).** The persisted `effect_id` is the SAME per-effect key `ApprovedTools`/step-6 use,
    /// and the requesting agent is structurally excluded from the persisted approver filter.
    #[test]
    fn a_withheld_gate_persists_waiting_and_is_lookup_able_by_gate_id() {
        let mut store = HitlVerdictStore::new();
        let gate = open_waiting();
        persist_gate_open(&mut store, &scope(), &gate, "agent:claude").expect("persists");

        let rec = store
            .fetch(&scope(), &gate.gate_id.0)
            .expect("the gate row is lookup-able by gate_id");
        assert_eq!(rec.state, DurableGateState::Waiting);
        assert_eq!(
            rec.effect_id,
            crate::effect_api::effect_gate_key_str("git.merge", "myelin://acme/git/pr/42"),
            "the durable verdict is keyed by the SAME per-effect key step 6 consults"
        );
        assert_eq!(
            rec.cost_estimate, 50,
            "the LIVE cost estimate rides the row"
        );
        assert_eq!(rec.requested_by, "agent:claude");
        assert!(
            !rec.approver_filter.contains(&"agent:claude".to_string()),
            "the requester is never an eligible approver of its own gate"
        );
    }

    /// **The decision UPDATEs the durable state — approve/reject (distinct eligible HUMAN decider,
    /// recorded), expire (system-only) — and the store refuses self, machine, and out-of-filter
    /// deciders at persist time (R2.4 / R2.4b).**
    #[test]
    fn a_decision_updates_the_durable_verdict_with_distinct_human_approver() {
        let mut store = HitlVerdictStore::new();

        // APPROVE by a distinct eligible HUMAN → durable `approved` + decided_by recorded.
        let mut g = open_waiting();
        persist_gate_open(&mut store, &scope(), &g, "agent:claude").unwrap();
        g.approve().unwrap();
        persist_gate_decision(&mut store, &scope(), &g, Some(&human_principal("psn:lead")))
            .expect("approves");
        let rec = store.fetch(&scope(), &g.gate_id.0).unwrap();
        assert_eq!(rec.state, DurableGateState::Approved);
        assert_eq!(rec.decided_by.as_deref(), Some("psn:lead"));
        assert!(rec.authorizes(&rec.effect_id.clone(), &rec.run_id.clone(), "agent:claude"));

        // A SELF-approval is refused server-side at persist time (the distinct-approver rule).
        let mut s2 = HitlVerdictStore::new();
        let mut g2 = open_waiting();
        persist_gate_open(&mut s2, &scope(), &g2, "agent:claude").unwrap();
        g2.approve().unwrap();
        assert!(
            persist_gate_decision(
                &mut s2,
                &scope(),
                &g2,
                Some(&agent_principal("agent:claude"))
            )
            .is_err(),
            "the requesting agent cannot approve its own gate — even through the persist path"
        );
        assert_eq!(
            s2.fetch(&scope(), &g2.gate_id.0).unwrap().state,
            DurableGateState::Waiting,
            "the refused self-approval left the durable row undecided"
        );

        // R2.4b: a MACHINE principal that IS eligible + distinct is STILL refused (distinct-HUMAN).
        let mut sm = HitlVerdictStore::new();
        let mut gm = open_waiting();
        // list the machine principal in the gate's approver filter so only the human check bites.
        gm.approver_filter
            .push(PrincipalId("machine:ci-bot".into()));
        persist_gate_open(&mut sm, &scope(), &gm, "agent:claude").unwrap();
        gm.approve().unwrap();
        assert_eq!(
            persist_gate_decision(
                &mut sm,
                &scope(),
                &gm,
                Some(&agent_principal("machine:ci-bot"))
            ),
            Err(GateDecideError::MachineApproverRefused),
            "a distinct, in-filter MACHINE approver is refused (distinct-HUMAN, R2.4b)"
        );
        assert_eq!(
            sm.fetch(&scope(), &gm.gate_id.0).unwrap().state,
            DurableGateState::Waiting,
            "the machine-refused approval left the durable row undecided"
        );

        // REJECT + EXPIRE settle their durable states (0 effect mutation either way, AG-8). A
        // terminal reject is a DoS capability over the effect, so the shared verdict store requires
        // the same distinct, eligible Human decision proof as approve. Only system expiry has no
        // Human decider.
        for (tag, decider, expected) in [
            (
                "self",
                agent_principal("agent:claude"),
                GateDecideError::SelfApproval,
            ),
            (
                "stranger",
                human_principal("psn:stranger"),
                GateDecideError::NotEligible,
            ),
            (
                "machine",
                agent_principal("machine:ci-bot"),
                GateDecideError::MachineApproverRefused,
            ),
        ] {
            let mut denied_store = HitlVerdictStore::new();
            let mut denied_gate = open_waiting();
            denied_gate.gate_id.0.push_str(tag);
            denied_gate
                .approver_filter
                .push(PrincipalId("machine:ci-bot".into()));
            persist_gate_open(&mut denied_store, &scope(), &denied_gate, "agent:claude").unwrap();
            denied_gate.reject("no").unwrap();
            assert_eq!(
                persist_gate_decision(&mut denied_store, &scope(), &denied_gate, Some(&decider),),
                Err(expected)
            );
            assert_eq!(
                denied_store
                    .fetch(&scope(), &denied_gate.gate_id.0)
                    .unwrap()
                    .state,
                DurableGateState::Waiting
            );
        }
        let mut s3 = HitlVerdictStore::new();
        let mut g3 = open_waiting();
        persist_gate_open(&mut s3, &scope(), &g3, "agent:claude").unwrap();
        g3.reject("no").unwrap();
        persist_gate_decision(&mut s3, &scope(), &g3, Some(&human_principal("psn:lead"))).unwrap();
        assert_eq!(
            s3.fetch(&scope(), &g3.gate_id.0).unwrap().state,
            DurableGateState::Rejected
        );

        let mut s4 = HitlVerdictStore::new();
        let mut g4 = open_waiting();
        persist_gate_open(&mut s4, &scope(), &g4, "agent:claude").unwrap();
        g4.expire().unwrap();
        persist_gate_decision(&mut s4, &scope(), &g4, None).unwrap();
        let rec = s4.fetch(&scope(), &g4.gate_id.0).unwrap();
        assert_eq!(rec.state, DurableGateState::Expired);
        assert_eq!(rec.decided_by, None);
    }

    // ───────── the Gated-result entry guard ─────────

    /// **The loop is entered ONLY by a `Gated` result — `Applied`/`Denied` never open a gate.**
    #[test]
    fn gate_id_of_only_for_gated() {
        assert_eq!(
            gate_id_of(&EffectResult::Gated(GateId("g".into()))),
            Some(GateId("g".into()))
        );
        assert_eq!(
            gate_id_of(&EffectResult::Applied(myelin_agent::EventId("e".into()))),
            None
        );
        assert_eq!(gate_id_of(&EffectResult::Denied("nope".into())), None);
    }
}
