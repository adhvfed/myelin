use std::future::Future;
use std::sync::Arc;

use chrono::Utc;
use myelin_events::{OutboxStore, UlidMinter};
use myelin_git::core::RepoLoc;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PrincipalStatus, RunId, RunToken};
use myelin_identity_service::mint::{RunTokenAuthorizer, RunTokenMinter};
use myelin_identity_service::{
    AgentRegistryError, AgentSessionError, AgentSessionIssuer, CredentialPurpose, PgAgentRegistry,
    PrincipalStore, EXTERNAL_MCP_RUNTIME,
};
use myelin_mcp::{
    GateApproverPolicy, GovernedRouter, GovernedRun, McpServer, OutboxGovernanceAudit,
    ToolRegistry, MAX_FRAME_BYTES,
};
use myelin_storage::hitl_gate_durable::HitlVerdictStore;
use myelin_storage::{PgOutboxBacking, SubstrateProvider};
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::catalogue::{Handler, HandlerCtx, Method};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::repo_authz::{RepoAuthorizer, RepoPermission};
use crate::request::EdgeResponse;
use crate::{
    DurableChatReadApi, DurableCiReadApi, DurableGitBackend, DurableIssueReadApi,
    DurableKnowledgeReadApi, GitEffectApi, McpReadExecutor,
};

#[derive(Clone)]
pub struct AgentMcpAuthority {
    registry: PgAgentRegistry,
    sessions: AgentSessionIssuer,
    run_tokens: RunTokenMinter,
    boundary: Arc<RunTokenAuthorizer>,
    principals: PrincipalStore,
}

impl AgentMcpAuthority {
    pub fn new(
        registry: PgAgentRegistry,
        sessions: AgentSessionIssuer,
        run_tokens: RunTokenMinter,
        boundary: Arc<RunTokenAuthorizer>,
        principals: PrincipalStore,
    ) -> Self {
        Self {
            registry,
            sessions,
            run_tokens,
            boundary,
            principals,
        }
    }
}

#[derive(Clone)]
pub struct AgentMcpResources {
    git: Arc<DurableGitBackend>,
    ci: DurableCiReadApi,
    issues: DurableIssueReadApi,
    knowledge: DurableKnowledgeReadApi,
    chat: DurableChatReadApi,
}

impl AgentMcpResources {
    pub fn new(
        git: Arc<DurableGitBackend>,
        ci: DurableCiReadApi,
        issues: DurableIssueReadApi,
        knowledge: DurableKnowledgeReadApi,
        chat: DurableChatReadApi,
    ) -> Self {
        Self {
            git,
            ci,
            issues,
            knowledge,
            chat,
        }
    }
}

#[derive(Clone)]
pub struct AgentMcpServices {
    authority: AgentMcpAuthority,
    provider: SubstrateProvider,
    resources: AgentMcpResources,
    audit: Arc<OutboxGovernanceAudit>,
    runtime: Handle,
}

impl AgentMcpServices {
    pub fn new(
        authority: AgentMcpAuthority,
        provider: SubstrateProvider,
        resources: AgentMcpResources,
        runtime: Handle,
    ) -> Self {
        let audit = Arc::new(OutboxGovernanceAudit::new(
            OutboxStore::durable(Arc::new(PgOutboxBacking::new(
                provider.db_pool().clone(),
                runtime.clone(),
            ))),
            Arc::new(UlidMinter::new()),
        ));
        Self {
            authority,
            provider,
            resources,
            audit,
            runtime,
        }
    }

    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: Future<Output = T>,
    {
        match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                Ok(tokio::task::block_in_place(|| {
                    self.runtime.block_on(future)
                }))
            }
            Ok(_) => Err(EdgeError::Internal(
                "agent MCP requires the Edge multi-thread runtime".into(),
            )),
            Err(_) => Ok(self.runtime.block_on(future)),
        }
    }
}

struct AgentMcpHandler {
    services: AgentMcpServices,
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
        let authorized = self
            .services
            .drive(self.services.authority.sessions.authorize(
                ctx.principal,
                run_id,
                &capability.jti,
                Utc::now(),
            ))?
            .map_err(map_session_error)?;
        let registration = self
            .services
            .drive(
                self.services
                    .authority
                    .registry
                    .get(ctx.principal, &authorized.agent_id),
            )?
            .map_err(map_registry_error)?;
        validate_registration(ctx.principal, &registration)?;
        let delegator = active_delegator(
            &self.services.authority.principals,
            ctx.scope,
            &PrincipalId(registration.created_by.clone()),
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
            run_id: RunId(authorized.run_id.clone()),
        };
        let effect_api = Box::new(GitEffectApi::new(
            self.services.resources.git.clone(),
            ctx.principal.tenant.0.clone(),
            ctx.principal.region.0.clone(),
            ctx.principal.clone(),
            delegator.clone(),
            self.services.authority.boundary.clone(),
        ));
        let approvers = Arc::new(CreatorApproverPolicy {
            creator_id: PrincipalId(registration.created_by),
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
            .with_issues(self.services.resources.issues.clone())
            .with_knowledge(self.services.resources.knowledge.clone())
            .with_chat(self.services.resources.chat.clone()),
        );
        let server = McpServer::with_router_and_reads(registry, router, reads);
        let frame = std::str::from_utf8(&ctx.request.body)
            .map_err(|_| EdgeError::BadRequest("MCP frame must be valid UTF-8".into()))?;
        let response = server.handle_line(frame);

        if server.router().is_some_and(GovernedRouter::is_fatal) {
            self.services
                .drive(
                    self.services
                        .authority
                        .sessions
                        .terminate(ctx.principal, run_id, &capability.jti),
                )?
                .map_err(map_session_error)?;
        }

        match response {
            Some(response) => Ok(no_store(EdgeResponse::Bytes {
                status: 200,
                content_type: "application/json".into(),
                headers: Vec::new(),
                body: response.into_bytes(),
            })),
            None => Ok(no_store(EdgeResponse::Bytes {
                status: 204,
                content_type: "application/json".into(),
                headers: Vec::new(),
                body: Vec::new(),
            })),
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
    builder.route(
        Method::Post,
        "/v1/agent-runs/{run}/mcp",
        "identity.agent.run.mcp",
        Arc::new(AgentMcpHandler { services }),
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
        if tool != "git.merge" {
            return Err(format!(
                "tool `{tool}` has no registered Edge approval policy"
            ));
        }
        let repo = args
            .get("repo")
            .and_then(serde_json::Value::as_str)
            .filter(|repo| !repo.is_empty() && repo.len() <= 255)
            .ok_or_else(|| "merge approval requires a bounded repository slug".to_string())?;
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
) -> Result<(), EdgeError> {
    if registration.principal_id != principal.principal_id.0
        || registration.runtime_ref != EXTERNAL_MCP_RUNTIME
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
