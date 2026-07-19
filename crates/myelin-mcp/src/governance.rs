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
//! ## Durable composition and transaction boundary
//! The production binary injects the durable Git effect adapter and PostgreSQL-backed identity,
//! revocation, delegation, HITL, and outbox stores. [`SkeletonEffectApi`] remains only a reference
//! test adapter. Governance audit intent commits before a mutation; the Git backend then commits its
//! own authoritative domain event with the mutation. Those are separate durable boundaries: this
//! module does not claim atomicity between the PostgreSQL audit intent and filesystem-backed Git.

use std::cell::RefCell;
use std::sync::Arc;

use myelin_agent::{EffectApi, EffectAuthority, EffectResult, ProposedEffect, RunCtx};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EventDraft, EventType, IdMinter,
    OutboxStore, OutboxTx, Timestamp, Visibility,
};
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, RevokeTarget, RunId, RunToken,
};
use myelin_identity_service::delegation::authority_of;
use myelin_identity_service::delegation_policy::ResolvedDelegationPolicy;
use myelin_identity_service::machine_auth::MachineKind;
use myelin_identity_service::mint::RunTokenMinter;
use myelin_storage::hitl_gate_durable::{
    opaque_gate_id, GateDecideError, GateRecord, GateState, HitlVerdictStore,
    DEFAULT_HITL_GATE_TTL_SECS,
};
use myelin_storage::{ContentHash, TenantScope};

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
    /// Signed trigger credential revocation handle, reconsulted immediately before lazy run mint.
    pub trigger_credential_jti: String,
    /// Signed trigger expiry. The run token TTL is clamped so it cannot outlive this credential.
    pub trigger_expires_at_unix: i64,
    /// The run id (the MCP session is one run; the token's life == the run's life).
    pub run_id: RunId,
    /// Server-resolved durable policy, including the positive snapshot cursor signed into the
    /// AgentRun credential. Production has no raw-policy fallback.
    pub resolved_policy: ResolvedDelegationPolicy,
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
    /// Application may have happened, but the post-apply audit observation did not durably commit.
    /// The process must stop after returning this honest outcome; retry safety is unknown.
    Indeterminate { reason: String, jti: String },
}

impl CallOutcome {
    /// The run-token jti this outcome is attributed to.
    pub fn jti(&self) -> &str {
        match self {
            CallOutcome::Applied { jti, .. }
            | CallOutcome::Gated { jti, .. }
            | CallOutcome::Denied { jti, .. }
            | CallOutcome::Indeterminate { jti, .. } => jti,
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

/// The durable audit phases surrounding a mutation. An `Attempt` is committed before
/// [`EffectApi::apply_authorized`], so an unavailable audit store prevents the effect. `Outcome`
/// records the observed result afterwards; it is intentionally not claimed atomic with a
/// filesystem-backed Git mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditPhase {
    Attempt,
    Outcome,
    Approved,
    Rejected,
    Expired,
}

/// One bounded governance audit fact. Grouping the attribution fields keeps the persistence
/// interface difficult to call with accidentally reordered string arguments.
pub struct GovernanceAuditRecord<'a> {
    pub scope: &'a TenantScope,
    pub actor: &'a Principal,
    pub run_id: &'a RunId,
    /// Opaque durable gate identifier for decision/expiry facts. Ordinary tool calls omit it.
    pub gate_id: Option<&'a str>,
    pub tool: &'a str,
    pub jti: &'a str,
    pub phase: AuditPhase,
    pub outcome: Option<&'a CallOutcome>,
    pub now: &'a Timestamp,
}

/// Persistence boundary for MCP governance audit. Production injects [`OutboxGovernanceAudit`]
/// over the PostgreSQL outbox; tests may inject the same type over the test-support store.
pub trait GovernanceAudit: Send + Sync {
    fn record(&self, record: GovernanceAuditRecord<'_>) -> Result<(), String>;
}

/// Governance audit committed through the shared transactional outbox.
pub struct OutboxGovernanceAudit {
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
}

impl OutboxGovernanceAudit {
    pub fn new(outbox: OutboxStore, minter: Arc<dyn IdMinter>) -> Self {
        Self { outbox, minter }
    }
}

impl GovernanceAudit for OutboxGovernanceAudit {
    fn record(&self, record: GovernanceAuditRecord<'_>) -> Result<(), String> {
        let GovernanceAuditRecord {
            scope,
            actor,
            run_id,
            gate_id,
            tool,
            jti,
            phase,
            outcome,
            now,
        } = record;
        let event_type = audit_event_type(tool, phase, outcome)?;
        let run_ref = format!("myelin://{}/agent/run/{}", scope.tenant().0, run_id.0);
        let gate_ref = gate_id.map(|gate_id| format!("{run_ref}/hitl-gate/{gate_id}"));
        let subject_ref = gate_ref.clone().unwrap_or_else(|| run_ref.clone());
        let mut payload = serde_json::json!({
            "run_ref": run_ref,
            "token_ref": format!("jti:{jti}"),
        });
        if let Some(gate_ref) = gate_ref {
            payload["gate_ref"] = serde_json::Value::String(gate_ref);
        }
        let mut tx = self.outbox.begin(
            self.minter.clone(),
            myelin_events::EmitContextBase {
                tenant: scope.tenant().clone(),
                region: scope.region().clone(),
                actor: Actor(actor.clone()),
                schema_ver: 1,
                occurred_at: now.clone(),
                recorded_at: now.clone(),
                caused_by: Some(CausedBy(format!("mcp:{}", run_id.0))),
            },
        );
        tx.emit(
            EventDraft {
                type_: EventType(event_type.into()),
                subject: ArtifactRef(subject_ref),
                aggregate: AggregateKey(format!("mcp-run:{}", run_id.0)),
                payload,
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
            None,
        )
        .map_err(|error| error.0)?;
        tx.commit().map_err(|error| error.0)
    }
}

fn audit_event_type(
    tool: &str,
    phase: AuditPhase,
    outcome: Option<&CallOutcome>,
) -> Result<&'static str, String> {
    let suffix = match (phase, outcome) {
        (AuditPhase::Attempt, None) => "attempted",
        (AuditPhase::Outcome, Some(CallOutcome::Applied { .. })) => "applied",
        (AuditPhase::Outcome, Some(CallOutcome::Gated { .. })) => "gated",
        (AuditPhase::Outcome, Some(CallOutcome::Denied { .. })) => "denied",
        (AuditPhase::Outcome, Some(CallOutcome::Indeterminate { .. })) => "indeterminate",
        (AuditPhase::Approved, None) => "approved",
        (AuditPhase::Rejected, None) => "rejected",
        (AuditPhase::Expired, None) => "expired",
        _ => return Err("invalid governance audit phase/outcome pairing".into()),
    };
    match (tool, suffix) {
        ("git.merge", "attempted") => Ok(myelin_git::events::GIT_MERGE_ATTEMPTED),
        ("git.merge", "applied") => Ok(myelin_git::events::GIT_MERGE_APPLIED),
        ("git.merge", "gated") => Ok(myelin_git::events::GIT_MERGE_GATED),
        ("git.merge", "denied") => Ok(myelin_git::events::GIT_MERGE_DENIED),
        ("git.merge", "indeterminate") => Ok(myelin_git::events::GIT_MERGE_INDETERMINATE),
        ("git.merge", "approved") => Ok(myelin_git::events::GIT_MERGE_APPROVED),
        ("git.merge", "rejected") => Ok(myelin_git::events::GIT_MERGE_REJECTED),
        ("git.merge", "expired") => Ok(myelin_git::events::GIT_MERGE_EXPIRED),
        ("git.open_pr", "attempted") => Ok(myelin_git::events::GIT_OPEN_PR_ATTEMPTED),
        ("git.open_pr", "applied") => Ok(myelin_git::events::GIT_OPEN_PR_APPLIED),
        ("git.open_pr", "gated") => Ok(myelin_git::events::GIT_OPEN_PR_GATED),
        ("git.open_pr", "denied") => Ok(myelin_git::events::GIT_OPEN_PR_DENIED),
        ("git.open_pr", "indeterminate") => Ok(myelin_git::events::GIT_OPEN_PR_INDETERMINATE),
        ("git.submit_review", "attempted") => Ok(myelin_git::events::GIT_SUBMIT_REVIEW_ATTEMPTED),
        ("git.submit_review", "applied") => Ok(myelin_git::events::GIT_SUBMIT_REVIEW_APPLIED),
        ("git.submit_review", "gated") => Ok(myelin_git::events::GIT_SUBMIT_REVIEW_GATED),
        ("git.submit_review", "denied") => Ok(myelin_git::events::GIT_SUBMIT_REVIEW_DENIED),
        ("git.submit_review", "indeterminate") => {
            Ok(myelin_git::events::GIT_SUBMIT_REVIEW_INDETERMINATE)
        }
        ("git.endorse_fork_ci", "attempted") => {
            Ok(myelin_git::events::GIT_ENDORSE_FORK_CI_ATTEMPTED)
        }
        ("git.endorse_fork_ci", "applied") => Ok(myelin_git::events::GIT_ENDORSE_FORK_CI_APPLIED),
        ("git.endorse_fork_ci", "gated") => Ok(myelin_git::events::GIT_ENDORSE_FORK_CI_GATED),
        ("git.endorse_fork_ci", "denied") => Ok(myelin_git::events::GIT_ENDORSE_FORK_CI_DENIED),
        ("git.endorse_fork_ci", "indeterminate") => {
            Ok(myelin_git::events::GIT_ENDORSE_FORK_CI_INDETERMINATE)
        }
        _ => Err("governance audit refused an unregistered tool/outcome taxonomy".into()),
    }
}

/// Per-run mutable state (the minted token + the audit trail). Behind a `RefCell` because the glue
/// `EffectApi::apply` is `&self` and the stdio loop is single-threaded.
#[derive(Default)]
struct RunState {
    token: Option<RunToken>,
    /// The exact effective grant set recorded by the same intersection proof that minted `token`.
    effective_grants: std::collections::BTreeSet<String>,
    audit: Vec<AuditEntry>,
    fatal: bool,
}

/// Resolve the active Human principals eligible to decide one exact gated effect. Production uses
/// a live object-scoped Git ReBAC policy. Tests inject their own explicit fixture implementation;
/// there is no production static-list constructor.
pub trait GateApproverPolicy: Send + Sync {
    fn eligible_approvers(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<Vec<PrincipalId>, String>;
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
    approver_policy: Arc<dyn GateApproverPolicy>,
    audit_sink: Arc<dyn GovernanceAudit>,
}

impl GovernedRouter {
    /// Build a router over the injected per-run minter, the run identity, the `EffectApi` body
    /// (the platform-owned governance chokepoint — `myelin_agent_service::PlanThenApply` in
    /// production, [`SkeletonEffectApi`] for the routing proof), the server-side HITL verdict
    /// store (R2.4 — durable in production), and the approver set for this run's gates.
    /// Build a router with a per-effect, object-scoped approver resolver.
    pub fn with_approver_policy(
        minter: RunTokenMinter,
        principal: RunPrincipal,
        effect_api: Box<dyn EffectApi>,
        verdicts: HitlVerdictStore,
        approver_policy: Arc<dyn GateApproverPolicy>,
        audit_sink: Arc<dyn GovernanceAudit>,
    ) -> GovernedRouter {
        GovernedRouter {
            minter,
            principal,
            effect_api,
            state: RefCell::new(RunState::default()),
            verdicts: RefCell::new(verdicts),
            approver_policy,
            audit_sink,
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

    /// A post-apply audit failure makes the session terminal: serving another request could turn an
    /// ambiguous mutation into an unsafe retry chain.
    pub fn is_fatal(&self) -> bool {
        self.state.borrow().fatal
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
    /// fabricated token. The trigger credential is rechecked immediately before this lazy mint.
    /// After mint, the session is governed by the independently revocable run token whose expiry is
    /// clamped to the trigger expiry; trigger revocation does not retroactively rewrite that signed
    /// run credential.
    fn ensure_run_token(&self, now: &Timestamp) -> Result<RunToken, String> {
        if let Some(t) = self.state.borrow().token.clone() {
            return Ok(t);
        }
        let p = &self.principal;
        let now_unix = chrono::DateTime::parse_from_rfc3339(&now.0)
            .map_err(|_| "current time is not a valid RFC3339 instant".to_string())?
            .timestamp();
        if now_unix >= p.trigger_expires_at_unix {
            return Err("authenticated MCP trigger credential is expired".into());
        }
        if self.minter.revocations().is_revoked(
            &p.scope,
            &RevokeTarget::Jti(p.trigger_credential_jti.clone()),
            now,
        ) {
            return Err("authenticated MCP trigger credential is revoked".into());
        }
        let remaining = u64::try_from(p.trigger_expires_at_unix - now_unix)
            .map_err(|_| "trigger credential remaining lifetime is invalid".to_string())?;
        let ttl = FailStaticBound {
            static_max_secs: p.ttl.static_max_secs.min(remaining),
        };
        let token = self
            .minter
            .mint_from_resolved_policy(
                &p.scope,
                &p.agent_id,
                &p.run_id,
                &p.agent,
                &p.trigger_actor,
                &p.resolved_policy,
                &p.caveats,
                p.kind,
                &ttl,
                now,
            )
            .map_err(|e| format!("per-run token mint refused: {e}"))?;
        let mut state = self.state.borrow_mut();
        state.effective_grants = authority_of(p.resolved_policy.effective_policy())
            .grants()
            .map(str::to_string)
            .collect();
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
        idempotency_key: &str,
        now: &Timestamp,
        presented_gate_id: Option<&str>,
    ) -> CallOutcome {
        let now_unix = match timestamp_to_unix(now) {
            Ok(value) => value,
            Err(reason) => {
                return self.push_local_audit(
                    tool.name(),
                    CallOutcome::Denied {
                        reason,
                        jti: "<unminted>".into(),
                    },
                )
            }
        };
        // (1) MINT the per-run attenuated token (NOT a bare PAT). A refusal is a loud Denied.
        let token = match self.ensure_run_token(now) {
            Ok(t) => t,
            // No token was minted ⇒ attribute the deny to the (would-be) run, jti unknown.
            Err(reason) => {
                return self.record(
                    tool.name(),
                    CallOutcome::Denied {
                        reason,
                        jti: "<unminted>".into(),
                    },
                    now,
                )
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
                now,
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
                }, now,
            );
        }

        // (4) HITL GATE (BEFORE apply) — a frozen `requires_approval` tool is withheld unless the
        //     SERVER-SIDE verdict store holds an `approved` gate for THIS exact effect, decided by
        //     a DISTINCT principal (R2.4). The flag is git's frozen `agent_tools()` default
        //     (git.merge = yes), REUSED — not re-decided here.
        let mut approval_to_consume = None;
        if tool.requires_approval() {
            let effect_key = mcp_effect_key(tool.name(), args);
            // Production expiry is deliberately lazy and ownership-scoped: a gated call advances
            // only elapsed rows for this exact MCP run, requester, and canonical effect. Other
            // shared agent-service gates remain untouched until their owning path reconciles them.
            // State and terminal audit are separate commits: an audit outage after expiry is
            // therefore fail-loud and makes this session terminal.
            let expired = self.verdicts.borrow_mut().expire_due_for_effect(
                &self.principal.scope,
                &self.principal.run_id.0,
                &self.principal.agent_id.0,
                &effect_key,
                now_unix,
            );
            if !expired.is_empty() && self.record_expired_gates(&expired, now).is_err() {
                self.state.borrow_mut().fatal = true;
                return self.push_local_audit(
                    tool.name(),
                    CallOutcome::Indeterminate {
                        reason: "HITL expiry committed but its governance audit did not; session terminated"
                            .into(),
                        jti,
                    },
                );
            }
            match presented_gate_id {
                // No gate presented → withhold the Git effect, but open (or resurface) a durable
                // pending gate row and return its OPAQUE server-issued id. Opening the row is a
                // governance-state mutation, so its durable audit intent is recorded first.
                None => {
                    if self
                        .audit_sink
                        .record(GovernanceAuditRecord {
                            scope: &self.principal.scope,
                            actor: &self.principal.agent,
                            run_id: &self.principal.run_id,
                            gate_id: None,
                            tool: tool.name(),
                            jti: &jti,
                            phase: AuditPhase::Attempt,
                            outcome: None,
                            now,
                        })
                        .is_err()
                    {
                        return self.push_local_audit(
                            tool.name(),
                            CallOutcome::Denied {
                                reason: "durable pre-gate audit is unavailable; effect withheld"
                                    .into(),
                                jti,
                            },
                        );
                    }
                    let gate_id =
                        match self.open_or_resurface_gate(tool.name(), args, &effect_key, now_unix)
                        {
                            Ok(gate) => gate,
                            Err(reason) => {
                                return self.record(
                                    tool.name(),
                                    CallOutcome::Denied { reason, jti },
                                    now,
                                )
                            }
                        };
                    let outcome = CallOutcome::Gated {
                        gate_id: gate_id.0,
                        jti,
                    };
                    return if gate_id.1 {
                        self.record_after_mutation(tool.name(), outcome, now)
                    } else {
                        self.record(tool.name(), outcome, now)
                    };
                }
                // A gate id presented → LOOK IT UP in the verdict store. Never trust the caller.
                Some(gid) => {
                    let verdict = self.verdicts.borrow().fetch(&self.principal.scope, gid);
                    match verdict {
                        // Approved, for THIS effect, by a distinct principal → the gate clears;
                        // fall through to EffectApi::apply.
                        Some(rec)
                            if rec.authorizes(
                                &effect_key,
                                &self.principal.run_id.0,
                                &self.principal.agent_id.0,
                            ) =>
                        {
                            approval_to_consume = Some(gid.to_string());
                        }
                        // The gate is real and pending for this effect → still withheld.
                        Some(rec)
                            if rec.state == GateState::Waiting
                                && rec.effect_id == effect_key
                                && rec.run_id == self.principal.run_id.0
                                && rec.requested_by == self.principal.agent_id.0 =>
                        {
                            return self.record(
                                tool.name(),
                                CallOutcome::Gated {
                                    gate_id: gid.to_string(),
                                    jti,
                                },
                                now,
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
                                now,
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
            idempotency_key: idempotency_key.to_string(),
        };
        let effect = proposed_effect_for(tool.name(), args);
        if self
            .audit_sink
            .record(GovernanceAuditRecord {
                scope: &self.principal.scope,
                actor: &self.principal.agent,
                run_id: &self.principal.run_id,
                gate_id: None,
                tool: tool.name(),
                jti: &jti,
                phase: AuditPhase::Attempt,
                outcome: None,
                now,
            })
            .is_err()
        {
            return self.record(
                tool.name(),
                CallOutcome::Denied {
                    reason: "durable pre-apply audit is unavailable; effect withheld".into(),
                    jti,
                },
                now,
            );
        }
        if let Some(gate_id) = approval_to_consume {
            if let Err(error) = self.verdicts.borrow_mut().consume_approval(
                &self.principal.scope,
                &gate_id,
                &mcp_effect_key(tool.name(), args),
                &self.principal.run_id.0,
                &self.principal.agent_id.0,
                now_unix,
            ) {
                return self.record(
                    tool.name(),
                    CallOutcome::Denied {
                        reason: format!("HITL approval consumption refused: {error}"),
                        jti,
                    },
                    now,
                );
            }
        }
        let outcome = match self
            .effect_api
            .apply_authorized(&run_ctx, &authority, effect)
        {
            EffectResult::Applied(ev) => CallOutcome::Applied {
                event_id: ev.0,
                jti,
            },
            EffectResult::Gated(g) => CallOutcome::Gated { gate_id: g.0, jti },
            EffectResult::Denied(reason) => CallOutcome::Denied { reason, jti },
        };
        self.record_after_mutation(tool.name(), outcome, now)
    }

    /// **Open (or resurface) the pending gate row for `effect_key` (R2.4 withhold).** If a
    /// `waiting` gate for this `(run, effect)` already exists, its id is returned again (a retried
    /// call re-surfaces the SAME pending gate — no duplicate spawn); otherwise a fresh row is
    /// INSERTed under an OPAQUE random gate id. The run's own agent principal is structurally
    /// excluded from the persisted approver filter.
    fn open_or_resurface_gate(
        &self,
        tool: &str,
        args: &serde_json::Value,
        effect_key: &str,
        opened_at_unix: i64,
    ) -> Result<(String, bool), String> {
        let requested_by = self.principal.agent_id.0.clone();
        let mut approvers = self
            .approver_policy
            .eligible_approvers(tool, args)?
            .into_iter()
            .map(|principal| principal.0)
            .filter(|principal| principal != &requested_by)
            .collect::<Vec<_>>();
        approvers.sort();
        approvers.dedup();
        if approvers.is_empty() {
            return Err(
                "no active object-authorized Human approver is available for this effect".into(),
            );
        }
        let mut verdicts = self.verdicts.borrow_mut();
        if let Some(existing) =
            verdicts.find_waiting(&self.principal.scope, &self.principal.run_id.0, effect_key)
        {
            return Ok((existing.gate_id, false));
        }
        let gate_id = opaque_gate_id();
        let record = GateRecord {
            gate_id: gate_id.clone(),
            run_id: self.principal.run_id.0.clone(),
            effect_id: effect_key.to_string(),
            risk_summary: Vec::new(),
            cost_estimate: 0,
            approver_filter: approvers,
            state: GateState::Waiting,
            card_ref: None,
            requested_by,
            decided_by: None,
            opened_at_unix,
            decided_at_unix: None,
            expires_at_unix: opened_at_unix.saturating_add(DEFAULT_HITL_GATE_TTL_SECS),
            approval_consumed_at_unix: None,
        };
        verdicts
            .open(&self.principal.scope, record)
            .map_err(|error| format!("durable HITL gate open refused: {error}"))?;
        Ok((gate_id, true))
    }

    /// **The server-side APPROVAL surface (R2.4 / R2.4b).** The human decision path (the approval
    /// card / operator surface) calls this — never the MCP client. It takes the AUTHENTICATED
    /// approver `Principal` (not a bare id) so the store can enforce, SERVER-SIDE: the approver is a
    /// **`Human`** (R2.4b — a machine/agent/service is refused even if listed), is eligible
    /// (∈ the gate's `approver_filter`), and is distinct from the gate's requester. A refusal
    /// leaves the gate `waiting`.
    pub fn approve_gate(
        &self,
        approver: &Principal,
        gate_id: &str,
        now: &Timestamp,
    ) -> Result<(), GateDecideError> {
        let decided_at_unix = timestamp_to_unix(now).map_err(|_| GateDecideError::NotEligible)?;
        self.verdicts.borrow_mut().approve_at(
            &self.principal.scope,
            gate_id,
            &approver.principal_id.0,
            approver.kind.clone(),
            decided_at_unix,
        )
    }

    /// **The server-side REJECT surface (R2.4).** Settles the gate `rejected` — the effect is
    /// withheld forever (0 mutation, AG-8); a later re-drive presenting this gate id is denied.
    pub fn reject_gate(
        &self,
        decider: &Principal,
        gate_id: &str,
        now: &Timestamp,
    ) -> Result<(), GateDecideError> {
        let decided_at_unix = timestamp_to_unix(now).map_err(|_| GateDecideError::NotEligible)?;
        self.verdicts.borrow_mut().reject_at(
            &self.principal.scope,
            gate_id,
            &decider.principal_id.0,
            decider.kind.clone(),
            decided_at_unix,
        )
    }

    /// Read a gate's current verdict row (the operator/test observability read).
    pub fn gate_verdict(&self, gate_id: &str) -> Option<GateRecord> {
        self.verdicts.borrow().fetch(&self.principal.scope, gate_id)
    }

    fn record_expired_gates(&self, expired: &[GateRecord], now: &Timestamp) -> Result<(), String> {
        let actor = Principal::new(
            self.principal.scope.tenant().clone(),
            self.principal.scope.region().clone(),
            PrincipalId("service:mcp-hitl-expiry".into()),
            myelin_identity::PrincipalKind::Service,
            myelin_identity::DataRole::Controller,
            myelin_identity::PrincipalStatus::Active,
        );
        for gate in expired {
            let tool = if git_merge_repo_from_effect_key(&gate.effect_id).is_some() {
                "git.merge"
            } else {
                return Err("expired gate has no registered governance audit taxonomy".into());
            };
            let run_id = RunId(gate.run_id.clone());
            self.audit_sink.record(GovernanceAuditRecord {
                scope: &self.principal.scope,
                actor: &actor,
                run_id: &run_id,
                gate_id: Some(&gate.gate_id),
                tool,
                jti: "system:hitl-expiry",
                phase: AuditPhase::Expired,
                outcome: None,
                now,
            })?;
        }
        Ok(())
    }

    /// Append the outcome to the run's audit trail, attributed to the jti + the principal + the tool.
    fn record(&self, tool: &str, outcome: CallOutcome, now: &Timestamp) -> CallOutcome {
        let recorded = match self.audit_sink.record(GovernanceAuditRecord {
            scope: &self.principal.scope,
            actor: &self.principal.agent,
            run_id: &self.principal.run_id,
            gate_id: None,
            tool,
            jti: outcome.jti(),
            phase: AuditPhase::Outcome,
            outcome: Some(&outcome),
            now,
        }) {
            Ok(()) => outcome,
            Err(_) => CallOutcome::Denied {
                reason: "durable governance audit is unavailable; no effect was attempted".into(),
                jti: outcome.jti().to_string(),
            },
        };
        self.push_local_audit(tool, recorded)
    }

    /// Record an outcome after entering the application boundary. A durable audit failure at this
    /// point is not a denial: the backend may already have mutated and emitted its domain event.
    fn record_after_mutation(
        &self,
        tool: &str,
        outcome: CallOutcome,
        now: &Timestamp,
    ) -> CallOutcome {
        let recorded = match self.audit_sink.record(GovernanceAuditRecord {
            scope: &self.principal.scope,
            actor: &self.principal.agent,
            run_id: &self.principal.run_id,
            gate_id: None,
            tool,
            jti: outcome.jti(),
            phase: AuditPhase::Outcome,
            outcome: Some(&outcome),
            now,
        }) {
            Ok(()) => outcome,
            Err(_) => {
                self.state.borrow_mut().fatal = true;
                CallOutcome::Indeterminate {
                    reason: "effect outcome is indeterminate because post-apply audit persistence failed; session terminated"
                        .into(),
                    jti: outcome.jti().to_string(),
                }
            }
        };
        self.push_local_audit(tool, recorded)
    }

    fn push_local_audit(&self, tool: &str, outcome: CallOutcome) -> CallOutcome {
        self.state.borrow_mut().audit.push(AuditEntry {
            jti: outcome.jti().to_string(),
            principal: self.principal.agent_id.0.clone(),
            tool: tool.to_string(),
            outcome: outcome.clone(),
        });
        outcome
    }
}

/// **The PER-EFFECT key an MCP approval is bound to (R2.4).** `mcp:{tool}:{args}` over the
/// canonical `serde_json` serialisation (object keys are sorted — `serde_json`'s default `Map` is
/// a `BTreeMap`), so an approval granted for `git.merge {number: 7}` NEVER clears a re-drive of
/// `git.merge {number: 8}` — the approval is bound to the exact effect, not the tool name.
fn timestamp_to_unix(timestamp: &Timestamp) -> Result<i64, String> {
    chrono::DateTime::parse_from_rfc3339(&timestamp.0)
        .map(|value| value.timestamp())
        .map_err(|_| "internal governance clock did not produce a valid RFC-3339 timestamp".into())
}

pub fn mcp_effect_key(tool: &str, args: &serde_json::Value) -> String {
    let canonical = canonical_json(args);
    let mut bound = Vec::with_capacity(tool.len() + 1 + canonical.to_string().len());
    bound.extend_from_slice(tool.as_bytes());
    bound.push(0);
    bound.extend_from_slice(canonical.to_string().as_bytes());
    let digest = ContentHash::blake3(&bound).to_multihash_string();
    if tool == "git.merge" {
        if let Some(repo) = args
            .get("repo")
            .and_then(serde_json::Value::as_str)
            .filter(|repo| !repo.is_empty() && repo.len() <= 255)
        {
            return format!(
                "mcp:v1:git.merge:repohex:{}:{digest}",
                hex_encode(repo.as_bytes())
            );
        }
    }
    format!("mcp:v1:{tool}:{digest}")
}

/// Recover the bounded repository name deliberately bound into a canonical git.merge gate key.
/// Operator decisions use this server-created value for their live object authorization check;
/// caller-supplied CLI values never select the authorization object.
pub fn git_merge_repo_from_effect_key(effect_key: &str) -> Option<String> {
    let rest = effect_key.strip_prefix("mcp:v1:git.merge:repohex:")?;
    let (encoded, digest) = rest.split_once(":blake3:")?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = hex_decode(encoded)?;
    if bytes.is_empty() || bytes.len() > 255 {
        return None;
    }
    let repo = String::from_utf8(bytes).ok()?;
    myelin_git::gix_backend::validate_repo_slug(&repo).ok()?;
    Some(repo)
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut result = serde_json::Map::new();
            for key in keys {
                result.insert(key.clone(), canonical_json(&object[key]));
            }
            serde_json::Value::Object(result)
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        other => other.clone(),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn hex_decode(encoded: &str) -> Option<Vec<u8>> {
    if !encoded.len().is_multiple_of(2) || encoded.len() > 510 {
        return None;
    }
    encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            Some(((hi << 4) | lo) as u8)
        })
        .collect()
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
        SkeletonEffectApi {
            calls: RefCell::new(Vec::new()),
        }
    }

    /// The `(RunCtx, ProposedEffect)` pairs this chokepoint was handed — proof the routing went
    /// THROUGH `EffectApi::apply` (and with what attribution).
    pub fn recorded(&self) -> Vec<(String, String)> {
        self.calls.borrow().clone()
    }
}

impl EffectApi for SkeletonEffectApi {
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        self.calls
            .borrow_mut()
            .push((run.0.clone(), effect.0.clone()));
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

#[cfg(test)]
mod security_tests {
    use super::{git_merge_repo_from_effect_key, mcp_effect_key};

    #[test]
    fn effect_key_is_canonical_bounded_and_binds_the_validated_repo() {
        let mut first = serde_json::Map::new();
        first.insert("repo".into(), serde_json::json!("team/alpha"));
        first.insert("number".into(), serde_json::json!(7));
        let mut second = serde_json::Map::new();
        second.insert("number".into(), serde_json::json!(7));
        second.insert("repo".into(), serde_json::json!("team/alpha"));
        let first = mcp_effect_key("git.merge", &serde_json::Value::Object(first));
        let second = mcp_effect_key("git.merge", &serde_json::Value::Object(second));
        assert_eq!(
            first, second,
            "object insertion order cannot change gate identity"
        );
        assert!(first.len() < 700);
        assert_eq!(
            git_merge_repo_from_effect_key(&first).as_deref(),
            Some("team/alpha")
        );

        let huge = mcp_effect_key(
            "git.open_pr",
            &serde_json::json!({"title":"x".repeat(900_000)}),
        );
        assert!(
            huge.len() < 128,
            "caller args are represented only by a digest"
        );
    }

    #[test]
    fn merge_effect_key_parser_refuses_malformed_or_aliasing_repo_names() {
        let traversal = mcp_effect_key(
            "git.merge",
            &serde_json::json!({"repo":"../secrets","number":1}),
        );
        assert!(git_merge_repo_from_effect_key(&traversal).is_none());
        assert!(git_merge_repo_from_effect_key(
            "mcp:v1:git.merge:repohex:zz:blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        .is_none());
    }
}
