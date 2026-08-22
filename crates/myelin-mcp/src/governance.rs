use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;

use myelin_agent::{
    EffectApi, EffectApproval, EffectAuthority, EffectKind, EffectResult, McpApprovalContract,
    ProposedEffect, RunCtx,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EventDraft, EventType, IdMinter,
    OutboxStore, OutboxTx, Timestamp, Ulid, Visibility,
};
use myelin_identity::{Principal, PrincipalId, RunId, RunToken};
use myelin_identity_service::mint::{RunTokenMinter, REPOSITORY_SCOPE_GRANT_PREFIX};
use myelin_storage::hitl_gate_durable::{
    gate_ref_token, opaque_gate_id, GateConsumeError, GateDecideError, GateRecord, GateState,
    GateStoreUnavailable, HitlVerdictStore, DEFAULT_HITL_GATE_TTL_SECS,
};
use myelin_storage::{ContentHash, TenantScope};

use crate::registry::{RegisteredTool, ToolRegistry};

mod audit_taxonomy;

use audit_taxonomy::EffectAuditOutcome;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernedRun {
    pub scope: TenantScope,
    pub agent_id: PrincipalId,
    pub agent: Principal,
    pub run_id: RunId,
}

impl GovernedRun {
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

pub struct IssuedGovernedRun {
    principal: GovernedRun,
    token: RunToken,
    effective_grants: BTreeSet<String>,
}

impl IssuedGovernedRun {
    pub fn new(
        principal: GovernedRun,
        token: RunToken,
        effective_grants: impl IntoIterator<Item = String>,
    ) -> Result<Self, String> {
        principal.validate()?;
        if token.token.is_empty() || token.jti.is_empty() {
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
        Ok(Self {
            principal,
            token,
            effective_grants,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallOutcome {
    Applied {
        event_id: String,
        resource: Option<myelin_agent::EffectResource>,
        jti: String,
    },
    Gated {
        gate_id: String,
        jti: String,
    },
    Denied {
        reason: String,
        jti: String,
    },
    Indeterminate {
        reason: String,
        jti: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadAuthorization {
    run_token: RunToken,
    tool: String,
    required_caps: Vec<String>,
    resource_ref: Option<ArtifactRef>,
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

    pub fn resource_ref(&self) -> Option<&ArtifactRef> {
        self.resource_ref.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadRefusalCategory {
    Authorization,
    InvalidInput,
    NotFound,
    Unavailable,
}

impl ReadRefusalCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Authorization => "authorization",
            Self::InvalidInput => "invalid_request",
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadAuditOutcome {
    Succeeded,
    Refused(ReadRefusalCategory),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GovernanceAuditOutcome<'a> {
    Effect(&'a CallOutcome),
    Read(ReadAuditOutcome),
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

impl AuditPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Attempt => "attempted",
            Self::Outcome => "outcome",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

/// Produces a stable event identity for a retryable gate lifecycle audit.
pub struct GateAuditMinter(Ulid);

impl GateAuditMinter {
    pub fn new(tenant: &str, gate_id: &str, phase: AuditPhase) -> Self {
        let digest = blake3::hash(format!("{tenant}\0{gate_id}\0{}", phase.as_str()).as_bytes());
        Self(Ulid(digest.to_hex().as_str()[..26].to_ascii_uppercase()))
    }
}

impl IdMinter for GateAuditMinter {
    fn mint(&self) -> Ulid {
        self.0.clone()
    }
}

pub struct GovernanceAuditRecord<'a> {
    pub scope: &'a TenantScope,
    pub actor: &'a Principal,
    pub run_id: &'a RunId,
    pub target: GovernanceAuditTarget<'a>,
    pub tool: &'a str,
    pub jti: &'a str,
    pub phase: AuditPhase,
    pub outcome: Option<GovernanceAuditOutcome<'a>>,
    pub now: &'a Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GovernanceAuditTarget<'a> {
    Run,
    Gate(&'a str),
    Resource(&'a ArtifactRef),
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
            target,
            tool,
            jti,
            phase,
            outcome,
            now,
        } = record;
        let event_type = audit_event_type(tool, phase, outcome)?;
        let run_ref = format!("myelin://{}/agent/run/{}", scope.tenant().0, run_id.0);
        let (subject_ref, gate_ref, resource_ref) = match target {
            GovernanceAuditTarget::Run => (run_ref.clone(), None, None),
            GovernanceAuditTarget::Gate(gate_id) => {
                let gate_ref = format!("{run_ref}:hitl-gate:{}", gate_ref_token(gate_id));
                (gate_ref.clone(), Some(gate_ref), None)
            }
            GovernanceAuditTarget::Resource(resource_ref) => {
                (resource_ref.0.clone(), None, Some(resource_ref))
            }
        };
        let mut payload = serde_json::json!({
            "run_ref": run_ref,
            "token_ref": format!("jti:{jti}"),
            "tool": tool,
        });
        if let Some(gate_ref) = gate_ref {
            payload["gate_ref"] = serde_json::Value::String(gate_ref);
        }
        if let Some(resource_ref) = resource_ref {
            payload["resource_ref"] = serde_json::Value::String(resource_ref.0.clone());
        }
        if let Some(outcome) = outcome {
            payload["outcome"] = match outcome {
                GovernanceAuditOutcome::Effect(CallOutcome::Applied {
                    event_id, resource, ..
                }) => serde_json::json!({
                    "kind": "applied",
                    "event_id": event_id,
                    "resource_ref": resource.as_ref().map(|resource| &resource.artifact_ref.0),
                }),
                GovernanceAuditOutcome::Effect(CallOutcome::Gated { gate_id, .. }) => {
                    serde_json::json!({ "kind": "gated", "gate_id": gate_id })
                }
                GovernanceAuditOutcome::Effect(CallOutcome::Denied { reason, .. }) => {
                    serde_json::json!({
                        "kind": "denied",
                        "reason_category": denial_reason_category(reason),
                    })
                }
                GovernanceAuditOutcome::Effect(CallOutcome::Indeterminate { .. }) => {
                    serde_json::json!({ "kind": "indeterminate" })
                }
                GovernanceAuditOutcome::Read(ReadAuditOutcome::Succeeded) => {
                    serde_json::json!({ "kind": "succeeded" })
                }
                GovernanceAuditOutcome::Read(ReadAuditOutcome::Refused(category)) => {
                    serde_json::json!({
                        "kind": "denied",
                        "reason_category": category.as_str(),
                    })
                }
            };
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
        tx.commit_absorb().map_err(|error| error.0)
    }
}

fn denial_reason_category(reason: &str) -> &'static str {
    let reason = reason.to_ascii_lowercase();
    if reason.contains("not found") || reason.contains("unknown") {
        "not_found"
    } else if reason.contains("permission")
        || reason.contains("authoriz")
        || reason.contains("capab")
        || reason.contains("delegat")
        || reason.contains("credential")
        || reason.contains("revoked")
    {
        "authorization"
    } else if reason.contains("budget") || reason.contains("cost") {
        "budget"
    } else if reason.contains("invalid")
        || reason.contains("malformed")
        || reason.contains("missing")
        || reason.contains("required")
        || reason.contains("schema")
    {
        "invalid_request"
    } else if reason.contains("conflict") || reason.contains("blocked") {
        "conflict"
    } else {
        "effect_denied"
    }
}

fn audit_event_type(
    tool: &str,
    phase: AuditPhase,
    outcome: Option<GovernanceAuditOutcome<'_>>,
) -> Result<&'static str, String> {
    if GOVERNED_DIRECT_READ_TOOLS.contains(&tool) {
        return match (phase, outcome) {
            (AuditPhase::Attempt, None) => Ok(myelin_agent::events::AGENT_TOOL_READ_ATTEMPTED),
            (
                AuditPhase::Outcome,
                Some(GovernanceAuditOutcome::Read(ReadAuditOutcome::Succeeded)),
            ) => Ok(myelin_agent::events::AGENT_TOOL_READ_SUCCEEDED),
            (
                AuditPhase::Outcome,
                Some(GovernanceAuditOutcome::Read(ReadAuditOutcome::Refused(_))),
            ) => Ok(myelin_agent::events::AGENT_TOOL_READ_DENIED),
            _ => Err("invalid governed read audit phase/outcome pairing".into()),
        };
    }
    if matches!(
        phase,
        AuditPhase::Approved | AuditPhase::Rejected | AuditPhase::Expired
    ) {
        if outcome.is_some() {
            return Err("invalid governance audit phase/outcome pairing".into());
        }
        let contract = McpApprovalContract::for_tool(tool).ok_or_else(|| {
            "governance audit refused an unregistered approval contract".to_string()
        })?;
        return audit_taxonomy::approval_event_type(contract, phase);
    }
    let effect_outcome = match (phase, outcome) {
        (AuditPhase::Attempt, None) => EffectAuditOutcome::Attempted,
        (
            AuditPhase::Outcome,
            Some(GovernanceAuditOutcome::Effect(CallOutcome::Applied { .. })),
        ) => EffectAuditOutcome::Applied,
        (AuditPhase::Outcome, Some(GovernanceAuditOutcome::Effect(CallOutcome::Gated { .. }))) => {
            EffectAuditOutcome::Gated
        }
        (AuditPhase::Outcome, Some(GovernanceAuditOutcome::Effect(CallOutcome::Denied { .. }))) => {
            EffectAuditOutcome::Denied
        }
        (
            AuditPhase::Outcome,
            Some(GovernanceAuditOutcome::Effect(CallOutcome::Indeterminate { .. })),
        ) => EffectAuditOutcome::Indeterminate,
        _ => return Err("invalid governance audit phase/outcome pairing".into()),
    };
    audit_taxonomy::effect_event_type(tool, effect_outcome)
}

pub const GOVERNED_DIRECT_READ_TOOLS: &[&str] = &[
    "chat.list_conversations",
    "chat.read_messages",
    "ci.read_log",
    "ci.read_run",
    "git.list_repositories",
    "git.read_file",
    "git.search_code",
    "issues.list",
    "issues.view",
    "knowledge.list_pages",
    "knowledge.read_page",
    "projects.list",
    "workspace.read_file",
];

fn read_resource_ref(
    scope: &TenantScope,
    tool: &str,
    args: &serde_json::Value,
) -> Option<ArtifactRef> {
    if let Some((field, subsystem, resource_type)) = match tool {
        "issues.view" => Some(("issue_ref", "issue", "issue")),
        "knowledge.read_page" => Some(("page_ref", "knowledge", "page")),
        _ => None,
    } {
        if args.get(field).is_some() {
            return canonical_read_resource_arg(scope, args, field, subsystem, resource_type);
        }
    }
    let (subsystem, resource_type, id) = match tool {
        "ci.read_run" | "ci.read_log" => ("ci", "run", args.get("run_id")?.as_str()?),
        "git.read_file" => ("git", "repo", args.get("repo")?.as_str()?),
        "git.search_code" => ("git", "repo", args.get("repo")?.as_str()?),
        "issues.view" => ("issue", "issue", args.get("issue_id")?.as_str()?),
        "knowledge.read_page" => ("knowledge", "page", args.get("page_id")?.as_str()?),
        "chat.read_messages" => ("chat", "channel", args.get("conversation_id")?.as_str()?),
        _ => return None,
    };
    Some(ArtifactRef(format!(
        "myelin://{}/{subsystem}/{resource_type}/{id}",
        scope.tenant().0
    )))
}

fn canonical_read_resource_arg(
    scope: &TenantScope,
    args: &serde_json::Value,
    field: &str,
    subsystem: &str,
    resource_type: &str,
) -> Option<ArtifactRef> {
    let reference = args.get(field)?.as_str()?;
    let parsed = myelin_refs::parse_scoped(reference).ok()?;
    (parsed.tenant == *scope.tenant()
        && parsed.subsystem == subsystem
        && parsed.type_ == resource_type
        && parsed.sub.is_none())
    .then(|| ArtifactRef(reference.to_string()))
}

fn read_audit_target(resource_ref: Option<&ArtifactRef>) -> GovernanceAuditTarget<'_> {
    resource_ref.map_or(GovernanceAuditTarget::Run, GovernanceAuditTarget::Resource)
}

struct RunState {
    token: RunToken,
    effective_grants: std::collections::BTreeSet<String>,
    audit: Vec<AuditEntry>,
    fatal: bool,
}

enum ApprovalGrant {
    NotRequired,
    Consume { gate_id: String, effect_key: String },
    Replay,
}

struct CallContext<'a> {
    tool: &'a RegisteredTool,
    args: &'a serde_json::Value,
    idempotency_key: &'a str,
    now: &'a Timestamp,
    now_unix: i64,
    presented_gate_id: Option<&'a str>,
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
    effect_api: Box<dyn EffectApi>,
    state: RefCell<RunState>,
    verdicts: RefCell<HitlVerdictStore>,
    approver_policy: Arc<dyn GateApproverPolicy>,
    audit_sink: Arc<dyn GovernanceAudit>,
}

impl GovernedRouter {
    pub fn with_issued_run(
        minter: RunTokenMinter,
        issued: IssuedGovernedRun,
        effect_api: Box<dyn EffectApi>,
        verdicts: HitlVerdictStore,
        approver_policy: Arc<dyn GateApproverPolicy>,
        audit_sink: Arc<dyn GovernanceAudit>,
    ) -> GovernedRouter {
        let IssuedGovernedRun {
            principal,
            token,
            effective_grants,
        } = issued;
        GovernedRouter {
            minter,
            principal,
            effect_api,
            state: RefCell::new(RunState {
                token,
                effective_grants,
                audit: Vec::new(),
                fatal: false,
            }),
            verdicts: RefCell::new(verdicts),
            approver_policy,
            audit_sink,
        }
    }

    pub fn minter(&self) -> &RunTokenMinter {
        &self.minter
    }

    pub fn principal(&self) -> &GovernedRun {
        &self.principal
    }

    pub fn current_token(&self) -> RunToken {
        self.state.borrow().token.clone()
    }

    pub fn is_fatal(&self) -> bool {
        self.state.borrow().fatal
    }

    pub fn teardown(&self, now: &Timestamp) {
        self.minter
            .teardown(&self.principal.scope, &self.current_token(), now);
    }

    pub fn audit(&self) -> Vec<AuditEntry> {
        self.state.borrow().audit.clone()
    }

    pub fn permitted_tool_names(
        &self,
        registry: &ToolRegistry,
        now: &Timestamp,
    ) -> Result<BTreeSet<String>, String> {
        let token = self.current_token();
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

    pub fn authorize_read(
        &self,
        tool: &RegisteredTool,
        args: &serde_json::Value,
        now: &Timestamp,
    ) -> Result<ReadAuthorization, CallOutcome> {
        let resource_ref = read_resource_ref(&self.principal.scope, tool.name(), args);
        if tool.effect_kind() != EffectKind::Read
            || tool.side_effecting()
            || !GOVERNED_DIRECT_READ_TOOLS.contains(&tool.name())
        {
            return Err(self.push_local_audit(
                tool.name(),
                CallOutcome::Denied {
                    reason: format!(
                        "tool `{}` is not a registered non-side-effecting direct read",
                        tool.name()
                    ),
                    jti: self.current_token().jti.clone(),
                },
            ));
        }
        let run_token = match self.authorize_declared_tool(tool, now) {
            Ok(token) => token,
            Err((reason, jti)) => {
                return Err(self.record_read_refusal(
                    tool.name(),
                    resource_ref.as_ref(),
                    CallOutcome::Denied { reason, jti },
                    ReadRefusalCategory::Authorization,
                    now,
                ))
            }
        };
        if let Err(reason) = self.authorize_resource_scope(tool, args) {
            return Err(self.record_read_refusal(
                tool.name(),
                resource_ref.as_ref(),
                CallOutcome::Denied {
                    reason,
                    jti: run_token.jti.clone(),
                },
                ReadRefusalCategory::Authorization,
                now,
            ));
        }
        if self
            .audit_sink
            .record(GovernanceAuditRecord {
                scope: &self.principal.scope,
                actor: &self.principal.agent,
                run_id: &self.principal.run_id,
                target: read_audit_target(resource_ref.as_ref()),
                tool: tool.name(),
                jti: &run_token.jti,
                phase: AuditPhase::Attempt,
                outcome: None,
                now,
            })
            .is_err()
        {
            return Err(self.push_local_audit(
                tool.name(),
                CallOutcome::Denied {
                    reason: "durable pre-read audit is unavailable; read withheld".into(),
                    jti: run_token.jti.clone(),
                },
            ));
        }
        Ok(ReadAuthorization {
            run_token,
            tool: tool.name().to_string(),
            required_caps: tool
                .required_caps()
                .into_iter()
                .map(str::to_string)
                .collect(),
            resource_ref,
        })
    }

    pub fn complete_read(
        &self,
        authorization: &ReadAuthorization,
        outcome: ReadAuditOutcome,
        now: &Timestamp,
    ) -> Result<(), String> {
        self.audit_sink.record(GovernanceAuditRecord {
            scope: &self.principal.scope,
            actor: &self.principal.agent,
            run_id: &self.principal.run_id,
            target: read_audit_target(authorization.resource_ref()),
            tool: authorization.tool(),
            jti: authorization.jti(),
            phase: AuditPhase::Outcome,
            outcome: Some(GovernanceAuditOutcome::Read(outcome)),
            now,
        })
    }

    fn authorize_declared_tool(
        &self,
        tool: &RegisteredTool,
        now: &Timestamp,
    ) -> Result<RunToken, (String, String)> {
        let token = self.current_token();
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

    fn authorize_resource_scope(
        &self,
        tool: &RegisteredTool,
        args: &serde_json::Value,
    ) -> Result<(), String> {
        if !tool.name().starts_with("git.") {
            return Ok(());
        }
        let state = self.state.borrow();
        let scopes = state
            .effective_grants
            .iter()
            .filter_map(|grant| grant.strip_prefix(REPOSITORY_SCOPE_GRANT_PREFIX))
            .collect::<BTreeSet<_>>();
        if scopes.is_empty() {
            return Ok(());
        }
        let Some(repository) = args.get("repo").and_then(serde_json::Value::as_str) else {
            return Err(format!(
                "tool `{}` needs an exact repository under this automation's delegation scope",
                tool.name()
            ));
        };
        if scopes.contains(repository) {
            Ok(())
        } else {
            Err(format!(
                "repository `{repository}` is outside the signed delegation scope"
            ))
        }
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
                        jti: self.current_token().jti.clone(),
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
        if let Err(reason) = self.authorize_resource_scope(tool, args) {
            return self.record(tool.name(), CallOutcome::Denied { reason, jti }, now);
        }
        let context = CallContext {
            tool,
            args,
            idempotency_key,
            now,
            now_unix,
            presented_gate_id,
        };
        let approval_grant = match self.approval_grant_for_call(&context, &jti) {
            Ok(grant) => grant,
            Err(outcome) => return outcome,
        };
        self.apply_with_approval(&context, token, approval_grant)
    }

    fn approval_grant_for_call(
        &self,
        context: &CallContext<'_>,
        jti: &str,
    ) -> Result<ApprovalGrant, CallOutcome> {
        let CallContext {
            tool,
            args,
            idempotency_key,
            now,
            now_unix,
            presented_gate_id,
        } = context;
        if !tool.requires_approval() {
            return Ok(ApprovalGrant::NotRequired);
        }
        let effect_key = mcp_effect_key_for_call(tool.name(), args, idempotency_key);
        let expired = self
            .verdicts
            .borrow_mut()
            .expire_due_for_effect(
                &self.principal.scope,
                &self.principal.run_id.0,
                &self.principal.agent_id.0,
                &effect_key,
                *now_unix,
            )
            .map_err(|_| {
                self.push_local_audit(
                    tool.name(),
                    CallOutcome::Indeterminate {
                        reason: "durable HITL state is unavailable; effect withheld".into(),
                        jti: jti.to_string(),
                    },
                )
            })?;
        if !expired.is_empty() && self.record_expired_gates(&expired, now).is_err() {
            self.state.borrow_mut().fatal = true;
            return Err(self.push_local_audit(
                tool.name(),
                CallOutcome::Indeterminate {
                    reason:
                        "HITL expiry committed but its governance audit did not; session terminated"
                            .into(),
                    jti: jti.to_string(),
                },
            ));
        }
        match *presented_gate_id {
            Some(gate_id) => {
                self.approval_grant_from_presented_gate(tool, &effect_key, gate_id, now, jti)
            }
            None => self.approval_grant_without_presented_gate(
                tool,
                args,
                &effect_key,
                now,
                *now_unix,
                jti,
            ),
        }
    }

    fn approval_grant_without_presented_gate(
        &self,
        tool: &RegisteredTool,
        args: &serde_json::Value,
        effect_key: &str,
        now: &Timestamp,
        now_unix: i64,
        jti: &str,
    ) -> Result<ApprovalGrant, CallOutcome> {
        let approved = self
            .verdicts
            .borrow()
            .find_approved(
                &self.principal.scope,
                &self.principal.run_id.0,
                &self.principal.agent_id.0,
                effect_key,
            )
            .map_err(|_| {
                self.push_local_audit(
                    tool.name(),
                    CallOutcome::Indeterminate {
                        reason: "durable HITL state is unavailable; effect withheld".into(),
                        jti: jti.to_string(),
                    },
                )
            })?;
        if let Some(record) = approved {
            if !record.authorizes(
                effect_key,
                &self.principal.run_id.0,
                &self.principal.agent_id.0,
            ) {
                return Err(self.record(
                    tool.name(),
                    CallOutcome::Denied {
                        reason: "stored HITL approval failed its exact effect binding".into(),
                        jti: jti.to_string(),
                    },
                    now,
                ));
            }
            return Ok(ApprovalGrant::Consume {
                gate_id: record.gate_id,
                effect_key: effect_key.to_string(),
            });
        }
        if self
            .audit_sink
            .record(GovernanceAuditRecord {
                scope: &self.principal.scope,
                actor: &self.principal.agent,
                run_id: &self.principal.run_id,
                target: GovernanceAuditTarget::Run,
                tool: tool.name(),
                jti,
                phase: AuditPhase::Attempt,
                outcome: None,
                now,
            })
            .is_err()
        {
            return Err(self.push_local_audit(
                tool.name(),
                CallOutcome::Denied {
                    reason: "durable pre-gate audit is unavailable; effect withheld".into(),
                    jti: jti.to_string(),
                },
            ));
        }
        let (gate_id, opened) = self
            .open_or_resurface_gate(tool.name(), args, effect_key, now_unix)
            .map_err(|reason| {
                self.record(
                    tool.name(),
                    CallOutcome::Denied {
                        reason,
                        jti: jti.to_string(),
                    },
                    now,
                )
            })?;
        let outcome = CallOutcome::Gated {
            gate_id,
            jti: jti.to_string(),
        };
        Err(if opened {
            self.record_after_mutation(tool.name(), outcome, now)
        } else {
            self.record(tool.name(), outcome, now)
        })
    }

    fn approval_grant_from_presented_gate(
        &self,
        tool: &RegisteredTool,
        effect_key: &str,
        gate_id: &str,
        now: &Timestamp,
        jti: &str,
    ) -> Result<ApprovalGrant, CallOutcome> {
        let verdict = self.verdicts.borrow().fetch(&self.principal.scope, gate_id);
        match verdict {
            Ok(Some(record))
                if record.authorizes(
                    effect_key,
                    &self.principal.run_id.0,
                    &self.principal.agent_id.0,
                ) =>
            {
                Ok(ApprovalGrant::Consume {
                    gate_id: gate_id.to_string(),
                    effect_key: effect_key.to_string(),
                })
            }
            Ok(Some(record))
                if record.authorizes_replay(
                    effect_key,
                    &self.principal.run_id.0,
                    &self.principal.agent_id.0,
                ) =>
            {
                Ok(ApprovalGrant::Replay)
            }
            Ok(Some(record))
                if record.state == GateState::Waiting
                    && record.effect_id == effect_key
                    && record.run_id == self.principal.run_id.0
                    && record.requested_by == self.principal.agent_id.0 =>
            {
                Err(self.record(
                    tool.name(),
                    CallOutcome::Gated {
                        gate_id: gate_id.to_string(),
                        jti: jti.to_string(),
                    },
                    now,
                ))
            }
            Err(_) => Err(self.push_local_audit(
                tool.name(),
                CallOutcome::Indeterminate {
                    reason: "durable HITL state is unavailable; effect withheld".into(),
                    jti: jti.to_string(),
                },
            )),
            Ok(None) | Ok(Some(_)) => Err(self.record(
                tool.name(),
                CallOutcome::Denied {
                    reason: format!(
                        "HITL approval not granted server-side for gate `{gate_id}` on `{}` - the \
                         gate must be Approved in the verdict store for this exact effect by a \
                         distinct human principal; a caller-supplied approval is never trusted \
                         (R2.4)",
                        tool.name()
                    ),
                    jti: jti.to_string(),
                },
                now,
            )),
        }
    }

    fn apply_with_approval(
        &self,
        context: &CallContext<'_>,
        token: RunToken,
        approval_grant: ApprovalGrant,
    ) -> CallOutcome {
        let CallContext {
            tool,
            args,
            idempotency_key,
            now,
            now_unix,
            ..
        } = context;
        let jti = token.jti.clone();
        if self
            .audit_sink
            .record(GovernanceAuditRecord {
                scope: &self.principal.scope,
                actor: &self.principal.agent,
                run_id: &self.principal.run_id,
                target: GovernanceAuditTarget::Run,
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
        let approval = match approval_grant {
            ApprovalGrant::Consume {
                gate_id,
                effect_key,
            } => {
                if let Err(error) = self.verdicts.borrow_mut().consume_approval(
                    &self.principal.scope,
                    &gate_id,
                    &effect_key,
                    &self.principal.run_id.0,
                    &self.principal.agent_id.0,
                    *now_unix,
                ) {
                    let outcome = if error == GateConsumeError::StorageUnavailable {
                        CallOutcome::Indeterminate {
                            reason: "durable HITL state is unavailable; effect withheld".into(),
                            jti,
                        }
                    } else {
                        CallOutcome::Denied {
                            reason: format!("HITL approval consumption refused: {error}"),
                            jti,
                        }
                    };
                    return self.record(tool.name(), outcome, now);
                }
                EffectApproval::HumanApproved
            }
            ApprovalGrant::Replay => EffectApproval::HumanApproved,
            ApprovalGrant::NotRequired => EffectApproval::NotRequired,
        };
        let run_ctx = run_ctx_for(&jti, &self.principal.agent_id.0, tool.name());
        let effect = proposed_effect_for(tool.name(), args);
        let authority = EffectAuthority {
            run_token: token,
            principal_id: self.principal.agent_id.clone(),
            tool: tool.name().to_string(),
            idempotency_key: idempotency_key.to_string(),
            approval,
        };
        let outcome = match self
            .effect_api
            .apply_authorized(&run_ctx, &authority, effect)
        {
            EffectResult::Applied(ev) => CallOutcome::Applied {
                event_id: ev.0,
                resource: None,
                jti,
            },
            EffectResult::AppliedResource { event_id, resource } => CallOutcome::Applied {
                event_id: event_id.0,
                resource: Some(resource),
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
        if let Some(existing) = verdicts
            .find_waiting(
                &self.principal.scope,
                &self.principal.run_id.0,
                &requested_by,
                effect_key,
            )
            .map_err(|_| "durable HITL state is unavailable; effect withheld".to_string())?
        {
            return Ok((existing.gate_id, false));
        }
        let gate_id = opaque_gate_id();
        let card_ref = approval_card_ref(&self.principal.scope, tool, args);
        let risk_summary = approval_risk_summary(tool, args);
        let record = GateRecord {
            gate_id: gate_id.clone(),
            run_id: self.principal.run_id.0.clone(),
            effect_id: effect_key.to_string(),
            risk_summary,
            cost_estimate: 0,
            approver_filter: approvers,
            state: GateState::Waiting,
            card_ref,
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

    pub fn gate_verdict(&self, gate_id: &str) -> Result<Option<GateRecord>, GateStoreUnavailable> {
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
            let contract = approval_contract_from_effect_key(&gate.effect_id)
                .ok_or_else(|| "expired gate has no registered approval contract".to_string())?;
            let run_id = RunId(gate.run_id.clone());
            self.audit_sink.record(GovernanceAuditRecord {
                scope: &self.principal.scope,
                actor: &actor,
                run_id: &run_id,
                target: GovernanceAuditTarget::Gate(&gate.gate_id),
                tool: contract.tool(),
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
            target: GovernanceAuditTarget::Run,
            tool,
            jti: outcome.jti(),
            phase: AuditPhase::Outcome,
            outcome: Some(GovernanceAuditOutcome::Effect(&outcome)),
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

    fn record_read_refusal(
        &self,
        tool: &str,
        resource_ref: Option<&ArtifactRef>,
        outcome: CallOutcome,
        category: ReadRefusalCategory,
        now: &Timestamp,
    ) -> CallOutcome {
        let recorded = match self.audit_sink.record(GovernanceAuditRecord {
            scope: &self.principal.scope,
            actor: &self.principal.agent,
            run_id: &self.principal.run_id,
            target: read_audit_target(resource_ref),
            tool,
            jti: outcome.jti(),
            phase: AuditPhase::Outcome,
            outcome: Some(GovernanceAuditOutcome::Read(ReadAuditOutcome::Refused(
                category,
            ))),
            now,
        }) {
            Ok(()) => outcome,
            Err(_) => CallOutcome::Denied {
                reason: "durable governance audit is unavailable; read denied".into(),
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
            target: GovernanceAuditTarget::Run,
            tool,
            jti: outcome.jti(),
            phase: AuditPhase::Outcome,
            outcome: Some(GovernanceAuditOutcome::Effect(&outcome)),
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

fn approval_card_ref(scope: &TenantScope, tool: &str, args: &serde_json::Value) -> Option<String> {
    match McpApprovalContract::for_tool(tool)? {
        McpApprovalContract::GitMerge => Some(format!(
            "myelin://{}/git/pr/{}:{}",
            scope.tenant().0,
            args.get("repo")?.as_str()?,
            args.get("number")?.as_u64()?,
        )),
        McpApprovalContract::IssuesClose => {
            canonical_issue_ref_arg(scope, args).map(str::to_string)
        }
    }
}

fn approval_risk_summary(tool: &str, args: &serde_json::Value) -> Vec<u8> {
    let Some(contract) = McpApprovalContract::for_tool(tool) else {
        return format!("Apply governed tool {tool}").into_bytes();
    };
    match contract {
        McpApprovalContract::GitMerge => match (
            args.get("repo").and_then(serde_json::Value::as_str),
            args.get("number").and_then(serde_json::Value::as_u64),
        ) {
            (Some(repo), Some(number)) => {
                format!("Merge pull request {repo}#{number}").into_bytes()
            }
            _ => b"Merge a pull request".to_vec(),
        },
        McpApprovalContract::IssuesClose => canonical_issue_ref_arg_for_any_tenant(args)
            .and_then(|reference| myelin_refs::parse_scoped(reference).ok())
            .map_or_else(
                || b"Close an issue".to_vec(),
                |parsed| format!("Close issue {}", parsed.id).into_bytes(),
            ),
    }
}

fn canonical_issue_ref_arg<'a>(
    scope: &TenantScope,
    args: &'a serde_json::Value,
) -> Option<&'a str> {
    let reference = canonical_issue_ref_arg_for_any_tenant(args)?;
    let parsed = myelin_refs::parse_scoped(reference).ok()?;
    (parsed.tenant == *scope.tenant()).then_some(reference)
}

fn canonical_issue_ref_arg_for_any_tenant(args: &serde_json::Value) -> Option<&str> {
    let reference = args.get("issue_ref")?.as_str()?;
    let parsed = myelin_refs::parse_scoped(reference).ok()?;
    (parsed.subsystem == "issue" && parsed.type_ == "issue" && parsed.sub.is_none())
        .then_some(reference)
}

fn timestamp_to_unix(timestamp: &Timestamp) -> Result<i64, String> {
    chrono::DateTime::parse_from_rfc3339(&timestamp.0)
        .map(|value| value.timestamp())
        .map_err(|_| "internal governance clock did not produce a valid RFC-3339 timestamp".into())
}

pub fn mcp_effect_key(tool: &str, args: &serde_json::Value) -> String {
    mcp_effect_key_from_material(tool, args, None)
}

pub fn mcp_effect_key_for_call(
    tool: &str,
    args: &serde_json::Value,
    idempotency_key: &str,
) -> String {
    mcp_effect_key_from_material(tool, args, Some(idempotency_key))
}

fn mcp_effect_key_from_material(
    tool: &str,
    args: &serde_json::Value,
    idempotency_key: Option<&str>,
) -> String {
    let canonical = canonical_json(args).to_string();
    let mut bound = Vec::with_capacity(
        tool.len() + 1 + canonical.len() + idempotency_key.map_or(0, str::len) + 1,
    );
    bound.extend_from_slice(tool.as_bytes());
    bound.push(0);
    bound.extend_from_slice(canonical.as_bytes());
    if let Some(idempotency_key) = idempotency_key {
        bound.push(0);
        bound.extend_from_slice(idempotency_key.as_bytes());
    }
    let digest = ContentHash::blake3(&bound).to_multihash_string();
    if tool == "git.merge" {
        if let Some(repo) = args
            .get("repo")
            .and_then(serde_json::Value::as_str)
            .filter(|repo| myelin_git::coordinate::RepositorySlug::parse(repo).is_ok())
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
    myelin_git::coordinate::RepositorySlug::parse(&repo).ok()?;
    Some(repo)
}

pub fn approval_contract_from_effect_key(effect_key: &str) -> Option<McpApprovalContract> {
    let tool = effect_key.strip_prefix("mcp:v1:")?.split_once(':')?.0;
    let contract = McpApprovalContract::for_tool(tool)?;
    match contract {
        McpApprovalContract::GitMerge => {
            git_merge_repo_from_effect_key(effect_key).map(|_| contract)
        }
        McpApprovalContract::IssuesClose => {
            valid_digest_effect_key(effect_key, contract.tool()).then_some(contract)
        }
    }
}

fn valid_digest_effect_key(effect_key: &str, tool: &str) -> bool {
    let Some(digest) = effect_key.strip_prefix(&format!("mcp:v1:{tool}:blake3:")) else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

#[cfg(test)]
mod security_tests {
    use myelin_events::IdMinter;

    use super::{
        approval_card_ref, approval_contract_from_effect_key, approval_risk_summary,
        git_merge_repo_from_effect_key, mcp_effect_key, mcp_effect_key_for_call, read_resource_ref,
        AuditPhase, GateAuditMinter,
    };
    use myelin_agent::McpApprovalContract;

    #[test]
    fn gate_lifecycle_audits_have_retry_stable_but_exact_event_identities() {
        let approved = GateAuditMinter::new("acme", "gate:one", AuditPhase::Approved).mint();
        assert_eq!(
            approved,
            GateAuditMinter::new("acme", "gate:one", AuditPhase::Approved).mint()
        );
        assert_ne!(
            approved,
            GateAuditMinter::new("acme", "gate:one", AuditPhase::Expired).mint()
        );
        assert_ne!(
            approved,
            GateAuditMinter::new("acme", "gate:two", AuditPhase::Approved).mint()
        );
    }

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
        assert_eq!(
            approval_contract_from_effect_key(&first),
            Some(McpApprovalContract::GitMerge)
        );
        assert_eq!(
            mcp_effect_key_for_call(
                "git.merge",
                &serde_json::json!({"repo":"team/alpha","number":7}),
                "merge-7",
            ),
            mcp_effect_key_for_call(
                "git.merge",
                &serde_json::json!({"number":7,"repo":"team/alpha"}),
                "merge-7",
            ),
        );
        assert_ne!(
            mcp_effect_key_for_call(
                "git.merge",
                &serde_json::json!({"repo":"team/alpha","number":7}),
                "merge-7",
            ),
            mcp_effect_key_for_call(
                "git.merge",
                &serde_json::json!({"repo":"team/alpha","number":7}),
                "merge-7-again",
            ),
            "approval identity distinguishes a retry from a new logical mutation"
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
        let bare_ancestor = mcp_effect_key(
            "git.merge",
            &serde_json::json!({"repo":"platform.git/api","number":1}),
        );
        assert!(git_merge_repo_from_effect_key(&bare_ancestor).is_none());
        assert!(git_merge_repo_from_effect_key(
            "mcp:v1:git.merge:repohex:zz:blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        )
        .is_none());
        assert!(approval_contract_from_effect_key(&traversal).is_none());
        assert!(approval_contract_from_effect_key(&mcp_effect_key(
            "knowledge.publish",
            &serde_json::json!({"page":"roadmap"}),
        ))
        .is_none());
    }

    #[test]
    fn issue_close_gate_identity_and_card_bind_the_canonical_reference() {
        let args = serde_json::json!({
            "issue_ref": "myelin://acme/issue/issue/ENG-41",
        });
        let effect_key = mcp_effect_key("issues.close", &args);
        assert_eq!(
            approval_contract_from_effect_key(&effect_key),
            Some(McpApprovalContract::IssuesClose)
        );
        let principal = myelin_identity::Principal::stub(
            myelin_identity::PrincipalId("agent:closer".into()),
            myelin_identity::PrincipalKind::Service,
            myelin_tenancy::TenantId::from_token("acme"),
        );
        let scope =
            myelin_storage::TenantScope::from_verified_token(&principal, principal.region.clone());
        assert_eq!(
            approval_card_ref(&scope, "issues.close", &args).as_deref(),
            Some("myelin://acme/issue/issue/ENG-41")
        );
        assert_eq!(
            approval_risk_summary("issues.close", &args),
            b"Close issue ENG-41"
        );

        let foreign = serde_json::json!({
            "issue_ref": "myelin://other/issue/issue/ENG-41",
        });
        assert!(approval_card_ref(&scope, "issues.close", &foreign).is_none());
        assert!(approval_contract_from_effect_key(
            "mcp:v1:issues.close:blake3:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        )
        .is_none());
    }

    #[test]
    fn governed_reads_audit_the_resource_identity_the_agent_received() {
        let principal = myelin_identity::Principal::stub(
            myelin_identity::PrincipalId("agent:reader".into()),
            myelin_identity::PrincipalKind::Service,
            myelin_tenancy::TenantId::from_token("acme"),
        );
        let scope =
            myelin_storage::TenantScope::from_verified_token(&principal, principal.region.clone());

        let issue_ref = "myelin://acme/issue/issue/ENG-41";
        assert_eq!(
            read_resource_ref(
                &scope,
                "issues.view",
                &serde_json::json!({"issue_ref": issue_ref})
            ),
            Some(myelin_tenancy::ArtifactRef(issue_ref.into()))
        );
        let page_ref = "myelin://acme/knowledge/page/01J00000000000000000000000";
        assert_eq!(
            read_resource_ref(
                &scope,
                "knowledge.read_page",
                &serde_json::json!({"page_ref": page_ref})
            ),
            Some(myelin_tenancy::ArtifactRef(page_ref.into()))
        );
        assert_eq!(
            read_resource_ref(
                &scope,
                "knowledge.read_page",
                &serde_json::json!({
                    "page_ref": "myelin://other/knowledge/page/01J00000000000000000000000"
                })
            ),
            None,
            "a foreign reference cannot become a local audit subject"
        );
    }
}
