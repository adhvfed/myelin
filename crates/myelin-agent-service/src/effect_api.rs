//! # `effect_api` — plan-then-apply `EffectApi::apply`: the schema → capability → delegation →
//! tenant → budget → HITL-gate → apply → meter pipeline (AG-P6 → P-218, M2-B)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §5.2 (the plan-then-apply
//! pipeline, **in order, fail-closed**: SCHEMA → CAPABILITY (with the `CaveatContext` for
//! field/transition ABAC, evaluated at check-time off the hot `list_objects` path) → DELEGATION
//! (`agent.policy ∩ delegation ∩ tenant.policy`, **intersection never union**, attenuation never
//! up) → TENANT → BUDGET → HITL GATE → APPLY via the subsystem's PUBLIC endpoint as the agent
//! principal (same gateway, no carve-out) → METER), §5.0 (the routing table: `mutate`/`external`
//! go through `EffectApi`). Reconciliation §X-6 (the four uniform guarantees), §OQ-E (the
//! `CaveatContext`).
//!
//! **Contract-index:** OWNS the **body** of 8.2 (`EffectApi::apply`). CONSUMES 4.2 (`check` +
//! `CaveatContext`), 4.5 (`delegation` → `EffectivePolicy`, the ∩ algebra), 11.7 (BUDGET /
//! reserve-settle). The glue trait [`myelin_agent::EffectApi`] (8.2, signature half, AG-P1 →
//! P-130) is implemented here; the eight-step pipeline IS the body the glue crate named as a floor.
//!
//! ## What this prompt ships — the eight-step fail-closed pipeline
//!
//! Agents are a pure-ish function `(event, context) → { effects }`; they **NEVER side-effect
//! directly** (ADR-08.3 / EI-03 §2/§4). The brain emits a `ProposedEffect`; [`PlanThenApply::apply`]
//! validates each through the pipeline and applies it via the subsystem's PUBLIC endpoint as the
//! agent principal. The eight steps, **in order, fail-closed** (a step that cannot affirmatively
//! allow → `Denied`, never a silent allow):
//!
//! 1. **SCHEMA** — validate `effect.input` against the [`ToolDef`] JSON Schema; malformed → `Denied`.
//! 2. **CAPABILITY** — `Id.check(agent_principal, required_cap, object, zookie, caveat)` for every
//!    `required_cap`; the caveat carries the field/transition ABAC ([`CaveatContext`]) evaluated
//!    HERE, off the hot `list_objects` path (OQ-E). Any `Deny`/`Conditional` → `Denied`.
//! 3. **DELEGATION** — `Id.delegation(agent, trigger_actor) → agent.policy ∩ delegation ∩
//!    tenant.policy`; the required caps must be **inside** the intersection (attenuation, never up).
//! 4. **TENANT** — the tenant guardrails (agent-allow-list, residency, AI-Act); a forbidden effect
//!    → `Denied`.
//! 5. **BUDGET** — the reserve has remaining balance for this effect's metered cost (11.7); no
//!    balance → `Denied` (no privileged fallback).
//! 6. **HITL GATE** — if `tool_def.requires_approval` AND not yet approved for this run → **WITHHELD**:
//!    return `Gated` and STOP — the tool does NOT mutate (AG-8). The HITL machinery (the
//!    withhold → surface → resume loop) is AG-P9 (→ P-221); HERE we only return `Gated`.
//! 7. **APPLY** — call the subsystem's PUBLIC endpoint as the agent principal (same gateway, no
//!    carve-out, EI-03 §4) ⇒ the subsystem emits its domain event via ITS outbox. Returns the
//!    `event_id`.
//! 8. **METER** — settle exactly one cost event for this effect (11.7); wholesale ≠ markup kept
//!    distinct.
//!
//! → [`EffectResult`] ∈ `{ Applied(event_id) | Gated(gate_id) | Denied(reason) }`.
//!
//! **The denied path:** an effect outside the ∩, a schema reject, a tenant deny, or a budget
//! refusal returns an ordinary `Denied` tool error — **NO privileged fallback** (AG-5). The denial
//! is surfaced LOUD and counted (the [`PipelineSignals`] denial counter; the fallback counter is
//! ALWAYS 0 by construction — there is no fallback code path).
//!
//! ## The consumer seams (the same trait-decoupling `skeleton.rs` uses)
//! `myelin-agent-service` is a LEAF CONSUMER; it does NOT take a production dep on
//! `myelin-identity-service` (the engine bodies). The pipeline consumes Identity through two
//! **seams** ([`CapabilityCheck`] for 4.2 `check`, [`DelegationLookup`] for 4.5 `delegation`), the
//! tenant guardrails through [`TenantGuard`], and the subsystem PUBLIC endpoint through
//! [`SubsystemApply`] (the only mutation path — there is no second write path). The provider+consumer
//! CDC pairs each seam with a real provider (`tests/`).
//!
//! ## FLOORS named (cross-references; VISION §3, EI-01 §1)
//! - **The pipeline is identical for mock and real — there is NO floor on `EffectApi`.** The mock
//!   and the LLM brain emit `ProposedEffect`s into the SAME pipeline.
//! - **The delegation-scoped tool-list** (the `list_objects` SetExpr push-down — an optimisation
//!   that pre-scopes the tool list the brain sees) is **AG-P7 (→ P-219)**. The apply-time re-check
//!   HERE is the guarantee; the push-down is the optimisation that feeds it.
//! - **The `requires_approval` per-subsystem DEFAULTS seed + the `run --dry-run` lever** are
//!   **AG-P8 (→ P-220)**. The gate COLUMN is read HERE (step 6); the defaults are SEEDED there.
//! - **The HITL machinery** (the withhold → surface → resume loop, the `hitl_gate` state machine,
//!   per-effect resume idempotency) is **AG-P9/P10/P11 (→ P-221/P-222/P-223)**. HERE step 6 only
//!   returns `Gated` (opens nothing) — the machinery resumes that result.
//! - **The reserve/settle cost-gate runaway self-limiter** (AG-D11) is **AG-P14 (→ P-227)**. HERE
//!   the BUDGET step reads the reserve balance (11.7); the runaway-loop drill is AG-P14's.

use myelin_agent::{
    EffectApi, EffectAuthority, EffectKind, EffectResult, EventId, GateId, ProposedEffect, RunCtx,
    ToolCall, ToolDef, ToolName, ToolSurface,
};
use myelin_identity::{
    CaveatContext, Consistency, ConsistencyMode, Decision, EffectivePolicy, FieldId, Permission,
    Principal, TransitionId, Zookie,
};
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_storage::{
    reserve_settle::{MeteredUnit, MinorUnits},
    TenantScope,
};
use myelin_tenancy::{ArtifactRef, Region};

// ───────────────────────── the structured planned effect (the brain's proposal) ─────────────────

/// **A structured proposed effect the brain emitted (the engine's working shape).** The glue
/// [`ProposedEffect`] (8.2) is an opaque-string carrier across the crate boundary; HERE the engine
/// parses it into the structured plan it validates through the pipeline. Carries: the tool the
/// effect invokes (the [`ToolName`] key into the [`ToolSurface`] catalogue), the target object
/// (`ArtifactRef`), the JSON input the brain authored, and the optional field/transition the ABAC
/// caveat gates (OQ-E). Built by the dispatch/loop tier when it routes a `mutate`/`external`
/// `UseTools` call (§5.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedEffect {
    /// The tool the effect invokes (the catalogue key — resolved against the [`ToolSurface`]).
    pub tool: ToolName,
    /// The target object the effect mutates (the `check` object, the apply target).
    pub object: ArtifactRef,
    /// The JSON input the brain authored (validated against the [`ToolDef`] schema at step 1).
    pub input_json: String,
    /// The field the effect writes, if field-level ABAC applies (OQ-E; e.g. a KN confidential
    /// field). Carried into the [`CaveatContext`] at step 2.
    pub field: Option<FieldId>,
    /// The state transition the effect performs, if transition-level ABAC applies (OQ-E; e.g. an
    /// Issues SLA-bound `transition(issue, →done)` gated on the approver edge). Carried into the
    /// caveat at step 2.
    pub transition: Option<TransitionId>,
    /// The metered cost of this effect, as a `(unit, wholesale, markup)` row (the BUDGET step reads
    /// the total; the METER step settles exactly this). Integer minor-units (never floats).
    pub cost: EffectCost,
}

/// **The metered cost of one effect — `(unit, wholesale, markup)`, integer minor-units (11.7).**
/// The BUDGET step (5) checks the reserve has ≥ `total()` remaining; the METER step (8) settles
/// exactly one cost event with this split (wholesale ≠ markup kept distinct, C-1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectCost {
    /// The metered-unit dimension this effect bills (e.g. `issue.transition`).
    pub unit: &'static str,
    /// The wholesale (provider) cost, minor-units.
    pub wholesale: u64,
    /// The markup (platform margin), minor-units — recorded DISTINCTLY from wholesale (C-1).
    pub markup: u64,
}

impl EffectCost {
    /// The total metered cost (`wholesale + markup`), saturating (a cost never silently wraps).
    pub fn total(&self) -> u64 {
        self.wholesale.saturating_add(self.markup)
    }

    /// The exact total when it is representable. Admission must use this form: an overflowing
    /// price cannot be faithfully reserved or settled and therefore must never reach mutation.
    pub fn checked_total(&self) -> Option<u64> {
        self.wholesale.checked_add(self.markup)
    }

    /// As the Storage [`MeteredUnit`] the settle records (one cost event per metered unit, 11.7).
    fn as_metered_unit(&self) -> MeteredUnit {
        MeteredUnit {
            unit: self.unit,
            wholesale: MinorUnits(self.wholesale),
            markup: MinorUnits(self.markup),
        }
    }
}

/// **Serialise a [`PlannedEffect`] into the opaque glue [`ProposedEffect`] carrier (8.2).** The
/// glue crate's [`ProposedEffect`] is an opaque string across the crate boundary; the dispatch tier
/// builds the structured plan, this stamps it onto the carrier, and [`PlanThenApply::apply`] parses
/// it back. Deterministic (a golden-fixture round-trip), so two runs over the same plan produce
/// byte-identical carriers (the AG-D9 proposed-effect-sequence determinism AG-P8 re-asserts).
pub fn encode_proposed(plan: &PlannedEffect) -> ProposedEffect {
    // A stable, field-ordered encoding (NOT a serde blob — a fixed, auditable shape).
    let field = plan.field.as_ref().map(|f| f.0.as_str()).unwrap_or("");
    let transition = plan.transition.as_ref().map(|t| t.0.as_str()).unwrap_or("");
    ProposedEffect(format!(
        "tool={}\u{1f}object={}\u{1f}field={}\u{1f}transition={}\u{1f}unit={}\u{1f}wholesale={}\u{1f}markup={}\u{1f}input={}",
        plan.tool.0,
        plan.object.0,
        field,
        transition,
        plan.cost.unit,
        plan.cost.wholesale,
        plan.cost.markup,
        plan.input_json,
    ))
}

/// **The PER-EFFECT gate key (R2.4 — the step-6 approval granularity).** One approval authorizes
/// exactly one `(tool, object)` effect: this is both the `GateId` step 6 mints on a withhold AND
/// the key the step-6 consult reads back from the run's `approved` set (one derivation — the mint
/// and the consult cannot diverge). An [`crate::hitl::ApprovedTools::admit`] threads exactly this
/// key; a bare tool name is never an approval key anywhere.
pub fn effect_gate_key(tool: &ToolName, object: &ArtifactRef) -> String {
    effect_gate_key_str(&tool.0, &object.0)
}

/// The string-typed derivation behind [`effect_gate_key`] (for callers holding the raw
/// `hitl_gate.tool_name` / object strings, e.g. the HITL resume threading).
pub fn effect_gate_key_str(tool: &str, object: &str) -> String {
    format!("gate:{tool}:{object}")
}

// ───────────────────────── the consumer seams (CONSUMED: 4.2, 4.5, tenant, apply) ───────────────

/// **The contract-4.2 `check` surface, as the engine consumes it (CONSUMED, §5.2 step 2).** A seam
/// so `myelin-agent-service` does NOT depend on `myelin-identity-service` (the same decoupling
/// `skeleton.rs`'s [`crate::skeleton::RunTokenRevoker`] uses — the DAG stays acyclic). The CDC pairs
/// this consumer with the real Identity `check` provider (`tests/cdc_4_2_capability_check.rs`).
///
/// The caveat carries the field/transition ABAC (OQ-E), evaluated HERE off the hot `list_objects`
/// path. A `Conditional` (a caveat needing missing context) is treated as a DENY — never a silent
/// allow (fail-closed, ADR-03 / §8.6).
pub trait CapabilityCheck {
    /// **`check` (4.2)** — does `subject` hold `permission` on `object` at the consistency `at`,
    /// under the optional `caveat`? Returns the per-action [`Decision`] (fail-closed).
    fn check(
        &self,
        subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &Consistency,
        caveat: Option<&CaveatContext>,
    ) -> Decision;
}

/// **The contract-4.5 `delegation` surface, as the engine consumes it (CONSUMED, §5.2 step 3).** A
/// seam (no dep on `myelin-identity-service`). Returns the [`EffectivePolicy`] — the attenuated
/// capability chain after the monotone intersection `agent.policy ∩ delegation ∩ tenant.policy`
/// (intersection, never union; attenuation, never up). The CDC pairs this with the real Identity
/// `delegation` provider (`tests/cdc_4_5_delegation.rs`).
pub trait DelegationLookup {
    /// **`delegation` (4.5)** — the run's effective policy after the intersection. The caps inside
    /// the returned [`EffectivePolicy::caveats`] are the ONLY caps the run may exercise (an agent
    /// can do nothing no human role can — EI-02 §2).
    fn delegation(&self, agent: &Principal, trigger_actor: &Principal) -> EffectivePolicy;
}

/// **The tenant guardrails (CONSUMED, §5.2 step 4 — agent-allow-list, residency, AI-Act).** A seam
/// the Tenancy/control-plane provides; the engine asks "may THIS agent run THIS effect under THIS
/// tenant's policy?" A forbidden effect → `Denied` (no carve-out).
pub trait TenantGuard {
    /// **Tenant guardrail (4.x / §5.2 step 4)** — is this effect permitted by the tenant's policy
    /// (agent allow-list, residency, AI-Act human-oversight)? `false` → `Denied`.
    fn permits(&self, agent: &Principal, tool: &ToolName, object: &ArtifactRef) -> bool;
}

/// **The subsystem PUBLIC endpoint the effect applies through (CONSUMED, §5.2 step 7 — same
/// gateway, no carve-out, EI-03 §4).** The ONLY mutation path: the engine calls the subsystem's
/// PUBLIC endpoint AS the agent principal, so the subsystem emits its domain event via ITS outbox
/// (there is no second write path; the agent never reaches into a subsystem's storage — the
/// `no-cross-db` lint, AG-D1, makes that structurally impossible). The CDC pairs this with a real
/// subsystem-endpoint provider (`tests/cdc_8_2_apply_endpoint.rs`).
pub trait SubsystemApply {
    /// **Apply via the subsystem's PUBLIC endpoint as the agent principal (step 7).** Returns the
    /// `event_id` the subsystem emitted (references-not-payloads). An endpoint error is surfaced
    /// LOUD (the apply FAILED; the meter does NOT settle a non-applied effect).
    fn apply_public(
        &self,
        agent: &Principal,
        tool: &ToolName,
        object: &ArtifactRef,
        input_json: &str,
    ) -> Result<EventId, ApplyError>;
}

/// **An error from the subsystem PUBLIC endpoint apply (step 7).** Surfaced LOUD — a failed apply
/// is NOT metered (the meter settles only an applied effect; a failed apply refunds the reserve).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyError(pub String);

impl core::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "subsystem public-endpoint apply failed: {}", self.0)
    }
}

impl std::error::Error for ApplyError {}

// ───────────────────────── the BUDGET seam (CONSUMED: 11.7) ──────────────────────────────────────

/// **The reserve/settle budget surface, as the engine consumes it (CONSUMED, §5.2 steps 5+8 —
/// 11.7).** A seam over the Storage [`reserve_settle`](myelin_storage::reserve_settle) ledger: the
/// BUDGET step asks "does the reserve have ≥ this effect's cost remaining?" and the METER step
/// settles exactly one cost event. The runaway self-limiter (reserve REFUSES past an exhausted
/// wallet) is AG-D11 / AG-P14; HERE the budget is the per-effect remaining-balance check.
pub trait EffectBudget {
    /// **BUDGET (step 5)** — does the run's reserve have ≥ `cost` minor-units remaining for this
    /// effect? `false` → `Denied` (no privileged fallback — the run cannot spend past its reserve).
    fn has_remaining(&self, cost: u64) -> bool;

    /// **METER (step 8)** — settle exactly one cost event for this applied effect (wholesale ≠
    /// markup kept distinct). Called ONLY after a successful apply (a non-applied effect is never
    /// metered). Returns the billed total (the bill the run reports).
    fn settle_one(&mut self, unit: &MeteredUnit) -> u64;
}

// ───────────────────────── the eight-step pipeline (8.2 — the OWNED body) ────────────────────────

/// **The verdict a single pipeline run produced, with the step that decided it (the audit fact the
/// `proposed_effect` row records).** Distinct from [`EffectResult`] (the glue outcome): this names
/// WHICH step denied/gated so the trail proves where the pipeline fail-closed (EI-01 §3 — a
/// property does not exist until a test forces the failure at each step).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineStep {
    /// Step 1 — the input failed the [`ToolDef`] JSON-schema validation.
    Schema,
    /// Step 2 — `check` returned Deny/Conditional for a required cap.
    Capability,
    /// Step 3 — a required cap was outside the delegation intersection (over-privilege).
    Delegation,
    /// Step 4 — the tenant guardrails forbade the effect.
    Tenant,
    /// Step 5 — the reserve had no remaining balance for the effect's cost.
    Budget,
    /// Step 6 — the tool `requires_approval` and is not yet approved (WITHHELD → Gated).
    HitlGate,
    /// Step 7 — the subsystem public-endpoint apply failed (a loud apply error).
    Apply,
    /// The pipeline ran to completion (Applied + metered).
    Applied,
}

/// **The verdict of the steps-1..6 gate run (the dry-run plan + the apply's pre-mutation decision).**
/// The pipeline's first SIX steps (SCHEMA → CAPABILITY → DELEGATION → TENANT → BUDGET → HITL-GATE)
/// are SIDE-EFFECT-FREE: they validate but never mutate or meter. This is exactly what `run --dry-run`
/// returns (8.7 — *steps 1..6, no apply*), and exactly the decision [`apply_planned`](PlanThenApply::apply_planned)
/// branches on before step 7. Extracting it makes the dry-run and the live apply share ONE code path
/// (no second implementation — the plan a dry-run shows IS the plan the apply executes, AG-D9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanVerdict {
    /// Steps 1..6 all PASS and the tool is NOT gated → the effect WOULD apply (step 7) if run live.
    WouldApply,
    /// Step 6 WITHHELD the effect (`requires_approval` + not yet approved) → it WOULD gate (does NOT
    /// mutate). Carries the gate id the live apply would return.
    WouldGate(GateId),
    /// A step 1..6 DENIED the effect → it WOULD deny. Carries the deciding step + the reason.
    WouldDeny(PipelineStep, String),
}

/// **The contract-1.8 survival signals the pipeline emits (the green artifacts; §3.1, EI-01 §3).**
/// A path that denies/gates but emits NO signal has FAILED the drill. The AG-D2 drill reads the
/// **denial counter** (it increments on every Denied) and the **fallback counter** (which is ALWAYS
/// 0 — there is NO privileged-fallback code path; the field exists so the drill can ASSERT 0, not
/// because anything ever increments it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PipelineSignals {
    /// The number of effects APPLIED (ran the full eight steps).
    applied: u64,
    /// The number of effects DENIED (an ordinary tool error — counted, surfaced loud).
    denied: u64,
    /// The number of effects GATED (withheld for HITL — does NOT mutate).
    gated: u64,
    /// The number of privileged-fallback fires — **ALWAYS 0** (there is no fallback code path; the
    /// drill asserts this is 0).
    privileged_fallback: u64,
    /// The total billed across applied effects (minor-units) — the meter settled exactly the
    /// applied effects.
    metered_total: u64,
}

impl PipelineSignals {
    /// A fresh, zeroed signal set.
    pub fn new() -> PipelineSignals {
        PipelineSignals::default()
    }
    /// The number of effects APPLIED.
    pub fn applied(&self) -> u64 {
        self.applied
    }
    /// The number of effects DENIED (the AG-D2 denial counter).
    pub fn denied(&self) -> u64 {
        self.denied
    }
    /// The number of effects GATED (withheld).
    pub fn gated(&self) -> u64 {
        self.gated
    }
    /// The number of privileged-fallback fires — **ALWAYS 0** (no fallback path exists; AG-D2).
    ///
    /// **Mutation note (measured):** `cargo-mutants` reports `replace … -> 0` as MISSED on this
    /// accessor. That mutant is **provably equivalent**: the field is only ever its `Default` (0) and
    /// NO method in this crate mutates it (there is no privileged-fallback code path — that absence
    /// IS the AG-D2 property). Returning `0` and returning `self.privileged_fallback` are the
    /// identical function, so no test can distinguish them. The invariant itself is forced by
    /// `privileged_fallback_stays_zero_across_every_outcome` (the counter is 0 across applied /
    /// gated / denied / apply-failed). This is the one named equivalent-mutant survivor; the
    /// apply-pipeline mutation score is otherwise 39/40 caught (the 6 unviable are type-only).
    pub fn privileged_fallback(&self) -> u64 {
        self.privileged_fallback
    }
    /// The total billed across applied effects (minor-units).
    pub fn metered_total(&self) -> u64 {
        self.metered_total
    }
}

/// **8.2 — the plan-then-apply `EffectApi` (the OWNED eight-step pipeline body).** Platform-owned —
/// identical for mock and real (the whole point of plan-then-apply; agents NEVER mutate directly).
/// Holds the consumer seams (Identity `check`/`delegation`, the tenant guard, the budget, the
/// subsystem apply endpoint) + the [`ToolSurface`] catalogue the schema/cap/gate steps read.
///
/// The lifetimes are the engine's: the seams + the catalogue are borrowed for the run; the budget +
/// signals are mutably borrowed (the meter settles + the signals record). Construct it per-run from
/// the dispatch tier's substrate.
pub struct PlanThenApply<'a, S, C, D, T, A, B>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
{
    /// The one permissioned tool catalogue (8.1) — schema/caps/gate read the [`ToolDef`].
    pub catalogue: &'a S,
    /// The contract-4.2 `check` seam (CONSUMED).
    pub check: &'a C,
    /// The contract-4.5 `delegation` seam (CONSUMED).
    pub delegation: &'a D,
    /// The tenant guardrails seam (CONSUMED).
    pub tenant: &'a T,
    /// The subsystem PUBLIC endpoint apply seam (CONSUMED — the ONLY mutation path).
    pub apply_endpoint: &'a A,
    /// The reserve/settle budget seam (CONSUMED, 11.7) — mutably borrowed (the meter settles).
    pub budget: &'a mut B,
    /// The agent principal the run acts as (the `check`/`delegation`/`apply` subject).
    pub agent: Principal,
    /// The human/system actor that triggered the run (the `delegation` `trigger_actor`).
    pub trigger_actor: Principal,
    /// The consistency the `check` reads at (the run's zookie watermark — read-your-writes).
    pub zookie: Zookie,
    /// The set of PER-EFFECT gate keys ([`effect_gate_key`] — `gate:{tool}:{object}`) already
    /// APPROVED for this run (step 6 reads this — a gated effect whose OWN key was approved
    /// passes). Empty for a fresh run; the HITL resume (AG-P9/P10) adds an approved gate's key.
    ///
    /// **R2.4 (Defect B fix):** this set held bare TOOL NAMES, so approving `git.merge` on PR 40
    /// admitted `git.merge` run-wide — a DECLINED sibling on PR 41 re-driven through
    /// `apply_planned` fell through step 6 and applied. The set now carries the SAME
    /// per-(tool, object) key the step-6 `GateId` is minted from: a declined sibling's key is
    /// never present, so it gates again (0 mutation, AG-8).
    pub approved: std::collections::BTreeSet<String>,
    /// The contract-1.8 signal set the pipeline records into (mutably borrowed).
    pub signals: &'a mut PipelineSignals,
}

impl<'a, S, C, D, T, A, B> PlanThenApply<'a, S, C, D, T, A, B>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
{
    /// **Apply a structured [`PlannedEffect`] through the eight-step pipeline (the §5.2 core).**
    /// IN ORDER, FAIL-CLOSED: a step that cannot affirmatively allow returns `Denied`/`Gated` and
    /// STOPS — there is no privileged fallback (AG-5), no carve-out (EI-03 §4), and the mutation
    /// happens ONLY at step 7 via the subsystem's PUBLIC endpoint. Records the signals (the AG-D2
    /// denial counter, the meter) and returns the glue [`EffectResult`].
    pub fn apply_planned(&mut self, plan: &PlannedEffect) -> EffectResult {
        // Steps 1..6 — the SIDE-EFFECT-FREE gate (schema → cap → delegation → tenant → budget →
        // HITL-gate). Shared verbatim with `run --dry-run` (8.7): the plan a dry-run shows IS the
        // plan the live apply executes (AG-D9 — there is no second pipeline). This records the
        // denial/gated signals (the gate IS the decision); steps 7..8 below only run on WouldApply.
        match self.plan_through_gate(plan) {
            PlanVerdict::WouldDeny(step, reason) => return self.deny(step, reason),
            PlanVerdict::WouldGate(gate_id) => {
                // Step 6 WITHHELD — the tool does NOT mutate (AG-8). Count it + return Gated.
                self.signals.gated = self.signals.gated.saturating_add(1);
                return EffectResult::Gated(gate_id);
            }
            PlanVerdict::WouldApply => {}
        }

        // (7) APPLY — call the subsystem's PUBLIC endpoint as the agent principal (same gateway, no
        //     carve-out) ⇒ the subsystem emits its domain event via ITS outbox. The ONLY mutation
        //     path. A failed apply is surfaced LOUD and is NOT metered (refund the reserve).
        let event_id = match self.apply_endpoint.apply_public(
            &self.agent,
            &plan.tool,
            &plan.object,
            &plan.input_json,
        ) {
            Ok(id) => id,
            Err(e) => return self.deny(PipelineStep::Apply, e.to_string()),
        };

        // (8) METER — settle exactly one cost event for this applied effect (wholesale ≠ markup).
        let billed = self.budget.settle_one(&plan.cost.as_metered_unit());
        self.signals.applied = self.signals.applied.saturating_add(1);
        self.signals.metered_total = self.signals.metered_total.saturating_add(billed);
        EffectResult::Applied(event_id)
    }

    /// **Run the SIDE-EFFECT-FREE gate (steps 1..6) and return the [`PlanVerdict`] (the `run
    /// --dry-run` plan, 8.7; the apply's pre-mutation decision).** SCHEMA → CAPABILITY → DELEGATION
    /// → TENANT → BUDGET → HITL-GATE, **in order, fail-closed** — but it NEVER calls the apply
    /// endpoint (step 7) and NEVER meters (step 8). The mutation + meter are the caller's
    /// ([`apply_planned`](PlanThenApply::apply_planned)) when (and only when) the verdict is
    /// [`PlanVerdict::WouldApply`].
    ///
    /// **Determinism (AG-D9):** the verdict is a pure function of `(plan, catalogue, check,
    /// delegation, tenant, budget-balance, approved-set)` — two runs over the same inputs produce
    /// byte-identical verdicts (the dry-run plan is reproducible). It does NOT mutate `self.signals`
    /// (a dry-run is observational — the denial/gated counters are the LIVE apply's, recorded by
    /// `apply_planned`; the dry-run plan is metering-free, [`DryRunPlanner`](crate::dry_run::DryRunPlanner)).
    pub fn plan_through_gate(&self, plan: &PlannedEffect) -> PlanVerdict {
        // (1) SCHEMA — the tool must be in the catalogue, and the input must validate against its
        //     JSON Schema. An unknown tool or a malformed input is Denied (fail-closed).
        let def: &ToolDef = match self.catalogue.resolve(&plan.tool) {
            Some(d) => d,
            None => {
                return PlanVerdict::WouldDeny(
                    PipelineStep::Schema,
                    format!("unknown tool {}", plan.tool.0),
                )
            }
        };
        if let Err(reason) = validate_schema(&def.input_schema, &plan.input_json) {
            return PlanVerdict::WouldDeny(PipelineStep::Schema, reason);
        }

        // Only `mutate`/`external` effects route through EffectApi (§5.0). A `read`/`compute` tool
        // reaching the apply path is a routing bug — fail-closed (never apply a non-mutate here).
        if !matches!(def.effect_kind, EffectKind::Mutate | EffectKind::External) {
            return PlanVerdict::WouldDeny(
                PipelineStep::Schema,
                format!(
                    "tool {} is {:?}, not mutate/external — it does not route through EffectApi (§5.0)",
                    plan.tool.0, def.effect_kind
                ),
            );
        }

        // The CaveatContext for step 2 — the field/transition ABAC, evaluated HERE off the hot
        // list_objects path (OQ-E). The attrs map is empty at this seam (the predicate evaluator is
        // Id's, P-ID-22); the object/field/transition carry the ABAC condition.
        let caveat = CaveatContext {
            object: plan.object.clone(),
            field: plan.field.clone(),
            transition: plan.transition.clone(),
            attrs: std::collections::BTreeMap::new(),
        };
        let at = Consistency {
            at_least: self.zookie.clone(),
            mode: ConsistencyMode::Strong,
        };

        // (2) CAPABILITY — the run's per-run identity must hold EVERY required cap (intersection,
        //     fail-closed). Any Deny/Conditional → Denied. The caveat carries the ABAC condition.
        for cap in &def.required_caps {
            let permission = Permission(cap.clone());
            match self
                .check
                .check(&self.agent, &permission, &plan.object, &at, Some(&caveat))
            {
                Decision::Allow => {}
                // Conditional == a caveat needs context the run did not supply → DENY, never a
                // silent allow (fail-closed, ADR-03 / §8.6).
                Decision::Deny | Decision::Conditional => {
                    return PlanVerdict::WouldDeny(
                        PipelineStep::Capability,
                        format!("capability check denied for {cap}"),
                    );
                }
            }
        }

        // (3) DELEGATION — agent.policy ∩ delegation ∩ tenant.policy (intersection, never up). The
        //     required caps must be INSIDE the effective policy. A cap the agent's policy allows but
        //     the delegation/tenant forbids (and vice-versa) is confined to the intersection.
        let policy: EffectivePolicy = self.delegation.delegation(&self.agent, &self.trigger_actor);
        for cap in &def.required_caps {
            if !policy.caveats.iter().any(|c| c == cap) {
                return PlanVerdict::WouldDeny(
                    PipelineStep::Delegation,
                    format!(
                        "{cap} is outside the delegation intersection \
                         (agent.policy ∩ delegation ∩ tenant.policy) — attenuation never up"
                    ),
                );
            }
        }

        // (4) TENANT — the tenant guardrails (agent-allow-list, residency, AI-Act). Forbidden →
        //     Denied (no carve-out).
        if !self.tenant.permits(&self.agent, &plan.tool, &plan.object) {
            return PlanVerdict::WouldDeny(
                PipelineStep::Tenant,
                format!(
                    "tenant guardrails forbid {} on {}",
                    plan.tool.0, plan.object.0
                ),
            );
        }

        // (5) BUDGET — the reserve has remaining balance for this effect's metered cost (11.7). No
        //     balance → Denied (no privileged fallback — the run cannot spend past its reserve).
        let Some(cost) = plan.cost.checked_total() else {
            return PlanVerdict::WouldDeny(
                PipelineStep::Budget,
                "metered cost exceeds the supported minor-unit range".into(),
            );
        };
        if !self.budget.has_remaining(cost) {
            return PlanVerdict::WouldDeny(
                PipelineStep::Budget,
                format!("reserve has no remaining balance for cost {cost} minor-units"),
            );
        }

        // (6) HITL GATE — if the tool requires_approval AND this EFFECT's per-(tool, object) gate
        //     key is not yet approved for this run → WITHHELD: the tool does NOT mutate (AG-8).
        //     The HITL machinery (open the card, surface, resume) is AG-P9 — HERE we only signal
        //     the gate. The COLUMN read here is the FROZEN §6.3 default (seeded at registration,
        //     [`crate::defaults`]). R2.4: the consult is keyed by [`effect_gate_key`] — the SAME
        //     key the `GateId` below is minted from — never by the bare tool name, so an approved
        //     sibling sharing a tool name can NEVER admit a declined effect.
        let gate_key = effect_gate_key(&plan.tool, &plan.object);
        if def.requires_approval && !self.approved.contains(&gate_key) {
            return PlanVerdict::WouldGate(GateId(gate_key));
        }

        PlanVerdict::WouldApply
    }

    /// Record a DENIED verdict at `step` and return the ordinary `Denied` tool error (NO privileged
    /// fallback — the deny is the end of the line, surfaced loud + counted, AG-5/AG-D2).
    fn deny(&mut self, _step: PipelineStep, reason: String) -> EffectResult {
        self.signals.denied = self.signals.denied.saturating_add(1);
        EffectResult::Denied(reason)
    }
}

/// **8.2 — the glue [`EffectApi`] frozen-shape bridge.** Bridges the opaque [`ProposedEffect`]
/// carrier to the structured pipeline: parse the carrier → [`apply_planned`](PlanThenApply::apply_planned).
/// A carrier that cannot be parsed is `Denied` (fail-closed — a malformed proposal never mutates).
///
/// **Note on `&self`:** the glue trait is `fn apply(&self, ...)`, but the pipeline mutates the
/// budget meter + the signal set. The bridge holds the [`PlanThenApply`] behind a [`core::cell::RefCell`]
/// (a local newtype — the orphan rule forbids `impl EffectApi for RefCell<…>` directly) so the OWNED
/// body satisfies the frozen `&self` signature without changing the glue contract (8.2 is frozen,
/// AG-P1). The structured [`apply_planned`](PlanThenApply::apply_planned) is the primary entry the
/// dispatch tier calls; this bridge is the frozen-shape entry the external MCP / a workflow activity
/// use.
pub struct EffectApiBridge<'a, S, C, D, T, A, B>(
    core::cell::RefCell<PlanThenApply<'a, S, C, D, T, A, B>>,
    Option<std::sync::Arc<RunTokenAuthorizer>>,
)
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget;

impl<'a, S, C, D, T, A, B> EffectApiBridge<'a, S, C, D, T, A, B>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
{
    /// Wrap a [`PlanThenApply`] pipeline as the frozen-shape [`EffectApi`] entry.
    pub fn new(pipeline: PlanThenApply<'a, S, C, D, T, A, B>) -> Self {
        EffectApiBridge(core::cell::RefCell::new(pipeline), None)
    }

    /// Wrap a pipeline for an external router, requiring a final-boundary signed run-token check.
    pub fn with_run_token_authorizer(
        pipeline: PlanThenApply<'a, S, C, D, T, A, B>,
        authorizer: std::sync::Arc<RunTokenAuthorizer>,
    ) -> Self {
        EffectApiBridge(core::cell::RefCell::new(pipeline), Some(authorizer))
    }
}

impl<S, C, D, T, A, B> EffectApi for EffectApiBridge<'_, S, C, D, T, A, B>
where
    S: ToolSurface,
    C: CapabilityCheck,
    D: DelegationLookup,
    T: TenantGuard,
    A: SubsystemApply,
    B: EffectBudget,
{
    fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Denied(
            "external plan-then-apply requires the signed run-token authority entry — direct apply denied"
                .into(),
        )
    }

    fn apply_authorized(
        &self,
        _run: &RunCtx,
        authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        match decode_proposed(&effect) {
            Ok(plan) => {
                if authority.tool != plan.tool.0 {
                    return EffectResult::Denied(format!(
                        "run-token authority is bound to `{}`, not proposed tool `{}`",
                        authority.tool, plan.tool.0
                    ));
                }
                let Some(authorizer) = &self.1 else {
                    return EffectResult::Denied(
                        "plan-then-apply bridge has no final-boundary run-token authorizer — denied"
                            .into(),
                    );
                };
                let pipeline = self.0.borrow();
                if authority.principal_id != pipeline.agent.principal_id {
                    return EffectResult::Denied(
                        "run-token authority principal does not match the plan-then-apply principal"
                            .into(),
                    );
                }
                let Some(def) = pipeline.catalogue.resolve(&plan.tool) else {
                    return EffectResult::Denied(format!("unknown tool {}", plan.tool.0));
                };
                let scope = TenantScope::from_verified_token(
                    &pipeline.agent,
                    Region(pipeline.agent.region.0.clone()),
                );
                if let Err(reason) = authorizer.authorize(
                    &scope,
                    &pipeline.agent.principal_id,
                    &authority.run_token,
                    &def.required_caps,
                ) {
                    return EffectResult::Denied(reason);
                }
                drop(pipeline);
                self.0.borrow_mut().apply_planned(&plan)
            }
            Err(reason) => {
                // Fail-closed: a carrier we cannot parse is Denied (it never mutates). Count it.
                let mut p = self.0.borrow_mut();
                p.signals.denied = p.signals.denied.saturating_add(1);
                EffectResult::Denied(format!("malformed proposed effect: {reason}"))
            }
        }
    }
}

// ───────────────────────── step 1 — the JSON-schema validator (the SCHEMA gate) ──────────────────

/// **Validate `input_json` against the [`ToolDef`] `input_schema` (step 1, the SCHEMA gate).** A
/// real, bounded JSON-Schema check (NOT a stub — the gate must FORCE the failure, EI-01 §3): the
/// input must parse as JSON, and — when the schema declares an object with `required` fields and/or
/// typed `properties` — every required field must be present and every present typed field must
/// match its declared JSON type. A malformed input or a missing/mistyped required field is
/// `Err(reason)` (Denied). An empty/`{}` schema admits any valid JSON (no constraints to fail).
///
/// **Floor (named):** this is a bounded subset of JSON Schema (`type`, `required`, `properties` for
/// object inputs) — sufficient for the frozen `ToolDef` schemas; the full JSON-Schema vocabulary
/// (nested `$ref`, `oneOf`, formats) is not needed by the M2 tool catalogue and is deferred to a
/// later prompt if a tool ever needs it. The gate it provides is REAL: it forces the AG-D-schema
/// failures (a wrong-typed / missing-required input is denied at step 1, not silently applied).
pub fn validate_schema(input_schema: &str, input_json: &str) -> Result<(), String> {
    let input: serde_json::Value =
        serde_json::from_str(input_json).map_err(|e| format!("input is not valid JSON: {e}"))?;
    let schema: serde_json::Value = serde_json::from_str(input_schema)
        .map_err(|e| format!("tool input_schema is not valid JSON: {e}"))?;

    // An empty / non-object schema admits any valid JSON (no constraints).
    let Some(schema_obj) = schema.as_object() else {
        return Ok(());
    };

    // If the schema declares `type: object`, the input MUST be a JSON object.
    if schema_obj.get("type").and_then(|t| t.as_str()) == Some("object") && !input.is_object() {
        return Err("schema requires an object input".into());
    }

    // `required`: every named field must be present in the input object.
    if let Some(required) = schema_obj.get("required").and_then(|r| r.as_array()) {
        let input_obj = input
            .as_object()
            .ok_or_else(|| "schema has `required` but input is not an object".to_string())?;
        for req in required {
            if let Some(name) = req.as_str() {
                if !input_obj.contains_key(name) {
                    return Err(format!("required field `{name}` is missing"));
                }
            }
        }
    }

    // `properties`: every PRESENT field that declares a `type` must match it.
    if let Some(props) = schema_obj.get("properties").and_then(|p| p.as_object()) {
        if let Some(input_obj) = input.as_object() {
            for (name, prop_schema) in props {
                let Some(value) = input_obj.get(name) else {
                    continue; // absent optional fields are fine (required is checked above).
                };
                if let Some(want) = prop_schema.get("type").and_then(|t| t.as_str()) {
                    if !json_type_matches(want, value) {
                        return Err(format!(
                            "field `{name}` must be of type `{want}`, got `{}`",
                            json_type_name(value)
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

/// **THE UNTRUSTED-ARGUMENTS ENFORCEMENT SEAM — validate a [`ToolCall`]'s model-chosen `arguments`
/// against the tool's [`ToolDef::input_schema`] BEFORE the call is dispatched to any tool.**
///
/// `ToolCall::arguments` is **untrusted model output** (the brain widened seam, §2.1). A real
/// tool-calling loop MUST route every `ToolCall` through this checkpoint before it dispatches the
/// call — for EVERY effect kind (a `read`/`compute`/`external` tool that goes to the hands, as well
/// as a `mutate` tool that goes to [`PlanThenApply`]). It reuses the SAME bounded JSON-Schema
/// semantics as the plan-then-apply step-1 [`validate_schema`] gate, so a `mutate` call validated
/// here and re-validated at apply gets one consistent verdict (defence in depth, never a weaker
/// check at the earlier boundary).
///
/// **TODO (the tool-calling loop slice):** the platform loop that turns a
/// [`StepOutcome::UseTools`](myelin_agent::StepOutcome::UseTools) into real tool invocations does not
/// exist yet (VISION §3 — the mock fabricates results, the dry-run maps names to fixture effects, so
/// no `ToolCall::arguments` are dispatched to a live tool today). That loop MUST call this function
/// on each `ToolCall` and refuse to dispatch on `Err` — this is the seam it fills, so the untrusted
/// arguments are NEVER handed to a tool unvalidated.
pub fn validate_tool_arguments(def: &ToolDef, arguments: &serde_json::Value) -> Result<(), String> {
    // Serialise the arguments to the string form the bounded JSON-Schema validator reads, then apply
    // the exact same check the plan-then-apply SCHEMA gate applies to `input_json` (one ACL meaning).
    let input_json = serde_json::to_string(arguments)
        .map_err(|e| format!("tool arguments are not serialisable JSON: {e}"))?;
    validate_schema(&def.input_schema, &input_json)
}

/// Convenience over [`validate_tool_arguments`]: resolve the [`ToolCall`]'s tool in the `catalogue`
/// and validate its `arguments`. An unregistered tool is `Err` (a call the run may not make must not
/// be dispatched). The future tool-calling loop calls this at the dispatch boundary.
pub fn validate_call<S: ToolSurface + ?Sized>(catalogue: &S, call: &ToolCall) -> Result<(), String> {
    let def = catalogue
        .resolve(&call.name)
        .ok_or_else(|| format!("tool `{}` is not registered in the catalogue", call.name.0))?;
    validate_tool_arguments(def, &call.arguments)
}

/// Whether a JSON value matches a JSON-Schema primitive `type` name (the bounded type set).
fn json_type_matches(want: &str, value: &serde_json::Value) -> bool {
    match want {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "null" => value.is_null(),
        // An unknown declared type is conservatively NOT validated (admit — the gate covers the
        // known type set; an unknown type is a schema-authoring concern, not an input failure).
        _ => true,
    }
}

/// The JSON-type name of a value (for the loud denial message).
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

// ───────────────────────── the opaque-carrier decode (the §5.0 routing entry) ────────────────────

/// **Parse the opaque glue [`ProposedEffect`] carrier back into a structured [`PlannedEffect`].**
/// The inverse of [`encode_proposed`]; a carrier that does not match the field-ordered shape is
/// `Err(reason)` (Denied — fail-closed). Deterministic.
pub fn decode_proposed(effect: &ProposedEffect) -> Result<PlannedEffect, String> {
    let mut tool = None;
    let mut object = None;
    let mut field = None;
    let mut transition = None;
    let mut unit: Option<&'static str> = None;
    let mut wholesale = None;
    let mut markup = None;
    let mut input = None;

    for part in effect.0.split('\u{1f}') {
        let Some((key, val)) = part.split_once('=') else {
            return Err(format!("malformed segment (no `=`): {part:?}"));
        };
        match key {
            "tool" => tool = Some(ToolName(val.to_string())),
            "object" => object = Some(ArtifactRef(val.to_string())),
            "field" => {
                field = if val.is_empty() {
                    None
                } else {
                    Some(FieldId(val.to_string()))
                }
            }
            "transition" => {
                transition = if val.is_empty() {
                    None
                } else {
                    Some(TransitionId(val.to_string()))
                }
            }
            // The unit must be one of the frozen metered-unit dimensions (a `&'static str`). We
            // intern the known dimensions; an unknown unit is denied (a cost dimension is frozen).
            "unit" => unit = Some(intern_unit(val)?),
            "wholesale" => {
                wholesale = Some(
                    val.parse::<u64>()
                        .map_err(|e| format!("bad wholesale: {e}"))?,
                )
            }
            "markup" => markup = Some(val.parse::<u64>().map_err(|e| format!("bad markup: {e}"))?),
            "input" => input = Some(val.to_string()),
            other => return Err(format!("unknown segment key: {other}")),
        }
    }

    Ok(PlannedEffect {
        tool: tool.ok_or("missing tool")?,
        object: object.ok_or("missing object")?,
        input_json: input.ok_or("missing input")?,
        field,
        transition,
        cost: EffectCost {
            unit: unit.ok_or("missing unit")?,
            wholesale: wholesale.ok_or("missing wholesale")?,
            markup: markup.ok_or("missing markup")?,
        },
    })
}

/// The frozen metered-unit dimensions an effect may bill (the cost dimension is a `&'static str`
/// label, frozen — a new dimension is added here, never invented at runtime). An unknown unit is
/// denied (fail-closed — a cost is never billed against an unrecognised dimension).
fn intern_unit(unit: &str) -> Result<&'static str, String> {
    match unit {
        "agent.effect" => Ok("agent.effect"),
        "issue.transition" => Ok("issue.transition"),
        "git.merge" => Ok("git.merge"),
        "external.call" => Ok("external.call"),
        other => Err(format!("unknown metered-unit dimension: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    // ───────── deterministic REAL impls of every consumed seam (the CDC provider shape) ─────────

    /// A `ToolSurface` over a fixed catalogue (the §4.2 registry, in-memory).
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

    /// A `check` provider that allows a fixed cap set, else Deny — and returns Conditional when a
    /// transition caveat is present but no approver attr (the OQ-E field/transition ABAC leg).
    struct Checker {
        allow: BTreeSet<String>,
        /// caps that resolve to Conditional iff a transition caveat is present (SLA-bound deny).
        conditional_on_transition: BTreeSet<String>,
    }
    impl CapabilityCheck for Checker {
        fn check(
            &self,
            _subject: &Principal,
            permission: &Permission,
            _object: &ArtifactRef,
            _at: &Consistency,
            caveat: Option<&CaveatContext>,
        ) -> Decision {
            if self.conditional_on_transition.contains(&permission.0)
                && caveat.map(|c| c.transition.is_some()).unwrap_or(false)
            {
                // an SLA-bound transition with no approver context → Conditional (treated as deny).
                return Decision::Conditional;
            }
            if self.allow.contains(&permission.0) {
                Decision::Allow
            } else {
                Decision::Deny
            }
        }
    }

    /// A `delegation` provider returning a fixed effective-policy cap set (the ∩ result).
    struct Delegator {
        policy: Vec<String>,
    }
    impl DelegationLookup for Delegator {
        fn delegation(&self, _agent: &Principal, _trigger: &Principal) -> EffectivePolicy {
            EffectivePolicy {
                caveats: self.policy.clone(),
            }
        }
    }

    /// A tenant guard that forbids a fixed deny-list of tools.
    struct Tenant {
        forbid: BTreeSet<String>,
    }
    impl TenantGuard for Tenant {
        fn permits(&self, _agent: &Principal, tool: &ToolName, _object: &ArtifactRef) -> bool {
            !self.forbid.contains(&tool.0)
        }
    }

    /// A subsystem PUBLIC endpoint that records the apply + returns an event id (the only mutation
    /// path). A `fail` flag makes the endpoint error (step-7 loud failure).
    struct Endpoint {
        fail: bool,
        applied: std::cell::RefCell<Vec<(String, String)>>,
    }
    impl SubsystemApply for Endpoint {
        fn apply_public(
            &self,
            _agent: &Principal,
            tool: &ToolName,
            object: &ArtifactRef,
            _input: &str,
        ) -> Result<EventId, ApplyError> {
            if self.fail {
                return Err(ApplyError("endpoint unavailable".into()));
            }
            self.applied
                .borrow_mut()
                .push((tool.0.clone(), object.0.clone()));
            Ok(EventId(format!("evt:{}:{}", tool.0, object.0)))
        }
    }

    /// A reserve/settle budget over a remaining balance — the BUDGET step reads it, the METER step
    /// settles (debits). A REAL minor-units ledger (integer, never floats).
    struct Budget {
        remaining: u64,
        billed: u64,
        settles: u64,
    }
    impl EffectBudget for Budget {
        fn has_remaining(&self, cost: u64) -> bool {
            self.remaining >= cost
        }
        fn settle_one(&mut self, unit: &MeteredUnit) -> u64 {
            let total = unit.total().map(|m| m.0).unwrap_or(0);
            self.remaining = self.remaining.saturating_sub(total);
            self.billed = self.billed.saturating_add(total);
            self.settles += 1;
            total
        }
    }

    fn agent() -> Principal {
        Principal::stub(
            PrincipalId("psn:agent-7".into()),
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("mock".into()),
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

    fn tool_def(name: &str, caps: &[&str], requires_approval: bool, kind: EffectKind) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "issues".into(),
            version: 1,
            input_schema:
                r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"}}}"#
                    .into(),
            required_caps: caps.iter().map(|c| c.to_string()).collect(),
            effect_kind: kind,
            side_effecting: true,
            requires_approval,
            exposed_over_mcp: false,
        }
    }

    fn plan(tool: &str, input: &str) -> PlannedEffect {
        PlannedEffect {
            tool: ToolName(tool.into()),
            object: ArtifactRef("myelin://acme/issues/i-1".into()),
            input_json: input.into(),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "issue.transition",
                wholesale: 3,
                markup: 1,
            },
        }
    }

    /// A pipeline wired with the supplied seams (an allowed run by default).
    #[allow(clippy::too_many_arguments)]
    fn pipeline<'a>(
        catalogue: &'a Catalogue,
        check: &'a Checker,
        delegation: &'a Delegator,
        tenant: &'a Tenant,
        endpoint: &'a Endpoint,
        budget: &'a mut Budget,
        approved: BTreeSet<String>,
        signals: &'a mut PipelineSignals,
    ) -> PlanThenApply<'a, Catalogue, Checker, Delegator, Tenant, Endpoint, Budget> {
        PlanThenApply {
            catalogue,
            check,
            delegation,
            tenant,
            apply_endpoint: endpoint,
            budget,
            agent: agent(),
            trigger_actor: human(),
            zookie: Zookie("z-1".into()),
            approved,
            signals,
        }
    }

    fn allow_caps(caps: &[&str]) -> Checker {
        Checker {
            allow: caps.iter().map(|c| c.to_string()).collect(),
            conditional_on_transition: BTreeSet::new(),
        }
    }

    // ───────────────────────── the eight-step pipeline — happy path ─────────────────────────

    /// **The full pipeline APPLIES an allowed effect: schema ✓ → cap ✓ → delegation ✓ → tenant ✓ →
    /// budget ✓ → no gate → APPLY → METER.** The subsystem endpoint recorded the mutation; the
    /// budget settled exactly one cost event; the signals counted one applied + the bill.
    #[test]
    fn pipeline_applies_an_allowed_effect_and_meters_it() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.create", r#"{"title":"bug"}"#));
        assert!(
            matches!(out, EffectResult::Applied(EventId(ref id)) if id == "evt:issue.create:myelin://acme/issues/i-1")
        );

        // the mutation went through the subsystem's PUBLIC endpoint (the ONLY mutation path).
        assert_eq!(
            endpoint.applied.borrow().len(),
            1,
            "exactly one apply via the public endpoint"
        );
        // the meter settled exactly one cost event (wholesale 3 + markup 1 = 4).
        assert_eq!(
            budget.settles, 1,
            "exactly one cost event settled (the METER step)"
        );
        assert_eq!(budget.billed, 4, "billed wholesale 3 + markup 1");
        assert_eq!(budget.remaining, 96, "the reserve debited the bill");
        assert_eq!(signals.applied(), 1);
        assert_eq!(signals.metered_total(), 4);
        assert_eq!(signals.denied(), 0);
        assert_eq!(
            signals.privileged_fallback(),
            0,
            "AG-D2: NO privileged fallback EVER fires"
        );
    }

    // ───────────────────────── step-by-step DENY legs (each step forces its failure) ─────────

    /// **Step 1 (SCHEMA) — a malformed input (missing the required `title`) is Denied; 0 mutation.**
    #[test]
    fn step1_schema_reject_denies_before_any_mutation() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        // input missing the required `title` field → Denied at step 1.
        let out = p.apply_planned(&plan("issue.create", r#"{"body":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("title")),
            "{out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "schema reject → 0 mutation"
        );
        assert_eq!(budget.settles, 0, "a denied effect is never metered");
        assert_eq!(
            signals.denied(),
            1,
            "the denial counter incremented (AG-D2)"
        );
        assert_eq!(signals.privileged_fallback(), 0);
    }

    /// **Step 2 (CAPABILITY) — `check` denies the required cap → Denied; 0 mutation. AND the OQ-E
    /// transition ABAC: an SLA-bound transition with no approver context → Conditional → Denied.**
    #[test]
    fn step2_capability_deny_and_caveat_conditional_deny() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.transition",
                &["issue.transition"],
                false,
                EffectKind::Mutate,
            )],
        };
        // the cap is NOT allowed → Deny at step 2.
        let check = allow_caps(&[]);
        let del = Delegator {
            policy: vec!["issue.transition".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        {
            let mut p = pipeline(
                &cat,
                &check,
                &del,
                &tenant,
                &endpoint,
                &mut budget,
                BTreeSet::new(),
                &mut signals,
            );
            let out = p.apply_planned(&plan("issue.transition", r#"{"title":"x"}"#));
            assert!(
                matches!(out, EffectResult::Denied(ref r) if r.contains("capability")),
                "{out:?}"
            );
        }
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "capability deny → 0 mutation"
        );

        // the OQ-E leg: the cap is allowed in general, but an SLA-bound transition caveat → Conditional → deny.
        let check2 = Checker {
            allow: ["issue.transition".to_string()].into_iter().collect(),
            conditional_on_transition: ["issue.transition".to_string()].into_iter().collect(),
        };
        let mut budget2 = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals2 = PipelineSignals::new();
        let mut p2 = pipeline(
            &cat,
            &check2,
            &del,
            &tenant,
            &endpoint,
            &mut budget2,
            BTreeSet::new(),
            &mut signals2,
        );
        let mut plan_t = plan("issue.transition", r#"{"title":"x"}"#);
        plan_t.transition = Some(TransitionId("to_done".into()));
        let out = p2.apply_planned(&plan_t);
        assert!(
            matches!(out, EffectResult::Denied(_)),
            "Conditional (caveat unmet) is a DENY, never a silent allow: {out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "the SLA-bound transition did NOT mutate"
        );
    }

    /// **Step 3 (DELEGATION) — a cap the agent's `check` allows but the delegation ∩ FORBIDS is
    /// confined to the intersection → Denied (over-privilege blocked; AG-D3).**
    #[test]
    fn step3_delegation_intersection_confines_over_privilege() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.delete",
                &["issue.delete"],
                false,
                EffectKind::Mutate,
            )],
        };
        // check ALLOWS issue.delete (the agent.policy term), but the delegation ∩ does NOT grant it.
        let check = allow_caps(&["issue.delete"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        }; // delegation lacks issue.delete.
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.delete", r#"{"title":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("intersection")),
            "{out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "over-privilege confined → 0 mutation (AG-D3)"
        );
        assert_eq!(signals.denied(), 1);
    }

    /// **Step 4 (TENANT) — the tenant guardrails forbid the tool → Denied; 0 mutation.**
    #[test]
    fn step4_tenant_guard_denies() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: ["issue.create".to_string()].into_iter().collect(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.create", r#"{"title":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("tenant")),
            "{out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "tenant deny → 0 mutation"
        );
    }

    /// **Step 5 (BUDGET) — an exhausted reserve refuses the effect → Denied; 0 mutation; NO
    /// privileged fallback (AG-5/AG-D2).**
    #[test]
    fn step5_budget_refusal_denies_with_no_fallback() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 1,
            billed: 0,
            settles: 0,
        }; // < cost (4).
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.create", r#"{"title":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("balance")),
            "{out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "no balance → no mutation"
        );
        assert_eq!(budget.settles, 0, "a budget-denied effect is never metered");
        assert_eq!(
            signals.privileged_fallback(),
            0,
            "AG-D2: 0 privileged fallback"
        );
    }

    #[test]
    fn step5_unrepresentable_cost_denies_before_mutation_or_metering() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: u64::MAX,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut effect = plan("issue.create", r#"{"title":"x"}"#);
        effect.cost = EffectCost {
            unit: "issue.create",
            wholesale: u64::MAX,
            markup: 1,
        };
        let mut pipeline = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let outcome = pipeline.apply_planned(&effect);
        assert!(
            matches!(outcome, EffectResult::Denied(ref reason) if reason.contains("minor-unit range")),
            "{outcome:?}"
        );
        assert!(endpoint.applied.borrow().is_empty(), "overflowing cost must not mutate");
        assert_eq!(budget.settles, 0, "overflowing cost must not reach settlement");
        assert_eq!(budget.billed, 0);
    }

    /// **Step 6 (HITL GATE) — a `requires_approval` tool not yet approved → Gated; the tool does NOT
    /// mutate (AG-8). Once approved → it Applies.**
    #[test]
    fn step6_hitl_gate_withholds_then_resumes() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "git.merge",
                &["git.merge"],
                true,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["git.merge"]);
        let del = Delegator {
            policy: vec!["git.merge".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };

        // not yet approved → Gated (no mutation).
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        {
            let mut p = pipeline(
                &cat,
                &check,
                &del,
                &tenant,
                &endpoint,
                &mut budget,
                BTreeSet::new(),
                &mut signals,
            );
            let out = p.apply_planned(&plan("git.merge", r#"{"title":"x"}"#));
            assert!(
                matches!(out, EffectResult::Gated(GateId(ref g)) if g.starts_with("gate:git.merge")),
                "{out:?}"
            );
        }
        assert_eq!(
            endpoint.applied.borrow().len(),
            0,
            "a gated effect does NOT mutate (AG-8)"
        );
        assert_eq!(
            budget.settles, 0,
            "a gated effect is never metered (it didn't apply)"
        );
        assert_eq!(signals.gated(), 1);

        // approved (the HITL resume, AG-P9, adds THIS effect's per-(tool, object) gate key to
        // `approved` — R2.4: never the bare tool name) → Applies.
        let mut budget2 = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals2 = PipelineSignals::new();
        let the_plan = plan("git.merge", r#"{"title":"x"}"#);
        let approved: BTreeSet<String> =
            [effect_gate_key(&the_plan.tool, &the_plan.object)].into_iter().collect();
        let mut p2 = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget2,
            approved,
            &mut signals2,
        );
        let out = p2.apply_planned(&the_plan);
        assert!(
            matches!(out, EffectResult::Applied(_)),
            "an approved gated effect Applies: {out:?}"
        );
        assert_eq!(
            endpoint.applied.borrow().len(),
            1,
            "the approved effect mutated once"
        );
    }

    /// **Step 6 is PER-EFFECT keyed (R2.4 Defect B):** an approval for `git.merge` on object A
    /// admits ONLY that effect — the same tool on object B still gates, and (regression pin) a
    /// bare TOOL NAME in the `approved` set admits NOTHING.
    #[test]
    fn step6_approval_is_per_effect_never_per_tool_name() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "git.merge",
                &["git.merge"],
                true,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["git.merge"]);
        let del = Delegator {
            policy: vec!["git.merge".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };

        let mut plan_a = plan("git.merge", r#"{"title":"a"}"#);
        plan_a.object = ArtifactRef("myelin://acme/git/pr/40".into());
        let mut plan_b = plan("git.merge", r#"{"title":"b"}"#);
        plan_b.object = ArtifactRef("myelin://acme/git/pr/41".into());

        // approved: exactly effect A's key (pr 40). Effect B (pr 41, SAME tool) must still gate.
        let approved: BTreeSet<String> =
            [effect_gate_key(&plan_a.tool, &plan_a.object)].into_iter().collect();
        let mut budget = Budget { remaining: 100, billed: 0, settles: 0 };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat, &check, &del, &tenant, &endpoint, &mut budget, approved, &mut signals,
        );
        assert!(
            matches!(p.apply_planned(&plan_a), EffectResult::Applied(_)),
            "the approved effect (pr 40) applies"
        );
        assert!(
            matches!(p.apply_planned(&plan_b), EffectResult::Gated(_)),
            "the sibling (pr 41) sharing the tool name still GATES — approval never transfers"
        );
        assert_eq!(endpoint.applied.borrow().len(), 1, "exactly the approved effect mutated");

        // Regression pin: a bare tool name in `approved` is NOT an approval key — it admits nothing.
        let by_name: BTreeSet<String> = ["git.merge".to_string()].into_iter().collect();
        let mut budget2 = Budget { remaining: 100, billed: 0, settles: 0 };
        let mut signals2 = PipelineSignals::new();
        let mut p2 = pipeline(
            &cat, &check, &del, &tenant, &endpoint, &mut budget2, by_name, &mut signals2,
        );
        assert!(
            matches!(p2.apply_planned(&plan_b), EffectResult::Gated(_)),
            "a bare tool name in the approved set clears NO gate (the old bypass shape)"
        );
    }

    /// **Step 7 (APPLY) — a subsystem endpoint failure is surfaced LOUD as Denied; the effect is NOT
    /// metered (a failed apply refunds the reserve; the meter settles only an applied effect).**
    #[test]
    fn step7_apply_failure_is_loud_and_unmetered() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: true,
            applied: RefCell::new(vec![]),
        }; // the endpoint errors.
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let out = p.apply_planned(&plan("issue.create", r#"{"title":"x"}"#));
        assert!(
            matches!(out, EffectResult::Denied(ref r) if r.contains("apply failed")),
            "{out:?}"
        );
        assert_eq!(budget.settles, 0, "a failed apply is NOT metered");
        assert_eq!(signals.applied(), 0);
        assert_eq!(signals.denied(), 1);
    }

    // ───────────────────────── chained-e2e: an allowed + a disallowed effect in one session ──────

    /// **The chained-e2e (TESTS field): a mock run chains an ALLOWED effect (Applied) and a
    /// DISALLOWED effect (Denied) through `apply` in one session, with shared budget + signals.**
    #[test]
    fn chained_e2e_allowed_then_disallowed_in_one_session() {
        let cat = Catalogue {
            defs: vec![
                tool_def("issue.create", &["issue.write"], false, EffectKind::Mutate),
                tool_def("issue.delete", &["issue.delete"], false, EffectKind::Mutate),
            ],
        };
        let check = allow_caps(&["issue.write", "issue.delete"]);
        // delegation grants write but NOT delete → the delete is confined out of the intersection.
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        // effect 1: allowed → Applied + metered.
        let a = p.apply_planned(&plan("issue.create", r#"{"title":"new"}"#));
        assert!(
            matches!(a, EffectResult::Applied(_)),
            "the allowed effect applies: {a:?}"
        );
        // effect 2: disallowed (over-privilege) → Denied, in the SAME session.
        let d = p.apply_planned(&plan("issue.delete", r#"{"title":"x"}"#));
        assert!(
            matches!(d, EffectResult::Denied(_)),
            "the disallowed effect is denied: {d:?}"
        );

        assert_eq!(signals.applied(), 1, "exactly one applied");
        assert_eq!(signals.denied(), 1, "exactly one denied");
        assert_eq!(
            endpoint.applied.borrow().len(),
            1,
            "exactly one mutation reached a subsystem"
        );
        assert_eq!(
            signals.privileged_fallback(),
            0,
            "AG-D2: 0 privileged fallback across the session"
        );
    }

    // ───────────────────────── the glue EffectApi (8.2) bridge via the opaque carrier ───────────

    /// **The unbound 8.2 `EffectApi::apply` entry cannot bypass run-token authority.** Structured
    /// internal dispatch uses `apply_planned`; external routing must use `apply_authorized` with a
    /// configured final-boundary verifier.
    #[test]
    fn glue_effect_api_bridge_denies_the_unbound_entry() {
        let cat = Catalogue {
            defs: vec![tool_def(
                "issue.create",
                &["issue.write"],
                false,
                EffectKind::Mutate,
            )],
        };
        let check = allow_caps(&["issue.write"]);
        let del = Delegator {
            policy: vec!["issue.write".into()],
        };
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );
        let bridge = EffectApiBridge::new(p);

        let carrier = encode_proposed(&plan("issue.create", r#"{"title":"x"}"#));
        let out = bridge.apply(&RunCtx::default(), carrier);
        assert!(
            matches!(out, EffectResult::Denied(ref reason) if reason.contains("signed run-token")),
            "the unbound bridge must deny: {out:?}"
        );
        assert!(endpoint.applied.borrow().is_empty());

        // Even a malformed direct carrier cannot select a weaker parsing path.
        let bad = bridge.apply(
            &RunCtx::default(),
            ProposedEffect("garbage-no-fields".into()),
        );
        assert!(
            matches!(bad, EffectResult::Denied(ref r) if r.contains("signed run-token")),
            "{bad:?}"
        );
    }

    /// **`encode_proposed`/`decode_proposed` round-trip is exact + deterministic (the AG-P8
    /// proposed-effect-sequence determinism support).** A plan encodes to a byte-stable carrier and
    /// decodes back to the identical plan.
    #[test]
    fn proposed_effect_carrier_round_trips_deterministically() {
        let mut original = plan("issue.transition", r#"{"title":"close it"}"#);
        original.field = Some(FieldId("status".into()));
        original.transition = Some(TransitionId("to_done".into()));
        let c1 = encode_proposed(&original);
        let c2 = encode_proposed(&original);
        assert_eq!(
            c1, c2,
            "the encoding is deterministic (byte-identical across calls)"
        );
        let back = decode_proposed(&c1).expect("round-trips");
        assert_eq!(back, original, "decode is the exact inverse of encode");
    }

    // ───────────────────────── step 1 schema validator — direct unit tests ───────────────────────

    /// **`validate_schema` forces its failures (EI-01 §3 — the gate is REAL, not a stub).** A
    /// missing required field, a mistyped field, and a non-object input are each rejected; a valid
    /// input + an empty schema pass.
    #[test]
    fn schema_validator_forces_each_failure() {
        let schema = r#"{"type":"object","required":["title"],"properties":{"title":{"type":"string"},"count":{"type":"integer"}}}"#;
        assert!(
            validate_schema(schema, r#"{"title":"ok"}"#).is_ok(),
            "a valid input passes"
        );
        assert!(
            validate_schema(schema, r#"{"title":"ok","count":3}"#).is_ok(),
            "a typed optional field passes"
        );
        assert!(
            validate_schema(schema, r#"{"count":3}"#).is_err(),
            "a missing required field is rejected"
        );
        assert!(
            validate_schema(schema, r#"{"title":5}"#).is_err(),
            "a mistyped required field is rejected"
        );
        assert!(
            validate_schema(schema, r#"{"title":"ok","count":"x"}"#).is_err(),
            "a mistyped optional field is rejected"
        );
        assert!(
            validate_schema(schema, r#"[1,2,3]"#).is_err(),
            "a non-object input under a type:object schema is rejected"
        );
        assert!(
            validate_schema(schema, r#"not json"#).is_err(),
            "a non-JSON input is rejected"
        );
        // an empty schema admits any valid JSON.
        assert!(
            validate_schema("{}", r#"{"anything":true}"#).is_ok(),
            "an empty schema admits any valid JSON"
        );
    }

    /// **`EffectCost::total` is saturating + exact (mutation-floor — a cost never silently wraps).**
    #[test]
    fn effect_cost_total_is_saturating_and_exact() {
        assert_eq!(
            EffectCost {
                unit: "x",
                wholesale: 3,
                markup: 1
            }
            .total(),
            4
        );
        assert_eq!(
            EffectCost {
                unit: "x",
                wholesale: u64::MAX,
                markup: 1
            }
            .total(),
            u64::MAX,
            "saturates, never wraps"
        );
        assert_eq!(
            EffectCost {
                unit: "x",
                wholesale: 0,
                markup: 0
            }
            .total(),
            0
        );
    }

    /// **The `PipelineSignals` accessors are exact (mutation-floor — the signals ARE the AG-D2 green
    /// artifacts; a `-> 0`/`-> 1` constant mutant must flip an assertion).**
    #[test]
    fn pipeline_signals_accessors_are_exact() {
        let mut s = PipelineSignals::new();
        assert_eq!(s.applied(), 0);
        assert_eq!(s.denied(), 0);
        assert_eq!(s.gated(), 0);
        assert_eq!(s.privileged_fallback(), 0);
        assert_eq!(s.metered_total(), 0);
        s.applied = 2;
        s.denied = 3;
        s.gated = 4;
        s.metered_total = 11;
        assert_eq!(s.applied(), 2, "applied returns its field (kills -> 0/1)");
        assert_eq!(s.denied(), 3, "denied returns its field");
        assert_eq!(s.gated(), 4, "gated returns its field");
        assert_eq!(s.metered_total(), 11, "metered_total returns its field");
        assert_eq!(
            s.privileged_fallback(),
            0,
            "privileged_fallback is ALWAYS 0 (no fallback path)"
        );
    }

    /// **`intern_unit` rejects an unknown metered-unit dimension (fail-closed — a cost is never
    /// billed against an unrecognised dimension).**
    #[test]
    fn intern_unit_rejects_unknown_dimension() {
        assert_eq!(intern_unit("issue.transition").unwrap(), "issue.transition");
        assert!(
            intern_unit("made.up.unit").is_err(),
            "an unknown dimension is rejected"
        );
    }

    /// **A bare `type:object` schema (no `required`/`properties`) rejects a non-object input at the
    /// line-604 check ALONE (kills the `== → !=` mutant on the `type:object` test).** With no
    /// `required` array, an array input must be rejected ONLY by the type check — so flipping that
    /// `==` to `!=` (which would then reject the valid object and admit the array) flips both asserts.
    #[test]
    fn bare_type_object_schema_rejects_non_object_input() {
        let schema = r#"{"type":"object"}"#;
        assert!(
            validate_schema(schema, r#"{"any":1}"#).is_ok(),
            "a type:object schema admits an object"
        );
        assert!(
            validate_schema(schema, r#"[1,2,3]"#).is_err(),
            "a type:object schema rejects an array (line-604 check)"
        );
        // a NON-object schema (no type:object) admits anything — proves the `Some(\"object\")` arm is exact.
        let no_type = r#"{"description":"free"}"#;
        assert!(
            validate_schema(no_type, r#"[1,2,3]"#).is_ok(),
            "a schema without type:object admits an array"
        );
    }

    /// **The untrusted-arguments enforcement seam validates a `ToolCall`'s arguments against the
    /// tool's `ToolDef.input_schema` — the SAME verdict the step-1 SCHEMA gate reaches.** Well-formed
    /// arguments pass; a missing required field / a mistyped field / a non-object is `Err` (so the
    /// future loop refuses to dispatch). An unregistered tool is `Err` via [`validate_call`].
    #[test]
    fn validate_tool_arguments_enforces_the_schema_before_dispatch() {
        use myelin_agent::{ToolCall, ToolCallId};
        use serde_json::json;

        let def = tool_def("create_issue", &["issue.write"], false, EffectKind::Mutate);

        // Well-formed model output (the required `title` string is present) passes.
        assert!(validate_tool_arguments(&def, &json!({"title": "CI is red"})).is_ok());
        // A missing required field is rejected (untrusted output must not be dispatched).
        assert!(validate_tool_arguments(&def, &json!({})).is_err());
        // A mistyped field is rejected.
        assert!(validate_tool_arguments(&def, &json!({"title": 7})).is_err());
        // A non-object (a `type:object` schema) is rejected — e.g. a `Null` placeholder.
        assert!(validate_tool_arguments(&def, &serde_json::Value::Null).is_err());

        // `validate_call` resolves the tool then validates; an unregistered tool is refused.
        struct Cat {
            defs: Vec<ToolDef>,
        }
        impl ToolSurface for Cat {
            fn register_tool(&mut self, d: ToolDef) {
                self.defs.push(d);
            }
            fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
                self.defs.iter().find(|d| &d.name == name)
            }
        }
        let cat = Cat { defs: vec![def] };
        let good = ToolCall {
            id: ToolCallId("c1".into()),
            name: ToolName("create_issue".into()),
            arguments: json!({"title": "ok"}),
        };
        assert!(validate_call(&cat, &good).is_ok());
        let unknown = ToolCall {
            id: ToolCallId("c2".into()),
            name: ToolName("no_such_tool".into()),
            arguments: json!({}),
        };
        assert!(validate_call(&cat, &unknown).is_err());
    }

    /// **`json_type_matches` is exact across EVERY primitive type (kills the per-arm delete + the
    /// `|| → &&` mutant on `integer`).** Each declared type matches its value and rejects the others.
    #[test]
    fn json_type_matches_is_exact_per_type() {
        use serde_json::json;
        // each type matches its OWN value.
        assert!(json_type_matches("object", &json!({"a":1})));
        assert!(json_type_matches("array", &json!([1, 2])));
        assert!(json_type_matches("string", &json!("s")));
        assert!(json_type_matches("boolean", &json!(true)));
        assert!(json_type_matches("number", &json!(1.5)));
        assert!(json_type_matches("integer", &json!(7)));
        assert!(json_type_matches("null", &json!(null)));
        // and REJECTS a mismatched value (kills the per-arm `delete match arm` → fall-through-to-true).
        assert!(
            !json_type_matches("object", &json!([1])),
            "object arm rejects an array"
        );
        assert!(
            !json_type_matches("array", &json!({"a":1})),
            "array arm rejects an object"
        );
        assert!(
            !json_type_matches("boolean", &json!(1)),
            "boolean arm rejects a number"
        );
        assert!(
            !json_type_matches("number", &json!("x")),
            "number arm rejects a string"
        );
        assert!(
            !json_type_matches("null", &json!(0)),
            "null arm rejects a number"
        );
        // integer: the `|| ` covers BOTH i64 and u64; a float is NOT an integer (kills `|| → &&`).
        assert!(
            json_type_matches("integer", &json!(-3)),
            "a negative i64 is an integer"
        );
        assert!(
            json_type_matches("integer", &json!(u64::MAX)),
            "a large u64 is an integer"
        );
        assert!(
            !json_type_matches("integer", &json!(1.5)),
            "a float is NOT an integer"
        );
        // an unknown declared type conservatively admits (the documented bounded-type-set behaviour).
        assert!(json_type_matches("made-up-type", &json!("anything")));
    }

    /// **`json_type_name` returns the EXACT type name per value (kills the `-> ""`/`-> "xyzzy"`
    /// constant mutants — the loud denial message must name the real type).**
    #[test]
    fn json_type_name_is_exact_per_value() {
        use serde_json::json;
        assert_eq!(json_type_name(&json!(null)), "null");
        assert_eq!(json_type_name(&json!(true)), "boolean");
        assert_eq!(json_type_name(&json!(3)), "number");
        assert_eq!(json_type_name(&json!("s")), "string");
        assert_eq!(json_type_name(&json!([1])), "array");
        assert_eq!(json_type_name(&json!({"a":1})), "object");
    }

    /// **`privileged_fallback` is STRUCTURALLY 0 — there is NO code path that increments it (AG-D2).**
    /// The accessor returns the field; since the field is only ever its `Default` (0) and no method
    /// mutates it, a `-> 0` mutant is *semantically equivalent* (the field IS always 0). We assert
    /// the INVARIANT the drill reads: across an applied + denied + gated + apply-failed session, the
    /// fallback counter never leaves 0 (the property the AG-D2 drill proves — there is no fallback).
    #[test]
    fn privileged_fallback_stays_zero_across_every_outcome() {
        let cat = Catalogue {
            defs: vec![
                tool_def("issue.create", &["issue.write"], false, EffectKind::Mutate),
                tool_def("git.merge", &["git.merge"], true, EffectKind::Mutate),
                tool_def("issue.delete", &["issue.delete"], false, EffectKind::Mutate),
            ],
        };
        let check = allow_caps(&["issue.write", "git.merge", "issue.delete"]);
        let del = Delegator {
            policy: vec!["issue.write".into(), "git.merge".into()],
        }; // no delete.
        let tenant = Tenant {
            forbid: BTreeSet::new(),
        };
        let endpoint = Endpoint {
            fail: false,
            applied: RefCell::new(vec![]),
        };
        let mut budget = Budget {
            remaining: 100,
            billed: 0,
            settles: 0,
        };
        let mut signals = PipelineSignals::new();
        let mut p = pipeline(
            &cat,
            &check,
            &del,
            &tenant,
            &endpoint,
            &mut budget,
            BTreeSet::new(),
            &mut signals,
        );

        let _ = p.apply_planned(&plan("issue.create", r#"{"title":"a"}"#)); // Applied
        let _ = p.apply_planned(&plan("git.merge", r#"{"title":"b"}"#)); // Gated
        let _ = p.apply_planned(&plan("issue.delete", r#"{"title":"c"}"#)); // Denied (∩)
        assert_eq!(signals.applied(), 1);
        assert_eq!(signals.gated(), 1);
        assert_eq!(signals.denied(), 1);
        // THE AG-D2 INVARIANT: across every outcome, the privileged-fallback counter is 0.
        assert_eq!(
            signals.privileged_fallback(),
            0,
            "AG-D2: 0 privileged fallback — there is NO fallback code path"
        );
    }
}
