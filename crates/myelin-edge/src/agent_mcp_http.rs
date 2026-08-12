use std::sync::Arc;

use chrono::Utc;
use myelin_agent::McpApprovalContract;
use myelin_agent_service::hosted_run_contract::{
    hosted_agent_decision_ref, HostedAgentDecision, HOSTED_AGENT_APPROVAL_SIGNAL,
};
use myelin_events::{OutboxStore, Timestamp, UlidMinter};
use myelin_flow::{DurableExecutor, PgFlowExecutor, RunId as FlowRunId, SignalSpec};
use myelin_git::core::RepoLoc;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PrincipalStatus, RunId, RunToken};
use myelin_identity_service::mint::{RunTokenAuthorizer, RunTokenMinter};
use myelin_identity_service::{
    AgentRegistryError, AgentSessionError, AgentSessionIssuer, CredentialPurpose, PgAgentRegistry,
    PrincipalStore, EXTERNAL_MCP_RUNTIME, HOSTED_LUNA_RUNTIME,
};
use myelin_mcp::{
    approval_contract_from_effect_key, AuditPhase, GateApproverPolicy, GateAuditMinter,
    GovernanceAudit, GovernanceAuditRecord, GovernanceAuditTarget, GovernedRouter, GovernedRun,
    McpServer, OutboxGovernanceAudit, ToolRegistry, MAX_FRAME_BYTES,
};
use myelin_notif::pg_inbox::PgInboxStore;
use myelin_notif::{agent_effect_approval_targets, pending_agent_effect_approval};
use myelin_storage::hitl_gate_durable::{
    DurableHitlGateBacking, GateDecideError, GateDecisionOutcome, GateState, HitlVerdictStore,
};
use myelin_storage::{DurableAgentTriggerBacking, PgOutboxBacking, SubstrateProvider};
use serde_json::json;
use tokio::runtime::Handle;

use crate::catalogue::{Handler, HandlerCtx, Method};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::repo_authz::{RepoAuthorizer, RepoPermission};
use crate::request::EdgeResponse;
use crate::runtime::drive_edge_future;
use crate::{
    ChatEffectApi, DurableChatMutationApi, DurableChatReadApi, DurableCiReadApi, DurableGitBackend,
    DurableIssueMutationApi, DurableKnowledgeMutationApi, DurableKnowledgeReadApi, GitEffectApi,
    IssueEffectApi, KnowledgeEffectApi, McpReadExecutor, RoutedEffectApi,
};

#[derive(Clone)]
pub struct AgentMcpAuthority {
    registry: PgAgentRegistry,
    sessions: AgentSessionIssuer,
    run_tokens: RunTokenMinter,
    boundary: Arc<RunTokenAuthorizer>,
    principals: PrincipalStore,
    hosted_runs: DurableAgentTriggerBacking,
}

impl AgentMcpAuthority {
    pub fn new(
        registry: PgAgentRegistry,
        sessions: AgentSessionIssuer,
        run_tokens: RunTokenMinter,
        boundary: Arc<RunTokenAuthorizer>,
        principals: PrincipalStore,
        hosted_runs: DurableAgentTriggerBacking,
    ) -> Self {
        Self {
            registry,
            sessions,
            run_tokens,
            boundary,
            principals,
            hosted_runs,
        }
    }
}

#[derive(Clone)]
pub struct AgentMcpResources {
    git: Arc<DurableGitBackend>,
    ci: DurableCiReadApi,
    issues: DurableIssueMutationApi,
    knowledge: DurableKnowledgeReadApi,
    knowledge_mutations: DurableKnowledgeMutationApi,
    chat: DurableChatReadApi,
    chat_mutations: DurableChatMutationApi,
}

impl AgentMcpResources {
    pub fn new(
        git: Arc<DurableGitBackend>,
        ci: DurableCiReadApi,
        issues: DurableIssueMutationApi,
        knowledge: DurableKnowledgeMutationApi,
        chat: DurableChatReadApi,
        chat_mutations: DurableChatMutationApi,
    ) -> Self {
        Self {
            git,
            ci,
            issues,
            knowledge: knowledge.reads(),
            knowledge_mutations: knowledge,
            chat,
            chat_mutations,
        }
    }
}

#[derive(Clone)]
pub struct AgentMcpServices {
    authority: AgentMcpAuthority,
    provider: SubstrateProvider,
    resources: AgentMcpResources,
    outbox: OutboxStore,
    audit: Arc<OutboxGovernanceAudit>,
    gates: DurableHitlGateBacking,
    inbox: PgInboxStore,
    runtime: Handle,
}

impl AgentMcpServices {
    pub fn new(
        authority: AgentMcpAuthority,
        provider: SubstrateProvider,
        resources: AgentMcpResources,
        runtime: Handle,
    ) -> Self {
        let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
            provider.db_pool().clone(),
            runtime.clone(),
        )));
        let audit = Arc::new(OutboxGovernanceAudit::new(
            outbox.clone(),
            Arc::new(UlidMinter::new()),
        ));
        Self {
            authority,
            gates: DurableHitlGateBacking::new(provider.clone()),
            inbox: PgInboxStore::new(provider.db_pool().clone()),
            provider,
            resources,
            outbox,
            audit,
            runtime,
        }
    }

    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: std::future::Future<Output = T>,
    {
        drive_edge_future(&self.runtime, future, "agent MCP")
    }

    fn project_gate(
        &self,
        scope: &myelin_storage::TenantScope,
        gate_id: &str,
    ) -> Result<(), EdgeError> {
        let gate = self
            .drive(self.gates.fetch(scope, gate_id))?
            .map_err(|_| EdgeError::Unavailable("agent approval lookup is unavailable".into()))?
            .ok_or_else(|| EdgeError::Unavailable("agent approval gate disappeared".into()))?;
        if gate.state != GateState::Waiting {
            return Ok(());
        }
        for recipient in &gate.approver_filter {
            let item =
                pending_agent_effect_approval(scope.tenant(), scope.region(), recipient, &gate);
            self.drive(self.inbox.ensure(&item))?.map_err(|_| {
                EdgeError::Unavailable("agent approval inbox is unavailable".into())
            })?;
        }
        Ok(())
    }

    fn decide_gate(
        &self,
        ctx: &HandlerCtx<'_>,
        gate_id: &str,
        decision: HostedAgentDecision,
    ) -> Result<GateDecisionOutcome, EdgeError> {
        let state = match decision {
            HostedAgentDecision::Approved => GateState::Approved,
            HostedAgentDecision::Rejected => GateState::Rejected,
            HostedAgentDecision::Expired => {
                return Err(EdgeError::BadRequest(
                    "humans may approve or reject an agent effect".into(),
                ))
            }
        };
        let decided_at_unix = Utc::now().timestamp();
        let outcome = self
            .drive(self.gates.decide(
                ctx.scope,
                gate_id,
                state,
                &ctx.principal.principal_id.0,
                ctx.principal.kind.clone(),
                decided_at_unix,
            ))?
            .map_err(|_| EdgeError::Unavailable("agent approval decision is unavailable".into()))?
            .map_err(map_gate_decision_error)?;
        self.audit_gate_decision(ctx, &outcome.record, decision)?;
        self.wake_hosted_run(ctx, &outcome.record, decision)?;
        self.complete_gate_notifications(ctx.scope, &outcome.record)?;
        Ok(outcome)
    }

    fn complete_gate_notifications(
        &self,
        scope: &myelin_storage::TenantScope,
        gate: &myelin_storage::hitl_gate_durable::GateRecord,
    ) -> Result<(), EdgeError> {
        for target in agent_effect_approval_targets(scope.tenant(), scope.region(), gate) {
            self.drive(
                self.inbox
                    .complete_if_present(&target.scope, &target.item_id),
            )?
            .map_err(|_| EdgeError::Unavailable("agent approval inbox is unavailable".into()))?;
        }
        Ok(())
    }

    fn audit_gate_decision(
        &self,
        ctx: &HandlerCtx<'_>,
        gate: &myelin_storage::hitl_gate_durable::GateRecord,
        decision: HostedAgentDecision,
    ) -> Result<(), EdgeError> {
        let contract = approval_contract_from_effect_key(&gate.effect_id).ok_or_else(|| {
            EdgeError::Conflict("agent approval has no registered approval contract".into())
        })?;
        let phase = match decision {
            HostedAgentDecision::Approved => AuditPhase::Approved,
            HostedAgentDecision::Rejected => AuditPhase::Rejected,
            HostedAgentDecision::Expired => AuditPhase::Expired,
        };
        let timestamp = chrono::DateTime::from_timestamp(
            gate.decided_at_unix.unwrap_or(gate.expires_at_unix),
            0,
        )
        .ok_or_else(|| EdgeError::Internal("agent approval timestamp is invalid".into()))?
        .to_rfc3339();
        let audit = OutboxGovernanceAudit::new(
            self.outbox.clone(),
            Arc::new(GateAuditMinter::new(
                ctx.scope.tenant().as_str(),
                &gate.gate_id,
                phase,
            )),
        );
        audit
            .record(GovernanceAuditRecord {
                scope: ctx.scope,
                actor: ctx.principal,
                run_id: &RunId(gate.run_id.clone()),
                target: GovernanceAuditTarget::Gate(&gate.gate_id),
                tool: contract.tool(),
                jti: &format!("human-decision:{}", gate.gate_id),
                phase,
                outcome: None,
                now: &Timestamp(timestamp),
            })
            .map_err(|_| EdgeError::Unavailable("agent approval audit is unavailable".into()))
    }

    fn wake_hosted_run(
        &self,
        ctx: &HandlerCtx<'_>,
        gate: &myelin_storage::hitl_gate_durable::GateRecord,
        decision: HostedAgentDecision,
    ) -> Result<(), EdgeError> {
        let hosted = self
            .drive(
                self.authority
                    .hosted_runs
                    .started_for_run(&ctx.principal.tenant.0, &gate.run_id),
            )?
            .map_err(|_| EdgeError::Unavailable("hosted agent run lookup is unavailable".into()))?;
        if hosted.is_none() {
            return Ok(());
        }
        PgFlowExecutor::new(
            self.provider.db_pool().clone(),
            self.runtime.clone(),
            Arc::new(UlidMinter::new()),
            ctx.principal.tenant.clone(),
            ctx.principal.region.clone(),
        )
        .signal(SignalSpec {
            run: FlowRunId(gate.run_id.clone()),
            signal_name: HOSTED_AGENT_APPROVAL_SIGNAL.into(),
            idem_key: gate.gate_id.clone(),
            payload: vec![hosted_agent_decision_ref(
                &ctx.principal.tenant,
                &gate.run_id,
                &gate.gate_id,
                decision,
            )],
            payload_key_ref: None,
        })
        .map(|_| ())
        .map_err(|error| {
            eprintln!(
                "edge: failed to wake hosted agent run {} after gate {}: {error}",
                gate.run_id, gate.gate_id
            );
            EdgeError::Unavailable("hosted agent approval wake is unavailable".into())
        })
    }
}

struct AgentMcpHandler {
    services: AgentMcpServices,
}

struct AgentApprovalDecisionHandler {
    services: AgentMcpServices,
}

impl Handler for AgentApprovalDecisionHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if ctx.principal.kind != PrincipalKind::Human
            || ctx.principal.status != PrincipalStatus::Active
        {
            return Err(EdgeError::Forbidden(
                "agent effects require an active human approver".into(),
            ));
        }
        let gate_id = ctx
            .params
            .get("gate")
            .map(String::as_str)
            .filter(|gate_id| {
                !gate_id.is_empty()
                    && gate_id.len() <= 256
                    && gate_id.bytes().all(|byte| byte.is_ascii_graphic())
                    && !gate_id.contains('/')
            })
            .ok_or_else(|| EdgeError::BadRequest("invalid agent approval gate ID".into()))?;
        let _request_key = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        if ctx.request.body.len() > 256 {
            return Err(EdgeError::PayloadTooLarge(
                "agent approval decision body exceeds 256 bytes".into(),
            ));
        }
        let body: serde_json::Value =
            serde_json::from_slice(&ctx.request.body).map_err(|error| {
                EdgeError::BadRequest(format!("invalid agent approval body: {error}"))
            })?;
        let object = body.as_object().ok_or_else(|| {
            EdgeError::BadRequest("agent approval body must be a JSON object".into())
        })?;
        if object.len() != 1 {
            return Err(EdgeError::BadRequest(
                "agent approval body accepts only `decision`".into(),
            ));
        }
        let decision = match object.get("decision").and_then(serde_json::Value::as_str) {
            Some("approve") => HostedAgentDecision::Approved,
            Some("reject") => HostedAgentDecision::Rejected,
            _ => {
                return Err(EdgeError::BadRequest(
                    "agent approval decision must be `approve` or `reject`".into(),
                ))
            }
        };
        let outcome = self.services.decide_gate(ctx, gate_id, decision)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "gate_id": outcome.record.gate_id,
                "run_id": outcome.record.run_id,
                "state": outcome.record.state.as_str(),
                "changed": outcome.changed,
            }),
        )))
    }
}

impl Handler for AgentMcpHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_mcp_request(ctx)?;
        let run_id = run_param(ctx)?;
        let capability = ctx.identity.capability();
        match &capability.purpose {
            CredentialPurpose::AgentRun {
                run_id: signed_run_id,
                delegation_snapshot: Some(snapshot),
            } if signed_run_id == run_id && *snapshot > 0 => {}
            _ => return Err(EdgeError::NotFound("agent run not found".into())),
        }
        let bearer = ctx
            .request
            .bearer()
            .filter(|bearer| !bearer.is_empty())
            .ok_or_else(|| EdgeError::Unauthorized("agent run bearer is missing".into()))?;
        let authorized = self.authorize_run(ctx, run_id, &capability.jti)?;
        let registration = self
            .services
            .drive(
                self.services
                    .authority
                    .registry
                    .get(ctx.principal, authorized.agent_id()),
            )?
            .map_err(map_registry_error)?;
        validate_registration(ctx.principal, &registration, authorized.runtime_ref())?;
        let delegator = active_delegator(
            &self.services.authority.principals,
            ctx.scope,
            &PrincipalId(authorized.delegator_id(&registration).to_string()),
        )?;

        let registry = ToolRegistry::for_cursors(&registration.tools)
            .map_err(|error| EdgeError::Unavailable(error.to_string()))?;
        let run_token = RunToken {
            token: bearer.to_string(),
            jti: capability.jti.clone(),
        };
        let principal = GovernedRun {
            scope: ctx.scope.clone(),
            agent_id: ctx.principal.principal_id.clone(),
            agent: ctx.principal.clone(),
            run_id: RunId(run_id.to_string()),
        };
        let effect_api = Box::new(
            RoutedEffectApi::try_new([
                (
                    "git",
                    Box::new(GitEffectApi::new(
                        self.services.resources.git.clone(),
                        ctx.principal.tenant.0.clone(),
                        ctx.principal.region.0.clone(),
                        ctx.principal.clone(),
                        delegator.clone(),
                        self.services.authority.boundary.clone(),
                    )) as Box<dyn myelin_agent::EffectApi>,
                ),
                (
                    "chat",
                    Box::new(ChatEffectApi::new(
                        self.services.resources.chat_mutations.clone(),
                        ctx.principal.clone(),
                        delegator.clone(),
                        self.services.authority.boundary.clone(),
                    )) as Box<dyn myelin_agent::EffectApi>,
                ),
                (
                    "issues",
                    Box::new(IssueEffectApi::new(
                        self.services.resources.issues.clone(),
                        ctx.principal.clone(),
                        delegator.clone(),
                        self.services.authority.boundary.clone(),
                    )) as Box<dyn myelin_agent::EffectApi>,
                ),
                (
                    "knowledge",
                    Box::new(KnowledgeEffectApi::new(
                        self.services.resources.knowledge_mutations.clone(),
                        ctx.principal.clone(),
                        delegator.clone(),
                        self.services.authority.boundary.clone(),
                    )) as Box<dyn myelin_agent::EffectApi>,
                ),
            ])
            .map_err(EdgeError::Unavailable)?,
        );
        let approvers = Arc::new(CreatorApproverPolicy {
            creator_id: PrincipalId(authorized.delegator_id(&registration).to_string()),
            scope: ctx.scope.clone(),
            principals: self.services.authority.principals.clone(),
            repos: self.services.resources.git.repo_authorizer().clone(),
        });
        let router = GovernedRouter::with_issued_run(
            self.services.authority.run_tokens.clone(),
            principal,
            run_token,
            capability.effective_authority.grants().map(str::to_string),
            effect_api,
            HitlVerdictStore::with_pg(self.services.provider.clone()),
            approvers,
            self.services.audit.clone(),
        )
        .map_err(EdgeError::Unavailable)?;
        let reads = Arc::new(
            McpReadExecutor::new(
                self.services.resources.ci.clone(),
                self.services.authority.boundary.clone(),
                delegator,
            )
            .with_issues(self.services.resources.issues.reads())
            .with_knowledge(self.services.resources.knowledge.clone())
            .with_chat(self.services.resources.chat.clone())
            .with_git(self.services.resources.git.clone()),
        );
        let server = McpServer::with_router_and_reads(registry, router, reads);
        let frame = std::str::from_utf8(&ctx.request.body)
            .map_err(|_| EdgeError::BadRequest("MCP frame must be valid UTF-8".into()))?;
        let response = server.handle_line(frame);

        if authorized.is_external() && server.router().is_some_and(GovernedRouter::is_fatal) {
            self.services
                .drive(self.services.authority.sessions.terminate(
                    ctx.principal,
                    run_id,
                    &capability.jti,
                ))?
                .map_err(map_session_error)?;
        }

        match response {
            Some(response) => {
                if let Some(gate_id) = response_gate_id(&response) {
                    self.services.project_gate(ctx.scope, &gate_id)?;
                }
                Ok(no_store(EdgeResponse::Bytes {
                    status: 200,
                    content_type: "application/json".into(),
                    headers: Vec::new(),
                    body: response.into_bytes(),
                }))
            }
            None => Ok(no_store(EdgeResponse::Bytes {
                status: 204,
                content_type: "application/json".into(),
                headers: Vec::new(),
                body: Vec::new(),
            })),
        }
    }
}

fn response_gate_id(response: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(response)
        .ok()?
        .get("result")?
        .get("_meta")?
        .get("gateId")?
        .as_str()
        .map(str::to_string)
}

enum AuthorizedAgentRun {
    External {
        agent_id: String,
    },
    Hosted {
        agent_id: String,
        delegator_id: String,
    },
}

impl AuthorizedAgentRun {
    fn agent_id(&self) -> &str {
        match self {
            Self::External { agent_id } | Self::Hosted { agent_id, .. } => agent_id,
        }
    }

    fn delegator_id<'a>(
        &'a self,
        registration: &'a myelin_identity_service::AgentRegistration,
    ) -> &'a str {
        match self {
            Self::External { .. } => &registration.created_by,
            Self::Hosted { delegator_id, .. } => delegator_id,
        }
    }

    fn runtime_ref(&self) -> &str {
        match self {
            Self::External { .. } => EXTERNAL_MCP_RUNTIME,
            Self::Hosted { .. } => HOSTED_LUNA_RUNTIME,
        }
    }

    fn is_external(&self) -> bool {
        matches!(self, Self::External { .. })
    }
}

impl AgentMcpHandler {
    fn authorize_run(
        &self,
        ctx: &HandlerCtx<'_>,
        run_id: &str,
        token_jti: &str,
    ) -> Result<AuthorizedAgentRun, EdgeError> {
        match self
            .services
            .drive(self.services.authority.sessions.authorize(
                ctx.principal,
                run_id,
                token_jti,
                Utc::now(),
            ))? {
            Ok(run) => Ok(AuthorizedAgentRun::External {
                agent_id: run.agent_id,
            }),
            Err(AgentSessionError::NotFound | AgentSessionError::RunNotFound) => {
                let hosted = self
                    .services
                    .drive(
                        self.services
                            .authority
                            .hosted_runs
                            .started_for_run(&ctx.principal.tenant.0, run_id),
                    )?
                    .map_err(|_| {
                        EdgeError::Unavailable("hosted agent run lookup is unavailable".into())
                    })?
                    .ok_or_else(|| EdgeError::NotFound("agent run not found".into()))?;
                let expected_principal = format!("agent:{}", hosted.run_as_agent_id);
                if hosted.runtime_ref != HOSTED_LUNA_RUNTIME
                    || ctx.principal.principal_id.0 != expected_principal
                {
                    return Err(EdgeError::NotFound("agent run not found".into()));
                }
                Ok(AuthorizedAgentRun::Hosted {
                    agent_id: hosted.run_as_agent_id,
                    delegator_id: hosted.owner_principal_id,
                })
            }
            Err(error) => Err(map_session_error(error)),
        }
    }
}

fn active_delegator(
    principals: &PrincipalStore,
    scope: &myelin_storage::TenantScope,
    id: &PrincipalId,
) -> Result<Principal, EdgeError> {
    let row = principals
        .try_get_principal(scope, id)
        .map_err(|_| EdgeError::Unavailable("agent delegator lookup is unavailable".into()))?
        .ok_or_else(|| EdgeError::NotFound("agent run not found".into()))?;
    let principal = Principal::new(
        row.tenant,
        row.region,
        row.principal_id,
        row.kind,
        row.data_role,
        row.status,
    );
    if !matches!(&principal.kind, PrincipalKind::Human)
        || principal.status != PrincipalStatus::Active
    {
        return Err(EdgeError::NotFound("agent run not found".into()));
    }
    Ok(principal)
}

pub fn register_agent_mcp(builder: GatewayBuilder, services: AgentMcpServices) -> GatewayBuilder {
    builder
        .route(
            Method::Post,
            "/v1/agent-runs/{run}/mcp",
            "identity.agent.run.mcp",
            Arc::new(AgentMcpHandler {
                services: services.clone(),
            }),
        )
        .route(
            Method::Post,
            "/v1/agent-approvals/{gate}/decision",
            "identity.agent.approval.decide",
            Arc::new(AgentApprovalDecisionHandler { services }),
        )
}

struct CreatorApproverPolicy {
    creator_id: PrincipalId,
    scope: myelin_storage::TenantScope,
    principals: PrincipalStore,
    repos: Arc<dyn RepoAuthorizer>,
}

impl GateApproverPolicy for CreatorApproverPolicy {
    fn eligible_approvers(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<Vec<PrincipalId>, String> {
        let contract = McpApprovalContract::for_tool(tool)
            .ok_or_else(|| format!("tool `{tool}` has no registered Edge approval policy"))?;
        let repo = match contract {
            McpApprovalContract::GitMerge => args
                .get("repo")
                .and_then(serde_json::Value::as_str)
                .filter(|repo| !repo.is_empty() && repo.len() <= 255)
                .ok_or_else(|| "merge approval requires a bounded repository slug".to_string())?,
        };
        let row = self
            .principals
            .try_get_principal(&self.scope, &self.creator_id)
            .map_err(|error| format!("approver lookup unavailable: {error}"))?;
        let Some(row) = row else {
            return Ok(Vec::new());
        };
        let approver = Principal::new(
            row.tenant,
            row.region,
            row.principal_id.clone(),
            row.kind,
            row.data_role,
            row.status,
        );
        if !matches!(approver.kind, PrincipalKind::Human)
            || approver.status != PrincipalStatus::Active
            || !self.repos.authorize_repo_permission(
                &approver,
                &RepoLoc::new(&approver.tenant.0, &approver.region.0, repo),
                RepoPermission::ProtectedPush,
            )
        {
            return Ok(Vec::new());
        }
        Ok(vec![approver.principal_id])
    }
}

fn require_mcp_request(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if !ctx.request.query.is_empty() {
        return Err(EdgeError::BadRequest(
            "agent MCP accepts no query parameters".into(),
        ));
    }
    if ctx.request.body.len() > MAX_FRAME_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "MCP frame exceeds {MAX_FRAME_BYTES} bytes"
        )));
    }
    if ctx.request.body.is_empty() {
        return Err(EdgeError::BadRequest("MCP frame is empty".into()));
    }
    let content_type = ctx
        .request
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if content_type != Some("application/json") {
        return Err(EdgeError::BadRequest(
            "agent MCP requires `Content-Type: application/json`".into(),
        ));
    }
    Ok(())
}

fn run_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    ctx.params
        .get("run")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a run id".into()))
}

fn validate_registration(
    principal: &Principal,
    registration: &myelin_identity_service::AgentRegistration,
    expected_runtime: &str,
) -> Result<(), EdgeError> {
    if registration.principal_id != principal.principal_id.0
        || registration.runtime_ref != expected_runtime
        || registration.status != PrincipalStatus::Active
    {
        return Err(EdgeError::NotFound("agent run not found".into()));
    }
    Ok(())
}

fn map_registry_error(error: AgentRegistryError) -> EdgeError {
    match error {
        AgentRegistryError::BadInput(message) => EdgeError::BadRequest(message),
        AgentRegistryError::NotFound => EdgeError::NotFound("agent not found".into()),
        AgentRegistryError::Conflict(message) | AgentRegistryError::Policy(message) => {
            EdgeError::Conflict(message)
        }
        AgentRegistryError::Storage(message) => EdgeError::Internal(message),
    }
}

fn map_session_error(error: AgentSessionError) -> EdgeError {
    match error {
        AgentSessionError::BadInput(message) => EdgeError::BadRequest(message),
        AgentSessionError::NotFound | AgentSessionError::RunNotFound => {
            EdgeError::NotFound("agent run not found".into())
        }
        AgentSessionError::Conflict(message) => EdgeError::Conflict(message),
        AgentSessionError::Policy(_) => EdgeError::Forbidden("agent run was refused".into()),
        AgentSessionError::Expired => EdgeError::Conflict("agent run has expired".into()),
        AgentSessionError::Storage(message) => EdgeError::Internal(message),
    }
}

fn map_gate_decision_error(error: GateDecideError) -> EdgeError {
    match error {
        GateDecideError::NotFound => EdgeError::NotFound("agent approval not found".into()),
        GateDecideError::SelfApproval
        | GateDecideError::NotEligible
        | GateDecideError::MachineApproverRefused => {
            EdgeError::Forbidden("principal is not eligible to decide this agent effect".into())
        }
        GateDecideError::ApprovalWindowExpired => {
            EdgeError::Conflict("agent approval window has expired".into())
        }
        GateDecideError::AlreadyDecided(state) => {
            EdgeError::Conflict(format!("agent approval is already {}", state.as_str()))
        }
    }
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::EdgeRequest;
    use myelin_identity::{DataRole, PrincipalStatus};
    use myelin_identity_service::{Authority, CredentialAudience, VerifiedCapabilityContext};
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region, TenantId};
    use std::collections::BTreeMap;

    #[test]
    fn transport_accepts_one_bounded_json_frame_and_nothing_ambiguous() {
        let principal = Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("agent:00000000-0000-0000-0000-000000000001".into()),
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef(EXTERNAL_MCP_RUNTIME.into()),
                on_behalf_of: None,
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        let identity = myelin_identity_service::RequestIdentity {
            principal: principal.clone(),
            scope: scope.clone(),
            credential: myelin_identity_service::CredentialContext::Capability(
                VerifiedCapabilityContext {
                    purpose: CredentialPurpose::AgentRun {
                        run_id: "run".into(),
                        delegation_snapshot: Some(1),
                    },
                    audience: CredentialAudience::Edge,
                    jti: "run-jti".into(),
                    effective_authority: Authority::of(["run.view"]),
                    expires_at_unix: i64::MAX,
                    dpop: myelin_identity_service::DpopState::Unbound,
                },
            ),
        };
        let params = BTreeMap::from([("run".into(), "run".into())]);
        let page = crate::Page {
            limit: 50,
            cursor: None,
        };
        let request = EdgeRequest::new(
            "POST",
            "/v1/agent-runs/run/mcp",
            "",
            vec![(
                "content-type".into(),
                "application/json; charset=utf-8".into(),
            )],
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.to_vec(),
        );
        let ctx = HandlerCtx {
            identity: &identity,
            principal: &principal,
            scope: &scope,
            params: &params,
            page: &page,
            request: &request,
        };
        assert!(require_mcp_request(&ctx).is_ok());

        let oversized = EdgeRequest::new(
            "POST",
            "/v1/agent-runs/run/mcp",
            "",
            vec![("content-type".into(), "application/json".into())],
            vec![b'x'; MAX_FRAME_BYTES + 1],
        );
        let oversized_ctx = HandlerCtx {
            request: &oversized,
            ..ctx
        };
        assert!(matches!(
            require_mcp_request(&oversized_ctx),
            Err(EdgeError::PayloadTooLarge(_))
        ));
    }
}
