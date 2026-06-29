//! # `governance` — THE MR-006 BINDING: `mint_run_token → EffectApi::apply`, HITL-gated, audited.
//!
//! This is the load-bearing module of MR-021. The MR-006 shape review is BINDING here: a local Claude
//! over MCP "routes through **mint_run_token → EffectApi**, NOT a bare human PAT". The
//! [`GovernedRouter`] is the realisation of that rule — on every `tools/call` it:
//!
//! 1. **mints a per-run attenuated capability token** ([`RunTokenMinter::mint_run_token`], MR-011-real)
//!    for the calling agent-principal, once per MCP run (the run's life == the session's life), and
//!    the call ACTS UNDER it — NOT a bare human PAT;
//! 2. **consults durable revocation** ([`RunTokenMinter::is_live`]) before acting — a revoked /
//!    expired run token is **Denied**, never routed (MR-011 durable revocation);
//! 3. **HITL-gates** a `requires_approval` tool (the FROZEN flag from git's `agent_tools()`) BEFORE
//!    apply — a gated tool with no approval is **withheld** and **does NOT mutate** (the AG-8 leg);
//! 4. **routes the effect through `EffectApi::apply`** (the platform-owned PLAN-THEN-APPLY chokepoint,
//!    §8.2) under a [`RunCtx`] that carries the run-token `jti` + the principal — NOT a direct
//!    mutation. `EffectApi` is **brain-agnostic** (identical for the mock runtime, a future hosted
//!    LLM, and local Claude over MCP — the whole point of plan-then-apply);
//! 5. **attributes every call** to the run (the jti + the principal + the tool + the outcome) in an
//!    auditable trail — this is what makes "agent governance real from day one".
//!
//! ## What is REAL vs the named boundary (honesty)
//! - **REAL:** the per-run token mint, the revocation consult, the HITL gate on the frozen flag, the
//!   routing through the `EffectApi` trait, and the per-run audit attribution.
//! - **INJECTED:** the concrete `EffectApi` body. The eight-step PlanThenApply pipeline (schema →
//!   capability → delegation → tenant → budget → HITL → apply-via-public-endpoint → meter) lives in
//!   `myelin-agent-service`; the composition root injects it. [`SkeletonEffectApi`] is a reference
//!   chokepoint impl (records the `RunCtx` it received, returns `Applied`) used to prove the ROUTING.
//! - **DEFERRED:** the durable git-backend EFFECT (a real merge/PR write) is the Git track **E1.1**.
//!   The governance pipeline + the audit attribution are what's real here; the durable mutation is not.

use std::cell::RefCell;

use myelin_agent::{EffectApi, EffectResult, ProposedEffect, RunCtx};
use myelin_events::Timestamp;
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, RunId, RunToken,
};
use myelin_identity_service::delegation::DelegationInput;
use myelin_identity_service::machine_auth::MachineKind;
use myelin_identity_service::mint::RunTokenMinter;
use myelin_storage::TenantScope;

use crate::registry::RegisteredTool;

/// The verified run identity an MCP run mints + acts under — the conjuncts `mint_run_token` composes
/// (the agent ceiling ∩ delegation ∩ tenant guardrails ∩ trigger-actor-held) and the run's scope.
/// The composition root supplies this from the authenticated MCP session (tenant-from-token — never
/// a path/arg). `Principal::stub` is NOT used here (it is test-only); production builds these from
/// `authenticate`.
pub struct RunPrincipal {
    /// The verified `(tenant, region)` scope the per-run token is minted under.
    pub scope: TenantScope,
    /// The agent principal id (the run-token subject; the `jti` is bound to `(agent_id, run_id)`).
    pub agent_id: PrincipalId,
    /// The agent principal (its policy ceiling is a conjunct of the mint intersection).
    pub agent: Principal,
    /// The human/operator the run acts on behalf of (the trigger actor — the held-set re-check).
    pub trigger_actor: Principal,
    /// The run id (the MCP session is one run; the token's life == the run's life).
    pub run_id: RunId,
    /// The delegation conjuncts the mint intersection composes.
    pub input: DelegationInput,
    /// The frozen-ABI delegation caveat carrier (the projection of `input.delegation`).
    pub caveats: DelegationCaveats,
    /// The token kind (`Agent` for a local-Claude run — a per-run agent token).
    pub kind: MachineKind,
    /// The run's life (the `expires_at` window of the minted per-run token).
    pub ttl: FailStaticBound,
}

/// The outcome of a governed `tools/call`, mapped by the server to a JSON-RPC result/error. Every
/// variant carries the run-token `jti` so the response is attributable to the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallOutcome {
    /// The effect was applied through `EffectApi::apply` — carries the emitted domain event id.
    Applied { event_id: String, jti: String },
    /// The effect is WITHHELD pending an HITL approval (a `requires_approval` tool, no approval) —
    /// it did NOT mutate. Carries the gate id.
    Gated { gate_id: String, jti: String },
    /// The effect was denied (a revoked/expired run token, a refused mint, or an `EffectApi` deny).
    Denied { reason: String, jti: String },
}

impl CallOutcome {
    /// The run-token jti this outcome is attributed to.
    pub fn jti(&self) -> &str {
        match self {
            CallOutcome::Applied { jti, .. }
            | CallOutcome::Gated { jti, .. }
            | CallOutcome::Denied { jti, .. } => jti,
        }
    }
}

/// One audit row — every governed call is attributed to the run (the jti + the principal + the tool +
/// the outcome). This is the "auditable to the run" leg of the MR-006 binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEntry {
    /// The run-token jti the call acted under.
    pub jti: String,
    /// The acting principal id.
    pub principal: String,
    /// The tool invoked.
    pub tool: String,
    /// The governed outcome.
    pub outcome: CallOutcome,
}

/// Per-run mutable state (the minted token + the audit trail). Behind a `RefCell` because the glue
/// `EffectApi::apply` is `&self` and the stdio loop is single-threaded.
#[derive(Default)]
struct RunState {
    token: Option<RunToken>,
    audit: Vec<AuditEntry>,
}

/// **THE GOVERNANCE ROUTER** — the MR-006 chokepoint for MCP `tools/call`. Holds the per-run token
/// minter, the run identity, and the injected `EffectApi` body. Routes every call through
/// `mint_run_token → (revocation consult) → (HITL gate) → EffectApi::apply`, attributing each to the
/// run. NEVER a bare PAT; NEVER a direct mutation.
pub struct GovernedRouter {
    minter: RunTokenMinter,
    principal: RunPrincipal,
    effect_api: Box<dyn EffectApi>,
    state: RefCell<RunState>,
}

impl GovernedRouter {
    /// Build a router over the injected per-run minter, the run identity, and the `EffectApi` body
    /// (the platform-owned governance chokepoint — `myelin_agent_service::PlanThenApply` in
    /// production, [`SkeletonEffectApi`] for the routing proof).
    pub fn new(
        minter: RunTokenMinter,
        principal: RunPrincipal,
        effect_api: Box<dyn EffectApi>,
    ) -> GovernedRouter {
        GovernedRouter { minter, principal, effect_api, state: RefCell::new(RunState::default()) }
    }

    /// The per-run minter (so a caller/test can revoke the run token + read the revocation telemetry).
    pub fn minter(&self) -> &RunTokenMinter {
        &self.minter
    }

    /// The run identity (so a caller/test can read the scope/principal the run acts as).
    pub fn principal(&self) -> &RunPrincipal {
        &self.principal
    }

    /// The per-run token the run has minted + is acting under (`None` before the first call).
    pub fn current_token(&self) -> Option<RunToken> {
        self.state.borrow().token.clone()
    }

    /// The audit trail accumulated for this run (every governed call, attributed to the jti).
    pub fn audit(&self) -> Vec<AuditEntry> {
        self.state.borrow().audit.clone()
    }

    /// **Mint the per-run token (idempotent for the run).** The run's first `tools/call` mints the
    /// token; subsequent calls act under the SAME token (the run's life == the session's life). A
    /// mint refusal (a non-positive TTL / a self-hosted scope violation) is surfaced LOUDLY — never a
    /// fabricated token.
    fn ensure_run_token(&self, now: &Timestamp) -> Result<RunToken, String> {
        if let Some(t) = self.state.borrow().token.clone() {
            return Ok(t);
        }
        let p = &self.principal;
        let token = self
            .minter
            .mint_run_token(
                &p.scope,
                &p.agent_id,
                &p.run_id,
                &p.agent,
                &p.trigger_actor,
                &p.input,
                &p.caveats,
                p.kind,
                &p.ttl,
                now,
            )
            .map_err(|e| format!("per-run token mint refused: {e}"))?;
        self.state.borrow_mut().token = Some(token.clone());
        Ok(token)
    }

    /// **Route one governed `tools/call`** (the MR-006 binding, in order, fail-closed):
    /// mint → revocation consult → HITL gate → `EffectApi::apply` → audit. `approval_granted` is the
    /// HITL signal (a withheld `requires_approval` tool is re-driven with this true after the human
    /// approves the card; absent it, a gated tool does NOT mutate).
    pub fn call(
        &self,
        tool: &RegisteredTool,
        args: &serde_json::Value,
        now: &Timestamp,
        approval_granted: bool,
    ) -> CallOutcome {
        // (1) MINT the per-run attenuated token (NOT a bare PAT). A refusal is a loud Denied.
        let token = match self.ensure_run_token(now) {
            Ok(t) => t,
            // No token was minted ⇒ attribute the deny to the (would-be) run, jti unknown.
            Err(reason) => {
                return self.record(tool.name(), CallOutcome::Denied { reason, jti: "<unminted>".into() })
            }
        };
        let jti = token.jti.clone();

        // (2) DURABLE REVOCATION CONSULT — a revoked/expired run token is denied, never routed.
        if !self.minter.is_live(&self.principal.scope, &token, now) {
            return self.record(
                tool.name(),
                CallOutcome::Denied {
                    reason: "run token is revoked or expired (MR-011 durable revocation) — denied"
                        .into(),
                    jti,
                },
            );
        }

        // (3) HITL GATE (BEFORE apply) — a frozen `requires_approval` tool with no approval is
        //     withheld and does NOT mutate (the AG-8 leg). The flag is git's frozen `agent_tools()`
        //     default (git.merge = yes), REUSED — not re-decided here.
        if tool.requires_approval() && !approval_granted {
            let gate_id = format!("hitl:{jti}:{}", tool.name());
            return self.record(tool.name(), CallOutcome::Gated { gate_id, jti });
        }

        // (4) ROUTE THE EFFECT THROUGH THE `EffectApi` CHOKEPOINT under a RunCtx that carries the
        //     run-token jti + the principal (the attribution the audit + the platform key on). This
        //     is plan-then-apply: the brain proposes, the platform applies. NEVER a direct mutation.
        let run_ctx = run_ctx_for(&jti, &self.principal.agent_id.0, tool.name());
        let effect = proposed_effect_for(tool.name(), args);
        let outcome = match self.effect_api.apply(&run_ctx, effect) {
            EffectResult::Applied(ev) => CallOutcome::Applied { event_id: ev.0, jti },
            EffectResult::Gated(g) => CallOutcome::Gated { gate_id: g.0, jti },
            EffectResult::Denied(reason) => CallOutcome::Denied { reason, jti },
        };
        self.record(tool.name(), outcome)
    }

    /// Append the outcome to the run's audit trail, attributed to the jti + the principal + the tool.
    fn record(&self, tool: &str, outcome: CallOutcome) -> CallOutcome {
        let entry = AuditEntry {
            jti: outcome.jti().to_string(),
            principal: self.principal.agent_id.0.clone(),
            tool: tool.to_string(),
            outcome: outcome.clone(),
        };
        self.state.borrow_mut().audit.push(entry);
        outcome
    }
}

/// Build the [`RunCtx`] an effect is applied under — it carries the run-token `jti` + the principal +
/// the tool, so the `EffectApi` (and the audit) can attribute the mutation to the run. `RunCtx` is an
/// opaque-string carrier at the glue boundary (the rich shape lands with the SKELETON runtime); the
/// MCP server packs the attribution facts into it.
pub fn run_ctx_for(jti: &str, principal_id: &str, tool: &str) -> RunCtx {
    RunCtx(format!("runtok:{jti}|principal:{principal_id}|tool:{tool}"))
}

/// Build the [`ProposedEffect`] from the tool name + the call arguments. The brain only ever
/// PROPOSES; the platform `EffectApi` validates + applies (plan-then-apply). Opaque-string carrier at
/// the glue boundary.
pub fn proposed_effect_for(tool: &str, args: &serde_json::Value) -> ProposedEffect {
    ProposedEffect(format!("tool:{tool}|args:{args}"))
}

/// **A reference `EffectApi` chokepoint impl (the routing proof — NOT the production body).** Records
/// the [`RunCtx`] + the [`ProposedEffect`] it is handed (so a test can prove the call genuinely
/// routed through `EffectApi::apply` attributed to the minted run token), and returns `Applied`. The
/// PRODUCTION body is `myelin_agent_service::PlanThenApply` (the eight-step pipeline) injected by the
/// composition root; the durable git EFFECT a real apply performs is the Git track E1.1. This is the
/// brain-agnostic chokepoint shape — identical for the mock runtime, a hosted LLM, and local Claude.
#[derive(Default)]
pub struct SkeletonEffectApi {
    calls: RefCell<Vec<(String, String)>>,
}

impl SkeletonEffectApi {
    /// A fresh recording skeleton chokepoint.
    pub fn new() -> SkeletonEffectApi {
        SkeletonEffectApi { calls: RefCell::new(Vec::new()) }
    }

    /// The `(RunCtx, ProposedEffect)` pairs this chokepoint was handed — proof the routing went
    /// THROUGH `EffectApi::apply` (and with what attribution).
    pub fn recorded(&self) -> Vec<(String, String)> {
        self.calls.borrow().clone()
    }
}

impl EffectApi for SkeletonEffectApi {
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        self.calls.borrow_mut().push((run.0.clone(), effect.0.clone()));
        // The governance pipeline + attribution are REAL; the durable git effect is E1.1. A
        // deterministic Applied(event id) keyed to the run is a valid skeleton apply outcome.
        EffectResult::Applied(myelin_agent::EventId(format!("evt:{}", run.0)))
    }
}
