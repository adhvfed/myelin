use std::cell::RefCell;
use std::collections::BTreeSet;
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

use crate::registry::{RegisteredTool, ToolRegistry};

pub struct RunPrincipal {
    pub scope: TenantScope,
    pub agent_id: PrincipalId,
    pub agent: Principal,
    pub trigger_actor: Principal,
    pub trigger_credential_jti: String,
    pub trigger_expires_at_unix: i64,
    pub run_id: RunId,
    pub resolved_policy: ResolvedDelegationPolicy,
    pub caveats: DelegationCaveats,
    pub kind: MachineKind,
    pub ttl: FailStaticBound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernedRun {
    pub scope: TenantScope,
    pub agent_id: PrincipalId,
    pub agent: Principal,
    pub run_id: RunId,
}

impl GovernedRun {
    fn from_minting_principal(principal: &RunPrincipal) -> Self {
        Self {
            scope: principal.scope.clone(),
            agent_id: principal.agent_id.clone(),
            agent: principal.agent.clone(),
            run_id: principal.run_id.clone(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.agent_id != self.agent.principal_id {
            return Err("governed run agent id does not match its authenticated principal".into());
        }
        if self.scope.tenant() != &self.agent.tenant || self.scope.region() != &self.agent.region {
            return Err("governed run scope does not match its authenticated principal".into());
        }
        if self.run_id.0.is_empty() || self.run_id.0.len() > 255 {
            return Err("governed run id must be non-empty and bounded".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallOutcome {
    Applied { event_id: String, jti: String },
    Gated { gate_id: String, jti: String },
    Denied { reason: String, jti: String },
    Indeterminate { reason: String, jti: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadAuthorization {
    run_token: RunToken,
    tool: String,
    required_caps: Vec<String>,
}

impl ReadAuthorization {
    pub fn jti(&self) -> &str {
        &self.run_token.jti
    }

    pub fn run_token(&self) -> &RunToken {
        &self.run_token
    }

    pub fn tool(&self) -> &str {
        &self.tool
    }

    pub fn required_caps(&self) -> &[String] {
        &self.required_caps
    }
}

impl CallOutcome {
    pub fn jti(&self) -> &str {
        match self {
            CallOutcome::Applied { jti, .. }
            | CallOutcome::Gated { jti, .. }
            | CallOutcome::Denied { jti, .. }
            | CallOutcome::Indeterminate { jti, .. } => jti,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditEntry {
    pub jti: String,
    pub principal: String,
    pub tool: String,
    pub outcome: CallOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditPhase {
    Attempt,
    Outcome,
    Approved,
    Rejected,
    Expired,
}

pub struct GovernanceAuditRecord<'a> {
    pub scope: &'a TenantScope,
    pub actor: &'a Principal,
    pub run_id: &'a RunId,
    pub gate_id: Option<&'a str>,
    pub tool: &'a str,
    pub jti: &'a str,
    pub phase: AuditPhase,
    pub outcome: Option<&'a CallOutcome>,
    pub now: &'a Timestamp,
}

pub trait GovernanceAudit: Send + Sync {
    fn record(&self, record: GovernanceAuditRecord<'_>) -> Result<(), String>;
}

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
        ("chat.post", "attempted") => Ok(myelin_chat::events::CHAT_POST_ATTEMPTED),
        ("chat.post", "applied") => Ok(myelin_chat::events::CHAT_POST_APPLIED),
        ("chat.post", "gated") => Ok(myelin_chat::events::CHAT_POST_GATED),
        ("chat.post", "denied") => Ok(myelin_chat::events::CHAT_POST_DENIED),
        ("chat.post", "indeterminate") => Ok(myelin_chat::events::CHAT_POST_INDETERMINATE),
        _ => Err("governance audit refused an unregistered tool/outcome taxonomy".into()),
    }
}

#[derive(Default)]
struct RunState {
    token: Option<RunToken>,
    effective_grants: std::collections::BTreeSet<String>,
    audit: Vec<AuditEntry>,
    fatal: bool,
}

pub trait GateApproverPolicy: Send + Sync {
    fn eligible_approvers(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<Vec<PrincipalId>, String>;
}

pub struct GovernedRouter {
    minter: RunTokenMinter,
    principal: GovernedRun,
    minting_principal: Option<RunPrincipal>,
    effect_api: Box<dyn EffectApi>,
    state: RefCell<RunState>,
    verdicts: RefCell<HitlVerdictStore>,
    approver_policy: Arc<dyn GateApproverPolicy>,
    audit_sink: Arc<dyn GovernanceAudit>,
}

impl GovernedRouter {
    pub fn with_approver_policy(
        minter: RunTokenMinter,
        principal: RunPrincipal,
        effect_api: Box<dyn EffectApi>,
        verdicts: HitlVerdictStore,
        approver_policy: Arc<dyn GateApproverPolicy>,
        audit_sink: Arc<dyn GovernanceAudit>,
    ) -> GovernedRouter {
        let governed_run = GovernedRun::from_minting_principal(&principal);
        governed_run
            .validate()
            .expect("an internally minted run must have one coherent principal");
        GovernedRouter {
            minter,
            principal: governed_run,
            minting_principal: Some(principal),
            effect_api,
            state: RefCell::new(RunState::default()),
            verdicts: RefCell::new(verdicts),
            approver_policy,
            audit_sink,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_issued_run(
        minter: RunTokenMinter,
        principal: GovernedRun,
        run_token: RunToken,
        effective_grants: impl IntoIterator<Item = String>,
        effect_api: Box<dyn EffectApi>,
        verdicts: HitlVerdictStore,
        approver_policy: Arc<dyn GateApproverPolicy>,
        audit_sink: Arc<dyn GovernanceAudit>,
    ) -> Result<GovernedRouter, String> {
        principal.validate()?;
        if run_token.token.is_empty() || run_token.jti.is_empty() {
            return Err("issued governed run requires a non-empty bearer and token id".into());
        }
        let effective_grants = effective_grants.into_iter().collect::<BTreeSet<_>>();
        if effective_grants.is_empty()
            || effective_grants.iter().any(|grant| {
                grant.is_empty() || grant.len() > 255 || grant.contains(char::is_whitespace)
            })
        {
            return Err("issued governed run requires bounded effective grants".into());
        }
        Ok(GovernedRouter {
            minter,
            principal,
            minting_principal: None,
            effect_api,
            state: RefCell::new(RunState {
                token: Some(run_token),
                effective_grants,
                ..RunState::default()
            }),
            verdicts: RefCell::new(verdicts),
            approver_policy,
            audit_sink,
        })
    }

    pub fn minter(&self) -> &RunTokenMinter {
        &self.minter
    }

    pub fn principal(&self) -> &GovernedRun {
        &self.principal
    }

    pub fn current_token(&self) -> Option<RunToken> {
        self.state.borrow().token.clone()
    }

    pub fn is_fatal(&self) -> bool {
        self.state.borrow().fatal
    }

    pub fn teardown(&self, now: &Timestamp) {
        if let Some(token) = self.current_token() {
            self.minter.teardown(&self.principal.scope, &token, now);
        }
    }

    pub fn audit(&self) -> Vec<AuditEntry> {
        self.state.borrow().audit.clone()
    }

    pub fn permitted_tool_names(
        &self,
        registry: &ToolRegistry,
        now: &Timestamp,
    ) -> Result<BTreeSet<String>, String> {
        let token = self.ensure_run_token(now)?;
        if !self.minter.is_live(&self.principal.scope, &token, now) {
            return Err("run token is revoked or expired; tool discovery is denied".into());
        }

        let state = self.state.borrow();
        Ok(registry
            .tools()
            .iter()
            .filter(|tool| {
                tool.required_caps()
                    .into_iter()
                    .all(|cap| state.effective_grants.contains(cap))
            })
            .map(|tool| tool.name().to_string())
            .collect())
    }

    fn ensure_run_token(&self, now: &Timestamp) -> Result<RunToken, String> {
        if let Some(t) = self.state.borrow().token.clone() {
            return Ok(t);
        }
        let p = self
            .minting_principal
            .as_ref()
            .ok_or_else(|| "issued run token is unavailable".to_string())?;
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

    pub fn authorize_read(
        &self,
        tool: &RegisteredTool,
        now: &Timestamp,
    ) -> Result<ReadAuthorization, CallOutcome> {
        if tool.effect_kind() != myelin_agent::EffectKind::Read || tool.side_effecting() {
            return Err(CallOutcome::Denied {
                reason: format!(
                    "tool `{}` is not a non-side-effecting direct read",
                    tool.name()
                ),
                jti: "<unminted>".into(),
            });
        }
        self.authorize_declared_tool(tool, now)
            .map(|run_token| ReadAuthorization {
                run_token,
                tool: tool.name().to_string(),
                required_caps: tool
                    .required_caps()
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            })
            .map_err(|(reason, jti)| CallOutcome::Denied { reason, jti })
    }

    fn authorize_declared_tool(
        &self,
        tool: &RegisteredTool,
        now: &Timestamp,
    ) -> Result<RunToken, (String, String)> {
        let token = self
            .ensure_run_token(now)
            .map_err(|reason| (reason, "<unminted>".into()))?;
        let jti = token.jti.clone();
        if !self.minter.is_live(&self.principal.scope, &token, now) {
            return Err((
                "run token is revoked or expired (MR-011 durable revocation) - denied".into(),
                jti,
            ));
        }
        let missing = {
            let state = self.state.borrow();
            tool.required_caps()
                .into_iter()
                .find(|cap| !state.effective_grants.contains(*cap))
        };
        if let Some(cap) = missing {
            return Err((
                format!(
                    "tool `{}` requires capability `{cap}` outside the exact minted delegation intersection",
                    tool.name()
                ),
                jti,
            ));
        }
        Ok(token)
    }

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
        let token = match self.authorize_declared_tool(tool, now) {
            Ok(token) => token,
            Err((reason, jti)) => {
                return self.record(tool.name(), CallOutcome::Denied { reason, jti }, now)
            }
        };
        let jti = token.jti.clone();

        let mut approval_to_consume = None;
        if tool.requires_approval() {
            let effect_key = mcp_effect_key(tool.name(), args);
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
                Some(gid) => {
                    let verdict = self.verdicts.borrow().fetch(&self.principal.scope, gid);
                    match verdict {
                        Some(rec)
                            if rec.authorizes(
                                &effect_key,
                                &self.principal.run_id.0,
                                &self.principal.agent_id.0,
                            ) =>
                        {
                            approval_to_consume = Some(gid.to_string());
                        }
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
                        _ => {
                            return self.record(
                                tool.name(),
                                CallOutcome::Denied {
                                    reason: format!(
                                        "HITL approval not granted server-side for gate `{gid}` \
                                         on `{}` - the gate must be Approved in the verdict store \
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

pub fn run_ctx_for(jti: &str, principal_id: &str, tool: &str) -> RunCtx {
    RunCtx(format!("runtok:{jti}|principal:{principal_id}|tool:{tool}"))
}

pub fn proposed_effect_for(tool: &str, args: &serde_json::Value) -> ProposedEffect {
    ProposedEffect(format!("tool:{tool}|args:{args}"))
}

#[derive(Default)]
pub struct SkeletonEffectApi {
    calls: RefCell<Vec<(String, String)>>,
}

impl SkeletonEffectApi {
    pub fn new() -> SkeletonEffectApi {
        SkeletonEffectApi {
            calls: RefCell::new(Vec::new()),
        }
    }

    pub fn recorded(&self) -> Vec<(String, String)> {
        self.calls.borrow().clone()
    }
}

impl EffectApi for SkeletonEffectApi {
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        self.calls
            .borrow_mut()
            .push((run.0.clone(), effect.0.clone()));
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
