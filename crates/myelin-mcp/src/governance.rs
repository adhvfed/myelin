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
//! 3. **checks every subsystem-declared required capability** against the exact effective grant set
//!    recorded by the same intersection proof that minted the token; the concrete mutation adapter
//!    independently verifies the signed token and repeats this check at its final boundary;
//! 4. **HITL-gates** a `requires_approval` tool (the FROZEN flag from git's `agent_tools()`) BEFORE
//!    apply — a gated tool with no approval is **withheld** and **does NOT mutate** (the AG-8 leg).
//!    **R2.4:** the approval is a SERVER-SIDE VERDICT — the withhold opens a `waiting` row in the
//!    durable verdict store (`agent_hitl_gate`, migration 0054) under an OPAQUE server-issued gate
//!    id; the re-drive PRESENTS that id and the gate clears ONLY if the store says it is Approved
//!    for that exact effect by a DISTINCT human principal. The caller-supplied `approval.granted`
//!    boolean of the 2026-07-06 finding is dead — it is not even parsed;
//! 5. **routes the effect through `EffectApi::apply_authorized`** (the platform-owned
//!    PLAN-THEN-APPLY chokepoint,
//!    §8.2) under a [`RunCtx`] that carries the run-token `jti` + the principal — NOT a direct
//!    mutation. `EffectApi` is **brain-agnostic** (identical for the mock runtime, a future hosted
//!    LLM, and local Claude over MCP — the whole point of plan-then-apply);
//! 6. **attributes every call** to the run (the jti + the principal + the tool + the outcome) in an
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

use myelin_agent::{EffectApi, EffectAuthority, EffectResult, ProposedEffect, RunCtx};
use myelin_events::Timestamp;
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, RunId, RunToken,
};
use myelin_identity_service::delegation::DelegationInput;
use myelin_identity_service::machine_auth::MachineKind;
use myelin_identity_service::mint::RunTokenMinter;
use myelin_storage::hitl_gate_durable::{
    opaque_gate_id, GateDecideError, GateRecord, GateState, HitlVerdictStore,
};
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
    /// The exact effective grant set recorded by the same intersection proof that minted `token`.
    effective_grants: std::collections::BTreeSet<String>,
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
    /// **The server-side HITL verdict authority (R2.4).** A gated call INSERTs a `waiting` row here
    /// under an OPAQUE server-issued gate id; a re-drive is admitted ONLY if the presented gate id
    /// is `approved` in THIS store for THIS effect by a DISTINCT principal. The caller's
    /// `approval.granted` boolean is never an enforcement input. Durable in production
    /// ([`HitlVerdictStore::with_pg`] over `agent_hitl_gate`, migration 0054); the in-memory arm is
    /// the test double.
    verdicts: RefCell<HitlVerdictStore>,
    /// The principals eligible to approve this run's gates (the `approver_filter` — in production
    /// `list_subjects(object, approve_perm)`, supplied by the composition root). The run's own
    /// agent principal is structurally EXCLUDED at gate-open time, so a self-approval is
    /// unrepresentable even if the composition lists it here.
    approvers: Vec<PrincipalId>,
}

impl GovernedRouter {
    /// Build a router over the injected per-run minter, the run identity, the `EffectApi` body
    /// (the platform-owned governance chokepoint — `myelin_agent_service::PlanThenApply` in
    /// production, [`SkeletonEffectApi`] for the routing proof), the server-side HITL verdict
    /// store (R2.4 — durable in production), and the approver set for this run's gates.
    pub fn new(
        minter: RunTokenMinter,
        principal: RunPrincipal,
        effect_api: Box<dyn EffectApi>,
        verdicts: HitlVerdictStore,
        approvers: Vec<PrincipalId>,
    ) -> GovernedRouter {
        GovernedRouter {
            minter,
            principal,
            effect_api,
            state: RefCell::new(RunState::default()),
            verdicts: RefCell::new(verdicts),
            approvers,
        }
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

    /// Explicitly revoke the current session token. Safe before the first call and idempotent after
    /// EOF/error teardown; the token is retained for audit attribution and becomes non-live.
    pub fn teardown(&self, now: &Timestamp) {
        if let Some(token) = self.current_token() {
            self.minter.teardown(&self.principal.scope, &token, now);
        }
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
        let (token, proof) = self
            .minter
            .mint_proved(
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
        let mut state = self.state.borrow_mut();
        state.effective_grants = proof.effective.into_iter().collect();
        state.token = Some(token.clone());
        Ok(token)
    }

    /// **Route one governed `tools/call`** (the MR-006 binding, in order, fail-closed):
    /// mint → revocation consult → HITL gate → `EffectApi::apply` → audit.
    ///
    /// **R2.4 — the HITL signal is a server-side verdict, never a caller boolean.** A withheld
    /// `requires_approval` tool returns an OPAQUE server-issued gate id (a `waiting` row in the
    /// verdict store); the re-drive PRESENTS that gate id (`presented_gate_id`), and the gate is
    /// cleared ONLY if the store says that specific gate is `approved` — for THIS exact effect
    /// (tool + args), by a DISTINCT human principal (approver ≠ the requesting agent). A made-up /
    /// foreign gate id, a still-waiting gate presented for a different effect, or a self-approved
    /// gate is refused. The old `approval.granted` boolean is NOT an input to this function at all.
    pub fn call(
        &self,
        tool: &RegisteredTool,
        args: &serde_json::Value,
        now: &Timestamp,
        presented_gate_id: Option<&str>,
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

        // (3) DECLARED CAPABILITY BINDING — the registered tool's subsystem-owned caps must all be
        //     inside the exact effective intersection recorded by THIS token's mint proof. This is
        //     an early fail-closed decision; the mutation adapter independently verifies the signed
        //     token and resolves/rechecks the same declaration again at its final boundary.
        let missing = {
            let state = self.state.borrow();
            tool.required_caps()
                .iter()
                .find(|cap| !state.effective_grants.contains(**cap))
                .copied()
        };
        if let Some(cap) = missing {
            return self.record(
                tool.name(),
                CallOutcome::Denied {
                    reason: format!(
                        "tool `{}` requires capability `{cap}` outside the exact minted delegation intersection",
                        tool.name()
                    ),
                    jti,
                },
            );
        }

        // (4) HITL GATE (BEFORE apply) — a frozen `requires_approval` tool is withheld unless the
        //     SERVER-SIDE verdict store holds an `approved` gate for THIS exact effect, decided by
        //     a DISTINCT principal (R2.4). The flag is git's frozen `agent_tools()` default
        //     (git.merge = yes), REUSED — not re-decided here.
        if tool.requires_approval() {
            let effect_key = mcp_effect_key(tool.name(), args);
            match presented_gate_id {
                // No gate presented → withhold: open (or resurface) the pending gate row and
                // return its OPAQUE server-issued id. 0 mutation.
                None => {
                    let gate_id = self.open_or_resurface_gate(&effect_key);
                    return self.record(tool.name(), CallOutcome::Gated { gate_id, jti });
                }
                // A gate id presented → LOOK IT UP in the verdict store. Never trust the caller.
                Some(gid) => {
                    let verdict = self.verdicts.borrow().fetch(&self.principal.scope, gid);
                    match verdict {
                        // Approved, for THIS effect, by a distinct principal → the gate clears;
                        // fall through to EffectApi::apply.
                        Some(rec) if rec.authorizes(&effect_key, &self.principal.agent_id.0) => {}
                        // The gate is real and pending for this effect → still withheld.
                        Some(rec)
                            if rec.state == GateState::Waiting && rec.effect_id == effect_key =>
                        {
                            return self.record(
                                tool.name(),
                                CallOutcome::Gated { gate_id: gid.to_string(), jti },
                            );
                        }
                        // Everything else — unknown/forged id, rejected/expired gate, an approval
                        // bound to a DIFFERENT effect, or a self-decided gate — is a loud deny.
                        _ => {
                            return self.record(
                                tool.name(),
                                CallOutcome::Denied {
                                    reason: format!(
                                        "HITL approval not granted server-side for gate `{gid}` \
                                         on `{}` — the gate must be Approved in the verdict store \
                                         for this exact effect by a distinct human principal; a \
                                         caller-supplied approval is never trusted (R2.4)",
                                        tool.name()
                                    ),
                                    jti,
                                },
                            );
                        }
                    }
                }
            }
        }

        // (5) ROUTE THE EFFECT THROUGH THE `EffectApi` CHOKEPOINT under a RunCtx that carries the
        //     run-token jti + the principal (the attribution the audit + the platform key on). This
        //     is plan-then-apply: the brain proposes, the platform applies. NEVER a direct mutation.
        let run_ctx = run_ctx_for(&jti, &self.principal.agent_id.0, tool.name());
        let authority = EffectAuthority {
            run_token: token,
            principal_id: self.principal.agent_id.clone(),
            tool: tool.name().to_string(),
        };
        let effect = proposed_effect_for(tool.name(), args);
        let outcome = match self.effect_api.apply_authorized(&run_ctx, &authority, effect) {
            EffectResult::Applied(ev) => CallOutcome::Applied { event_id: ev.0, jti },
            EffectResult::Gated(g) => CallOutcome::Gated { gate_id: g.0, jti },
            EffectResult::Denied(reason) => CallOutcome::Denied { reason, jti },
        };
        self.record(tool.name(), outcome)
    }

    /// **Open (or resurface) the pending gate row for `effect_key` (R2.4 withhold).** If a
    /// `waiting` gate for this `(run, effect)` already exists, its id is returned again (a retried
    /// call re-surfaces the SAME pending gate — no duplicate spawn); otherwise a fresh row is
    /// INSERTed under an OPAQUE random gate id. The run's own agent principal is structurally
    /// excluded from the persisted approver filter.
    fn open_or_resurface_gate(&self, effect_key: &str) -> String {
        let mut verdicts = self.verdicts.borrow_mut();
        if let Some(existing) =
            verdicts.find_waiting(&self.principal.scope, &self.principal.run_id.0, effect_key)
        {
            return existing.gate_id;
        }
        let requested_by = self.principal.agent_id.0.clone();
        let gate_id = opaque_gate_id();
        let record = GateRecord {
            gate_id: gate_id.clone(),
            run_id: self.principal.run_id.0.clone(),
            effect_id: effect_key.to_string(),
            risk_summary: Vec::new(),
            cost_estimate: 0,
            approver_filter: self
                .approvers
                .iter()
                .map(|p| p.0.clone())
                .filter(|p| *p != requested_by)
                .collect(),
            state: GateState::Waiting,
            card_ref: None,
            requested_by,
            decided_by: None,
        };
        verdicts
            .open(&self.principal.scope, record)
            .expect("a freshly minted opaque gate id never collides");
        gate_id
    }

    /// **The server-side APPROVAL surface (R2.4 / R2.4b).** The human decision path (the approval
    /// card / operator surface) calls this — never the MCP client. It takes the AUTHENTICATED
    /// approver `Principal` (not a bare id) so the store can enforce, SERVER-SIDE: the approver is a
    /// **`Human`** (R2.4b — a machine/agent/service is refused even if listed), is eligible
    /// (∈ the gate's `approver_filter`), and is distinct from the gate's requester. A refusal
    /// leaves the gate `waiting`.
    pub fn approve_gate(&self, approver: &Principal, gate_id: &str) -> Result<(), GateDecideError> {
        self.verdicts.borrow_mut().approve(
            &self.principal.scope,
            gate_id,
            &approver.principal_id.0,
            approver.kind.clone(),
        )
    }

    /// **The server-side REJECT surface (R2.4).** Settles the gate `rejected` — the effect is
    /// withheld forever (0 mutation, AG-8); a later re-drive presenting this gate id is denied.
    pub fn reject_gate(&self, decider: &PrincipalId, gate_id: &str) -> Result<(), GateDecideError> {
        self.verdicts.borrow_mut().reject(&self.principal.scope, gate_id, &decider.0)
    }

    /// Read a gate's current verdict row (the operator/test observability read).
    pub fn gate_verdict(&self, gate_id: &str) -> Option<GateRecord> {
        self.verdicts.borrow().fetch(&self.principal.scope, gate_id)
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

/// **The PER-EFFECT key an MCP approval is bound to (R2.4).** `mcp:{tool}:{args}` over the
/// canonical `serde_json` serialisation (object keys are sorted — `serde_json`'s default `Map` is
/// a `BTreeMap`), so an approval granted for `git.merge {number: 7}` NEVER clears a re-drive of
/// `git.merge {number: 8}` — the approval is bound to the exact effect, not the tool name.
pub fn mcp_effect_key(tool: &str, args: &serde_json::Value) -> String {
    format!("mcp:{tool}:{args}")
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

    fn apply_authorized(
        &self,
        run: &RunCtx,
        _authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        self.apply(run, effect)
    }
}
