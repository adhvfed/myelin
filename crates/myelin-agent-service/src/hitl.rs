//! # `hitl` — the withhold → surface → resume loop + the `hitl_gate` state machine (AG-P9 → P-221, M2-B)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §5.3 (HITL: **withhold** (a
//! gated effect returns `Gated`, does not mutate) → **surface** (a durable-workflow wait, contract
//! 9.4, surfaced as a chat approval card showing the pending action + risk + the **LIVE cost
//! estimate**, with the approver set = `list_subjects(object, approve_perm)`, contract 4.4) →
//! **decide** (minutes or days; the wait holds no runtime) → **resume** (the workflow signal re-runs
//! the step with the tool name added to "approved"; step 6 of the pipeline now passes; the effect
//! applies). Rejection settles `Halted::Rejected` with the reason in the trace + audit), §4.4 (the
//! `hitl_gate` table: `gate_id`, `run_id`, `effect_id`, `risk_summary`, `cost_estimate`,
//! `approver_filter`, `state`, `card_ref`).
//!
//! **Contract-index:** OWNS the completion of 8.2's **HITL GATE step** (the state machine the
//! `Gated` result resumes). CONSUMES 9.4 (the durable HITL signal — `state=waiting` holds NO runtime;
//! an approval/cancel signal arrives hours/days later, re-leases + replays + consumes — that
//! machinery is `myelin-flow`'s `request_approval_and_wait`, reconciled
//! below) and 4.4 (`list_subjects` → `SubjectTree`, the approver set).
//!
//! ## What this prompt ships — the agent-fabric HITL machinery on top of `EffectApi::apply`'s `Gated`
//!
//! [`crate::effect_api::PlanThenApply`] (AG-P6 → P-218) already returns
//! [`EffectResult::Gated`](myelin_agent::EffectResult) at step 6 when a `requires_approval` tool is
//! not yet in the run's `approved` set — it opens nothing, it only signals the gate (AG-8: a withheld
//! gated effect does NOT mutate). THIS module is the machinery that **resumes** that result:
//!
//! 1. **WITHHOLD → OPEN A GATE.** [`open_gate`] turns a `Gated` verdict + the [`PlannedEffect`] into a
//!    [`HitlGate`] row in state [`HitlGateState::Waiting`] — carrying the humanised `risk_summary`
//!    slot, the LIVE `cost_estimate`, the `approver_filter` (= `list_subjects(object, approve_perm)`,
//!    4.4), and the `card_ref` the durable wait surfaces as. A withheld gate holds **no runtime**.
//! 2. **SURFACE.** [`surface_card`] projects the gate into a [`HitlCard`] — the pending action, the
//!    risk, the LIVE cost estimate, and the approver set — the chat approval card a viewer sees. The
//!    durable wait itself (`myelin_flow::approval::request_approval_and_wait`, 9.4) is
//!    `myelin-flow`'s, consumed via the [`HitlWait`] seam (no production dep — the DAG stays acyclic).
//! 3. **DECIDE.** A human clicks Approve/Reject minutes-or-days later. The run is PARKED
//!    (`state=waiting`) holding no runtime; the worker is free. The decide step is observational here
//!    — the durable wait (9.4) does the multi-day park.
//! 4. **RESUME.** [`HitlGate::approve`] transitions `Waiting → Approved` and adds the tool name to the
//!    run's `approved` set ([`ApprovedTools`]); a re-run of [`crate::effect_api::PlanThenApply::apply_planned`]
//!    now passes step 6 and applies. [`HitlGate::reject`] transitions `Waiting → Rejected` and
//!    settles [`Halted::Rejected`] with the reason in the trace + audit — the effect is never applied
//!    (0 mutation).
//!
//! ## The state machine ([`HitlGateState`], §4.4 `state`)
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
//!   │ Expired  │  (the approval window lapsed → auto-deny; 0 mutation, AG-8)
//!   └──────────┘
//! ```
//!
//! Every transition is from `Waiting` ONLY (a terminal gate never re-transitions — a double-click on
//! an already-decided gate is a no-op, [`HitlGate::approve`]/[`reject`] return `Err` on a non-Waiting
//! gate). The **0-mutation-pre-approval** invariant is structural: the gate carries the effect but
//! NEVER applies it — the apply happens only when the re-run of the pipeline (with the tool now in
//! `approved`) reaches step 7. A Rejected/Expired gate never adds the tool to `approved`, so the
//! re-run gates again (or the loop withholds it) — the declined effect makes ZERO mutation (AG-8).
//!
//! ## Reconciliation with the durable-workflow HITL round-trip (9.4 — `myelin-flow::approval`)
//!
//! The **durable wait** half of the loop — emit `agent.approval.requested` via the outbox ONCE → park
//! `state=waiting` holding no runtime → resume on the `approval` signal days later, consume-exactly-
//! once across a re-drive, the per-effect `idem_key` rule (C4) — already landed in
//! `myelin-flow::approval` (`request_approval_and_wait`,
//! `per_effect_idem_key`, P-FLOW-10/11 → P-206/P-208). THIS prompt does NOT
//! re-implement it: it ships the **agent-fabric side** the durable wait drives — the `hitl_gate`
//! state machine, the card projection (action + risk + LIVE cost), the approver-set derivation
//! (`list_subjects`, 4.4), and the resume that threads the tool into the run's `approved` set so the
//! `EffectApi` step passes. The durable wait is consumed through the [`HitlWait`] seam (a caller-
//! supplied driver), keeping `myelin-agent-service` free of a production dep on `myelin-flow`.
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **Per-effect resume idempotency (C4/OQ-F) is AG-P10 (→ P-222).** A batch card gating MULTIPLE
//!   effects keys each on `card_id ":" effect_idx`; a partial approval + double-click is well-defined
//!   exactly-once. The per-effect key rule itself already landed in
//!   `per_effect_idem_key`; the exactly-once + per-effect PARITY leg
//!   (AG-D5's second half) is AG-P10. HERE the gate is single-effect (one gate per withheld effect)
//!   and the `card_id` is the single-effect key (the degenerate per-effect case, §6.4).
//! - **The humanise card-text surface is AG-P11 (→ P-223).** The `risk_summary` is a
//!   [`RiskSummary`] = `(template_key, args)` pair (NOT a raw string, C9/OQ-L); AG-P11 wires it
//!   through Notif `humanise` (contract 7.3, the ONE templating surface). HERE the card carries the
//!   humanised SLOT; the render is that follow-on.
//! - **Implicit auto-dispatch on a casual mention** remains `[OPEN → LEGAL]` L-3, handled at the Chat
//!   dispatch layer in **AG-P20**, not here (a mention notifies; it does not auto-spawn a costed run).

use crate::effect_api::{EffectCost, PlannedEffect};
use myelin_agent::{EffectResult, GateId};
use myelin_identity::{Consistency, Permission, PrincipalId, SubjectTree};
use myelin_tenancy::ArtifactRef;

// ───────────────────────── §4.4 the hitl_gate state machine — the state enum ─────────────────────

/// **The `hitl_gate.state` state machine (§4.4 `state`; AG-8).** A gate opens `Waiting`, then
/// transitions ONCE to a terminal state: `Approved` (the tool is added to the run's `approved` set; a
/// re-run of the pipeline step 6 passes and the effect applies) | `Rejected` (settles
/// [`Halted::Rejected`] with the reason; the effect is never applied — 0 mutation) | `Expired` (the
/// approval window lapsed → auto-deny; 0 mutation, AG-8). A terminal gate never re-transitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HitlGateState {
    /// the gate is open and PARKED — the durable wait holds no runtime; a human has not yet decided.
    Waiting,
    /// the gate was APPROVED — the tool is added to `approved`; the re-run applies the effect.
    Approved,
    /// the gate was REJECTED — settles [`Halted::Rejected`]; the effect is never applied (0 mutation).
    Rejected,
    /// the gate EXPIRED — the approval window lapsed → auto-deny (0 mutation, AG-8).
    Expired,
}

impl HitlGateState {
    /// The wire/audit token for the `hitl_gate.state` column (§4.4) — a stable, lowercase taxonomy
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

// ───────────────────────── the humanised risk_summary slot (C9 — AG-P11 wires humanise) ──────────

/// **The humanised `risk_summary` SLOT (§4.4; C9/OQ-L) — a `(template_key, args)` pair, NEVER a raw
/// string.** The card's `risk_summary` is never an agent-authored raw string: it is a stable
/// `template_key` + ordered `(name, ArtifactRef)` args that Notif `humanise` (contract 7.3, the ONE
/// templating surface) renders per-viewer/locale, permission- + erasure-safe.
///
/// **Floor (named):** the humanise WIRING (the `humanise((template_key, args), viewer, locale)` call
/// that renders this slot into card text) is **AG-P11 (→ P-223)**. HERE the gate carries the SLOT;
/// the render is that follow-on. Modeling it as `(key, args)` now means AG-P11 wires a render, it
/// does not re-shape the data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiskSummary {
    /// the stable template key (e.g. `agent.hitl.merge_pr`) — a taxonomy token, never free text.
    pub template_key: String,
    /// the ordered humanise args — `(arg_name, ArtifactRef)` (references-not-payloads, §3.4). The
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

// ───────────────────────── §4.4 the hitl_gate row + the state machine ─────────────────────────────

/// **A `Halt` settlement of a run (§5.3).** A REJECTED gate settles `Halted::Rejected(reason)` — the
/// run halts with the reason recorded in the trace + audit; the effect is never applied (0 mutation,
/// AG-8). (The reason is a machine token / template ref, not PII; the loud audit trail proves WHY the
/// run halted.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Halted {
    /// the HITL gate was rejected — the reason is recorded in the trace + audit (0 mutation, AG-8).
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

/// **The `hitl_gate` row + its state machine (§4.4; the OWNED completion of 8.2's HITL GATE step).**
/// Field list frozen by §4.4: `gate_id`, `run_id`, `effect_id`, `risk_summary` (the humanised SLOT),
/// `cost_estimate` (the LIVE estimate, integer minor-units, never floats), `approver_filter` (=
/// `list_subjects(object, approve_perm)`, 4.4), `state`, `card_ref`. A gate holds NO runtime while it
/// waits — the durable wait (9.4) parks the run; this struct is the durable row the wait re-leases.
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
    /// the humanised risk summary SLOT the card shows (`(template_key, args)`, C9 — AG-P11 renders it).
    pub risk_summary: RiskSummary,
    /// the LIVE cost estimate the card shows, integer minor-units (never floats) — the metered cost of
    /// the withheld effect (wholesale + markup, the BUDGET would debit on apply).
    pub cost_estimate: u64,
    /// the APPROVER set = `list_subjects(object, approve_perm)` (4.4) — a set of OPAQUE principal
    /// pseudonyms (4.8), never raw names. Who MAY approve this gate.
    pub approver_filter: Vec<PrincipalId>,
    /// the gate state (the §4.4 state machine).
    pub state: HitlGateState,
    /// the `card_ref` the gate surfaces as (the chat approval card; the durable-wait `idem_key` base).
    pub card_ref: String,
}

impl HitlGate {
    /// **WITHHOLD → OPEN: build a `Waiting` gate from a [`PlannedEffect`] + the [`GateId`] the step-6
    /// `Gated` verdict carried (§5.3 withhold).** The gate carries the humanised risk slot, the LIVE
    /// cost estimate (the effect's metered cost — what the BUDGET would debit on apply), the approver
    /// set, and the `card_ref` the durable wait surfaces as. Opening a gate makes NO mutation (AG-8) —
    /// it is a durable row, not an apply.
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

    /// **RESUME → APPROVE: transition `Waiting → Approved` (§5.3 resume).** The caller then adds
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

    /// **RESUME → REJECT: transition `Waiting → Rejected` + settle [`Halted::Rejected`] (§5.3
    /// resume).** The reason is recorded in the trace + audit; the tool is NEVER added to `approved`,
    /// so the effect is never applied (0 mutation, AG-8). Refused (a no-op) if already terminal.
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
    /// applied (0 mutation, AG-8); settles [`Halted::Expired`]. Refused (a no-op) if already terminal.
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

/// **The LIVE cost estimate the card shows (§5.3 — the "LIVE cost estimate") — the withheld effect's
/// metered total, integer minor-units.** It is the cost the BUDGET would debit on apply
/// (`wholesale + markup`), so the human approves with the real bill in view (no surprise charge). A
/// pure function of the effect's [`EffectCost`] — never a float, never a guess.
pub fn live_cost_estimate(cost: &EffectCost) -> u64 {
    cost.total()
}

// ───────────────────────── the surfaced card (action + risk + LIVE cost + approvers) ─────────────

/// **The chat approval card the gate surfaces as (§5.3 surface).** Projects a [`HitlGate`] into the
/// card a viewer sees: the pending ACTION (tool + object), the humanised RISK summary slot, the LIVE
/// COST estimate, and the APPROVER set (who may decide). The card's TEXT is rendered by Notif
/// `humanise` (AG-P11 → P-223, the ONE templating surface) from [`Self::risk_summary`] — HERE the
/// card carries the slot + the structured fields, not a rendered string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitlCard {
    /// the gate this card surfaces (the durable row the durable wait re-leases).
    pub gate_id: GateId,
    /// the pending action (the tool the effect invokes).
    pub action_tool: String,
    /// the object the action targets.
    pub action_object: ArtifactRef,
    /// the humanised risk summary SLOT (`(template_key, args)`, C9 — AG-P11 renders it per-viewer).
    pub risk_summary: RiskSummary,
    /// the LIVE cost estimate, integer minor-units (the bill the human approves with in view).
    pub cost_estimate: u64,
    /// the approver set (who MAY decide) = `list_subjects(object, approve_perm)` (4.4).
    pub approvers: Vec<PrincipalId>,
    /// the `card_ref` (the durable-wait `idem_key` base; the chat card identity).
    pub card_ref: String,
}

/// **SURFACE: project a `Waiting` [`HitlGate`] into the [`HitlCard`] a viewer sees (§5.3 surface).**
/// The card shows the pending action + risk + the LIVE cost estimate + the approver set — exactly the
/// three things AG-D5 asserts a withhold card carries (action + risk + cost). A purely observational
/// projection (no mutation, no state change) — the surface is the read side of the durable wait.
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

// ───────────────────────── the consumer seams (4.4 list_subjects, 9.4 durable wait) ──────────────

/// **The contract-4.4 `list_subjects` surface, as the HITL machinery consumes it (CONSUMED, §5.3 —
/// the approver set).** A seam so `myelin-agent-service` does NOT depend on `myelin-identity-service`
/// (the engine body) — the same decoupling [`crate::effect_api::CapabilityCheck`] uses. The approver
/// set = `list_subjects(object, approve_perm)`: the subjects who hold the approval permission on the
/// gated object, at the run's zookie snapshot. The CDC pairs this consumer with the real Identity
/// `list_subjects` provider (`tests/cdc_4_4_list_subjects.rs`).
pub trait ApproverSet {
    /// **`list_subjects` (4.4)** — the subject userset tree of who holds `approve_perm` on `object` at
    /// the consistency `at`. The HITL card's `approver_filter` is its [`SubjectTree::members`].
    fn list_subjects(
        &self,
        object: &ArtifactRef,
        approve_perm: &Permission,
        at: &Consistency,
    ) -> SubjectTree;
}

/// **Derive the approver set for a gated effect (§5.3 — `list_subjects(object, approve_perm)`, 4.4).**
/// The set of principals who MAY approve this gate = who holds `approve_perm` on the gated object at
/// the run's zookie snapshot. Returns the opaque pseudonyms (4.8), never raw names — the `card.approvers`
/// + the `hitl_gate.approver_filter`.
pub fn derive_approver_set<A: ApproverSet>(
    approvers: &A,
    object: &ArtifactRef,
    approve_perm: &Permission,
    at: &Consistency,
) -> Vec<PrincipalId> {
    approvers.list_subjects(object, approve_perm, at).members
}

/// **The durable HITL wait, as the agent fabric consumes it (CONSUMED, contract 9.4).** A seam over
/// the durable-workflow `request_approval_and_wait` round-trip (`myelin_flow::approval`,
/// P-FLOW-10/11): emit `agent.approval.requested` via the outbox ONCE → park `state=waiting` holding
/// NO runtime → resume on the `approval` signal days later (consume-exactly-once across a re-drive).
/// Modeled as a seam so `myelin-agent-service` does NOT take a production dep on `myelin-flow` (the
/// DAG stays acyclic — `myelin-flow` already depends on the agent-fabric effect set; the reverse edge
/// would be a cycle). The production wiring hands the real `WfCtx`-driven round-trip; the CDC pairs
/// this consumer with the real `myelin-flow` provider (`tests/cdc_9_4_durable_hitl.rs`).
pub trait HitlWait {
    /// **Park on the durable HITL wait for `gate` (9.4).** Returns the human's [`WaitDecision`] when
    /// the `approval` signal arrives (days later) — `Approve`, `Reject(reason)`, or `Expired` (the
    /// window lapsed). While parked, the run holds NO runtime (the worker is free). A re-drive replays
    /// the prefix + consumes the signal EXACTLY once.
    fn park_and_wait(&self, gate: &HitlGate) -> WaitDecision;
}

/// **The human's decision returned by the durable HITL wait (9.4).** `Approve` (the effect applies),
/// `Reject(reason)` (the effect is withheld → 0 mutation, AG-8), or `Expired` (the window lapsed →
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

/// **The run's `approved` tool set — the slot the resume threads a decided gate into (§5.3 resume).**
/// `EffectApi::apply`'s step 6 reads exactly this set: a `requires_approval` tool in the set passes
/// the gate and applies. A fresh run's set is empty (every gated tool withholds); the HITL resume
/// adds an APPROVED gate's tool name so the re-run applies it. This is the bridge between the
/// `hitl_gate` machine (this module) and [`crate::effect_api::PlanThenApply::approved`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApprovedTools(pub std::collections::BTreeSet<String>);

impl ApprovedTools {
    /// A fresh (empty) approved set — every `requires_approval` tool withholds until a gate approves.
    pub fn new() -> ApprovedTools {
        ApprovedTools::default()
    }

    /// **RESUME: add an APPROVED gate's tool to the set (§5.3 resume).** After this, a re-run of the
    /// pipeline passes step 6 for that tool. Refused (a no-op, returns `false`) if the gate is NOT
    /// approved (a Rejected/Expired/Waiting gate NEVER threads its tool into `approved` — the effect
    /// is never applied, AG-8). Idempotent: adding an already-approved tool is a no-op (a double-click
    /// is one approval — the set is the truth, not the click count).
    pub fn admit(&mut self, gate: &HitlGate) -> bool {
        if !gate.is_approved() {
            return false;
        }
        self.0.insert(gate.tool_name.clone());
        true
    }

    /// Whether `tool` is approved for this run (the step-6 read).
    pub fn contains(&self, tool: &str) -> bool {
        self.0.contains(tool)
    }

    /// The set as the [`crate::effect_api::PlanThenApply::approved`] field expects it (a `BTreeSet`).
    pub fn as_set(&self) -> std::collections::BTreeSet<String> {
        self.0.clone()
    }
}

// ───────────────────────── the end-to-end withhold→surface→resume loop driver ────────────────────

/// **The outcome of the full withhold → surface → resume loop for one gated effect (§5.3).** Either
/// the effect was APPROVED (the tool admitted to `approved`; the caller re-runs the pipeline → applies)
/// or it was HALTED (rejected/expired → settles [`Halted`]; 0 mutation, AG-8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HitlOutcome {
    /// the gate was APPROVED — the tool is now in the run's [`ApprovedTools`]; the caller re-runs the
    /// pipeline step (which now passes step 6 → applies). Carries the approved gate.
    Approved(HitlGate),
    /// the gate was HALTED (rejected or expired) — settles [`Halted`]; the effect is never applied
    /// (0 mutation, AG-8). Carries the halt settlement (the reason recorded in the trace + audit).
    Halted(Halted),
}

/// **Drive the withhold → surface → resume loop for ONE gated effect (§5.3, the AG-D5 withhold/resume
/// leg).** Given the `Gated` verdict's [`GateId`], the [`PlannedEffect`], the approver set, and the
/// durable-wait + approved-set seams:
///
/// 1. **WITHHOLD → OPEN** a `Waiting` gate ([`HitlGate::open`]) — 0 mutation.
/// 2. **SURFACE** the card ([`surface_card`]) — the durable wait emits `agent.approval.requested`
///    (9.4) carrying the card.
/// 3. **DECIDE** — park on the durable wait ([`HitlWait::park_and_wait`]); the run holds no runtime
///    until a human decides (days later).
/// 4. **RESUME** — on `Approve`, transition the gate `→ Approved` + admit the tool to `approved`
///    ([`ApprovedTools::admit`]) so a re-run applies the effect → [`HitlOutcome::Approved`]; on
///    `Reject`/`Expired`, transition `→ Rejected`/`Expired` + settle [`Halted`] → [`HitlOutcome::Halted`]
///    (0 mutation, AG-8).
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

    // 2 + 3. SURFACE + DECIDE — park on the durable wait (9.4); the run holds NO runtime until a
    //        human decides (days later). `surface_card` is the card the wait emits.
    let _card = surface_card(&gate);
    let decision = wait.park_and_wait(&gate);

    // 4. RESUME — transition the gate per the decision + thread the result.
    match decision {
        WaitDecision::Approve => {
            // approve is infallible here (the gate is freshly Waiting); admit the tool so the re-run
            // applies the effect (the caller re-runs apply_planned with the populated approved set).
            gate.approve().expect("a freshly-opened gate is Waiting");
            approved.admit(&gate); // thread the tool into `approved` (idempotent).
            HitlOutcome::Approved(gate)
        }
        WaitDecision::Reject(reason) => {
            let halted = gate
                .reject(reason)
                .expect("a freshly-opened gate is Waiting");
            HitlOutcome::Halted(halted)
        }
        WaitDecision::Expired => {
            let halted = gate.expire().expect("a freshly-opened gate is Waiting");
            HitlOutcome::Halted(halted)
        }
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

    /// **RESUME threads an APPROVED gate's tool into `approved`; a Rejected/Expired gate NEVER does
    /// (0 mutation, AG-8); admit is idempotent (a double-click is one approval).**
    #[test]
    fn approved_set_admits_only_approved_gates_idempotently() {
        let mut approved = ApprovedTools::new();
        assert!(!approved.contains("git.merge"));

        // a Waiting gate is NOT admitted (no decision yet).
        let waiting = open_waiting();
        assert!(!approved.admit(&waiting), "a Waiting gate threads nothing");
        assert!(!approved.contains("git.merge"));

        // an Approved gate IS admitted → the re-run applies the effect.
        let mut g = open_waiting();
        g.approve().unwrap();
        assert!(
            approved.admit(&g),
            "an Approved gate threads its tool into approved"
        );
        assert!(approved.contains("git.merge"));
        // idempotent: a double-click (re-admit) is a no-op.
        assert!(approved.admit(&g));
        assert_eq!(
            approved.as_set().len(),
            1,
            "a double-click is one approval (one entry)"
        );

        // a Rejected gate NEVER threads its tool (0 mutation, AG-8).
        let mut r = open_waiting();
        r.tool_name = "git.force_push".into();
        r.reject("no").unwrap();
        assert!(
            !approved.admit(&r),
            "a Rejected gate NEVER approves the tool (AG-8)"
        );
        assert!(!approved.contains("git.force_push"));
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
        // the resume threaded the tool into `approved` → a re-run of apply_planned now passes step 6.
        assert!(
            approved.contains("git.merge"),
            "the approved tool is now in the run's approved set"
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
        // the rejected gate NEVER threaded the tool → the re-run gates again / the loop withholds.
        assert!(
            !approved.contains("git.merge"),
            "a rejected gate makes 0 mutation (the tool stays unapproved, AG-8)"
        );
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
