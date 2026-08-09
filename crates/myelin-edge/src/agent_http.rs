use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use myelin_agent::is_canonical_tool_name;
use myelin_agent_service::{catalogue_cursor, tool_ref, PlatformToolCatalogue};
use myelin_identity::PrincipalStatus;
use myelin_identity_service::{
    agent_ref, agent_run_ref, AgentActivation, AgentLifecycleAction, AgentLifecycleOutcome,
    AgentLifecycleRequest, AgentRegistration, AgentRegistryError, AgentSessionError,
    AgentSessionIssuer, AgentSessionRequest, ClosedAgentSession, IssuedAgentSession, NewAgent,
    PgAgentRegistry, EXTERNAL_MCP_RUNTIME,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::catalogue::{page_envelope, Handler, HandlerCtx, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::Method;

const MAX_AGENT_JSON_BYTES: usize = 16 * 1024;
const AGENT_BASELINE_GRANTS: &[&str] = &["agent.tools.read", "edge.identity.read"];

#[derive(Clone)]
struct AgentHttpApi {
    registry: PgAgentRegistry,
    sessions: AgentSessionIssuer,
    catalogue: Arc<PlatformToolCatalogue>,
    runtime: Handle,
}

impl AgentHttpApi {
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
                "agent HTTP requires the Edge multi-thread runtime".into(),
            )),
            Err(_) => Ok(self.runtime.block_on(future)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgentBody {
    name: String,
    tools: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyJsonObject {}

struct AgentCreateHandler {
    api: AgentHttpApi,
}

impl Handler for AgentCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body = parse_create_body(&ctx.request.body)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let proposal = activation_proposal(ctx, &self.api.catalogue, body, client_nonce)?;
        let activation = self
            .api
            .drive(self.api.registry.create(ctx.principal, proposal))?
            .map_err(map_registry_error)?;
        Ok(no_store(EdgeResponse::json(
            if activation.created { 201 } else { 200 },
            &activation_json(&ctx.principal.tenant.0, &self.api.catalogue, &activation),
        )))
    }
}

struct AgentGetHandler {
    api: AgentHttpApi,
}

struct AgentRunCreateHandler {
    api: AgentHttpApi,
}

struct AgentRunCloseHandler {
    api: AgentHttpApi,
}

struct AgentLifecycleHandler {
    api: AgentHttpApi,
    action: AgentLifecycleAction,
}

impl Handler for AgentRunCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let _: EmptyJsonObject = parse_empty_json_object(&ctx.request.body, "agent run")?;
        let agent_id = agent_param(ctx)?.to_string();
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let capability = ctx.identity.capability();
        let issued = self
            .api
            .drive(self.api.sessions.start(
                ctx.principal,
                AgentSessionRequest {
                    agent_id,
                    client_nonce,
                    trigger_credential_jti: capability.jti.clone(),
                    trigger_expires_at_unix: capability.expires_at_unix,
                    trigger_authority: capability.effective_authority.clone(),
                    now: chrono::Utc::now(),
                },
            ))?
            .map_err(map_session_error)?;
        Ok(no_store(EdgeResponse::json(
            if issued.created { 201 } else { 200 },
            &agent_session_json(&ctx.principal.tenant.0, &self.api.catalogue, &issued),
        )))
    }
}

impl Handler for AgentRunCloseHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let _: EmptyJsonObject = parse_empty_json_object(&ctx.request.body, "agent run close")?;
        let run_id = run_param(ctx)?;
        let closed = self
            .api
            .drive(
                self.api
                    .sessions
                    .close(ctx.principal, run_id, &ctx.identity.capability().jti),
            )?
            .map_err(map_session_error)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &closed_agent_session_json(&ctx.principal.tenant.0, &closed),
        )))
    }
}

impl Handler for AgentLifecycleHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let _: EmptyJsonObject = parse_empty_json_object(&ctx.request.body, "agent lifecycle")?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let outcome = self
            .api
            .drive(self.api.registry.change_status(
                ctx.principal,
                AgentLifecycleRequest {
                    agent_id: agent_param(ctx)?.to_string(),
                    action: self.action,
                    client_nonce,
                },
            ))?
            .map_err(map_registry_error)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &lifecycle_json(
                &ctx.principal.tenant.0,
                &self.api.catalogue,
                self.action,
                &outcome,
            ),
        )))
    }
}

impl Handler for AgentGetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        require_empty_body(ctx, "agent lookup")?;
        let id = agent_param(ctx)?;
        let agent = self
            .api
            .drive(self.api.registry.get(ctx.principal, id))?
            .map_err(map_registry_error)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "agent": agent_json(&ctx.principal.tenant.0, &self.api.catalogue, &agent),
            }),
        )))
    }
}

struct AgentListHandler {
    api: AgentHttpApi,
}

impl Handler for AgentListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx, "agent list")?;
        let (limit, cursor) = parse_page_query(&ctx.request.query)?;
        let mut agents = self
            .api
            .drive(
                self.api
                    .registry
                    .list(ctx.principal, cursor.as_deref(), limit as u32 + 1),
            )?
            .map_err(map_registry_error)?;
        let has_more = agents.len() > limit;
        agents.truncate(limit);
        let next_cursor = has_more
            .then(|| agents.last().map(|agent| agent.id.clone()))
            .flatten();
        let items = agents
            .iter()
            .map(|agent| agent_json(&ctx.principal.tenant.0, &self.api.catalogue, agent))
            .collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next_cursor, limit),
        )))
    }
}

pub fn register_agents(
    builder: GatewayBuilder,
    registry: PgAgentRegistry,
    sessions: AgentSessionIssuer,
    runtime: Handle,
) -> GatewayBuilder {
    let catalogue = Arc::new(
        PlatformToolCatalogue::platform()
            .expect("the built-in platform ToolDef catalogue must be valid"),
    );
    let api = AgentHttpApi {
        registry,
        sessions,
        catalogue,
        runtime,
    };
    builder
        .route(
            Method::Get,
            "/v1/agents",
            "identity.agents.list",
            Arc::new(AgentListHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agents",
            "identity.agent.create",
            Arc::new(AgentCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/agents/{agent}",
            "identity.agent.view",
            Arc::new(AgentGetHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agents/{agent}/suspend",
            "identity.agent.suspend",
            Arc::new(AgentLifecycleHandler {
                api: api.clone(),
                action: AgentLifecycleAction::Suspend,
            }),
        )
        .route(
            Method::Post,
            "/v1/agents/{agent}/resume",
            "identity.agent.resume",
            Arc::new(AgentLifecycleHandler {
                api: api.clone(),
                action: AgentLifecycleAction::Resume,
            }),
        )
        .route(
            Method::Post,
            "/v1/agents/{agent}/retire",
            "identity.agent.retire",
            Arc::new(AgentLifecycleHandler {
                api: api.clone(),
                action: AgentLifecycleAction::Retire,
            }),
        )
        .route(
            Method::Post,
            "/v1/agents/{agent}/runs",
            "identity.agent.run.create",
            Arc::new(AgentRunCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-runs/{run}/close",
            "identity.agent.run.close",
            Arc::new(AgentRunCloseHandler { api }),
        )
}

fn activation_proposal(
    ctx: &HandlerCtx<'_>,
    catalogue: &PlatformToolCatalogue,
    body: CreateAgentBody,
    client_nonce: String,
) -> Result<NewAgent, EdgeError> {
    if body.tools.is_empty() {
        return Err(EdgeError::BadRequest(
            "agent create needs at least one `--tool` selection".into(),
        ));
    }
    let mut requested = body.tools;
    if requested.len() > myelin_identity_service::MAX_AGENT_TOOLS {
        return Err(EdgeError::BadRequest(format!(
            "agent create accepts at most {} tools",
            myelin_identity_service::MAX_AGENT_TOOLS
        )));
    }
    let original_len = requested.len();
    requested.sort();
    requested.dedup();
    if requested.len() != original_len {
        return Err(EdgeError::BadRequest(
            "agent tool selections must be distinct".into(),
        ));
    }

    let actor_authority = &ctx.identity.capability().effective_authority;
    let mut selected = Vec::with_capacity(requested.len());
    let mut grants = AGENT_BASELINE_GRANTS
        .iter()
        .map(|grant| (*grant).to_string())
        .collect::<BTreeSet<_>>();
    for name in requested {
        if !is_canonical_tool_name(&name) {
            return Err(EdgeError::BadRequest(format!(
                "agent tool `{name}` is not a canonical `<subsystem>.<name>`"
            )));
        }
        let definition = catalogue
            .resolve(&name)
            .filter(|definition| definition.exposed_over_mcp)
            .ok_or_else(|| EdgeError::BadRequest(format!("agent tool `{name}` is unavailable")))?;
        for capability in &definition.required_caps {
            if !actor_authority.holds(capability) {
                return Err(EdgeError::Forbidden(format!(
                    "the authenticated human cannot delegate `{capability}`"
                )));
            }
            grants.insert(capability.clone());
        }
        selected.push(catalogue_cursor(definition));
    }
    for baseline in AGENT_BASELINE_GRANTS {
        if !actor_authority.holds(baseline) {
            return Err(EdgeError::Forbidden(format!(
                "the authenticated human cannot delegate `{baseline}`"
            )));
        }
    }

    let mut tenant_policy = AGENT_BASELINE_GRANTS
        .iter()
        .map(|grant| (*grant).to_string())
        .collect::<BTreeSet<_>>();
    for definition in catalogue.latest_definitions() {
        if definition.exposed_over_mcp {
            tenant_policy.extend(definition.required_caps.iter().cloned());
        }
    }
    let trigger_actor_policy = actor_authority
        .grants()
        .map(str::to_string)
        .collect::<Vec<_>>();
    Ok(NewAgent {
        name: body.name,
        runtime_ref: EXTERNAL_MCP_RUNTIME.into(),
        tools: selected,
        grants: grants.into_iter().collect(),
        tenant_policy_if_missing: tenant_policy.into_iter().collect(),
        trigger_actor_policy_if_missing: trigger_actor_policy,
        client_nonce,
    })
}

fn activation_json(
    tenant: &str,
    catalogue: &PlatformToolCatalogue,
    activation: &AgentActivation,
) -> Value {
    json!({
        "agent": agent_json(tenant, catalogue, &activation.agent),
        "created": activation.created,
        "durable": true,
        "governance": {
            "policy_versions": {
                "agent": activation.policy_versions.agent,
                "delegation": activation.policy_versions.delegation,
                "tenant": activation.policy_versions.tenant,
                "trigger_actor": activation.policy_versions.trigger_actor,
            },
            "policy_revisions": {
                "agent": activation.policy_revisions.agent,
                "delegation": activation.policy_revisions.delegation,
                "tenant": activation.policy_revisions.tenant,
                "trigger_actor": activation.policy_revisions.trigger_actor,
            },
        },
    })
}

fn agent_json(tenant: &str, catalogue: &PlatformToolCatalogue, agent: &AgentRegistration) -> Value {
    let selected_tools = agent
        .tools
        .iter()
        .map(|cursor| selected_tool_json(tenant, catalogue, cursor))
        .collect::<Vec<_>>();
    let effective_tools = catalogue
        .latest_definitions()
        .into_iter()
        .filter(|definition| definition.exposed_over_mcp)
        .filter(|definition| {
            definition
                .required_caps
                .iter()
                .all(|required| agent.grants.binary_search(required).is_ok())
        })
        .map(|definition| {
            json!({
                "name": definition.canonical_name(),
                "version": definition.version,
                "ref": tool_ref(&myelin_tenancy::TenantId(tenant.into()), definition).0,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "id": agent.id,
        "ref": agent_ref(tenant, &agent.id).0,
        "principal_id": agent.principal_id,
        "name": agent.name,
        "runtime_ref": agent.runtime_ref,
        "on_behalf_of": agent.created_by,
        "status": status_name(agent.status),
        "selected_tools": selected_tools,
        "effective_tools": effective_tools,
        "grants": agent.grants,
        "created_at": agent.created_at,
    })
}

fn agent_session_json(
    tenant: &str,
    catalogue: &PlatformToolCatalogue,
    issued: &IssuedAgentSession,
) -> Value {
    let session = &issued.session;
    json!({
        "run": {
            "id": session.run_id,
            "ref": agent_run_ref(tenant, &session.run_id).0,
            "agent_id": session.agent_id,
            "agent_ref": agent_ref(tenant, &session.agent_id).0,
            "principal_id": session.agent_principal_id,
            "trigger_actor": session.trigger_actor_id,
            "selected_tools": session.selected_tools.iter()
                .map(|cursor| selected_tool_json(tenant, catalogue, cursor))
                .collect::<Vec<_>>(),
            "effective_grants": session.effective_grants,
            "state": session.state.token(),
            "issued_at": session.issued_at,
            "expires_at": session.expires_at,
        },
        "credential": {
            "scheme": myelin_identity_service::machine_scheme::AGENT,
            "token": issued.run_token.token,
            "expires_at": session.expires_at,
        },
        "created": issued.created,
        "durable": true,
    })
}

fn closed_agent_session_json(tenant: &str, closed: &ClosedAgentSession) -> Value {
    json!({
        "run": {
            "id": closed.run_id,
            "ref": agent_run_ref(tenant, &closed.run_id).0,
            "agent_id": closed.agent_id,
            "agent_ref": agent_ref(tenant, &closed.agent_id).0,
            "state": closed.state.token(),
        },
        "closed": true,
        "durable": true,
    })
}

fn lifecycle_json(
    tenant: &str,
    catalogue: &PlatformToolCatalogue,
    action: AgentLifecycleAction,
    outcome: &AgentLifecycleOutcome,
) -> Value {
    json!({
        "agent": agent_json(tenant, catalogue, &outcome.agent),
        "action": match action {
            AgentLifecycleAction::Suspend => "suspend",
            AgentLifecycleAction::Resume => "resume",
            AgentLifecycleAction::Retire => "retire",
        },
        "changed": outcome.changed,
        "terminated_runs": outcome.terminated_runs,
        "durable": true,
    })
}

fn selected_tool_json(tenant: &str, catalogue: &PlatformToolCatalogue, cursor: &str) -> Value {
    match catalogue
        .definitions()
        .iter()
        .find(|definition| catalogue_cursor(definition) == cursor)
    {
        Some(definition) => json!({
            "name": definition.canonical_name(),
            "version": definition.version,
            "ref": tool_ref(&myelin_tenancy::TenantId(tenant.into()), definition).0,
        }),
        None => json!({ "cursor": cursor, "state": "unavailable" }),
    }
}

fn status_name(status: PrincipalStatus) -> &'static str {
    match status {
        PrincipalStatus::Active => "active",
        PrincipalStatus::Suspended => "suspended",
        PrincipalStatus::Disabled => "disabled",
    }
}

fn parse_create_body(bytes: &[u8]) -> Result<CreateAgentBody, EdgeError> {
    if bytes.len() > MAX_AGENT_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "agent request body exceeds {MAX_AGENT_JSON_BYTES} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(
            "empty request body (expected JSON)".into(),
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid agent create body: {error}")))
}

fn parse_empty_json_object(bytes: &[u8], operation: &str) -> Result<EmptyJsonObject, EdgeError> {
    if bytes.len() > MAX_AGENT_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "{operation} request body exceeds {MAX_AGENT_JSON_BYTES} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(
            "empty request body (expected an empty JSON object)".into(),
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid {operation} body: {error}")))
}

fn parse_page_query(query: &str) -> Result<(usize, Option<String>), EdgeError> {
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| EdgeError::BadRequest("malformed agent query parameter".into()))?;
            match name {
                "limit" if limit.is_none() => {
                    limit = Some(
                        value
                            .parse::<usize>()
                            .ok()
                            .filter(|parsed| {
                                parsed.to_string() == value && (1..=MAX_PAGE_LIMIT).contains(parsed)
                            })
                            .ok_or_else(|| {
                                EdgeError::BadRequest(
                                    "agent limit must be a canonical integer between 1 and 100"
                                        .into(),
                                )
                            })?,
                    );
                }
                "cursor" if cursor.is_none() => cursor = Some(value.to_string()),
                "limit" | "cursor" => {
                    return Err(EdgeError::BadRequest(format!(
                        "duplicate agent query parameter `{name}`"
                    )))
                }
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown agent query parameter `{other}`"
                    )))
                }
            }
        }
    }
    Ok((limit.unwrap_or(DEFAULT_PAGE_LIMIT), cursor))
}

fn agent_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    ctx.params
        .get("agent")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind an agent id".into()))
}

fn run_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    ctx.params
        .get("run")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a run id".into()))
}

fn require_empty_query(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.query.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "agent operation accepts no query parameters".into(),
        ))
    }
}

fn require_empty_body(ctx: &HandlerCtx<'_>, operation: &str) -> Result<(), EdgeError> {
    if ctx.request.body.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(format!(
            "{operation} accepts no request body"
        )))
    }
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
        AgentSessionError::NotFound => EdgeError::NotFound("agent not found".into()),
        AgentSessionError::RunNotFound => EdgeError::NotFound("agent run not found".into()),
        AgentSessionError::Conflict(message) => EdgeError::Conflict(message),
        AgentSessionError::Policy(_) => {
            EdgeError::Forbidden("agent run delegation was refused".into())
        }
        AgentSessionError::Expired => EdgeError::Conflict(
            "agent run has expired; start a new run with a new idempotency key".into(),
        ),
        AgentSessionError::Storage(message) => EdgeError::Internal(message),
    }
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind};
    use myelin_identity_service::{
        Authority, CredentialAudience, CredentialContext, CredentialPurpose, DpopState,
        RequestIdentity, VerifiedCapabilityContext,
    };
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region, TenantId};
    use std::collections::BTreeMap;

    fn proposal(authority: &[&str], tools: &[&str]) -> Result<NewAgent, EdgeError> {
        let principal = Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("human:ada".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        let identity = RequestIdentity {
            principal: principal.clone(),
            scope: scope.clone(),
            credential: CredentialContext::Capability(VerifiedCapabilityContext {
                purpose: CredentialPurpose::HumanSession,
                audience: CredentialAudience::Edge,
                jti: "session".into(),
                effective_authority: Authority::of(authority.iter().copied()),
                expires_at_unix: i64::MAX,
                dpop: DpopState::Unbound,
            }),
        };
        let request = crate::EdgeRequest::new("POST", "/v1/agents", "", vec![], vec![]);
        let params = BTreeMap::new();
        let page = crate::Page {
            limit: 50,
            cursor: None,
        };
        activation_proposal(
            &HandlerCtx {
                identity: &identity,
                principal: &principal,
                scope: &scope,
                request: &request,
                params: &params,
                page: &page,
            },
            &PlatformToolCatalogue::platform().unwrap(),
            CreateAgentBody {
                name: "Review companion".into(),
                tools: tools.iter().map(|tool| (*tool).into()).collect(),
            },
            "retry-safe".into(),
        )
    }

    #[test]
    fn tool_selection_derives_a_bounded_non_amplifying_activation() {
        let proposal = proposal(
            &[
                "agent.tools.read",
                "edge.identity.read",
                "repo.push",
                "run.view",
            ],
            &["ci.read_run", "git.open_pr"],
        )
        .unwrap();
        assert_eq!(proposal.runtime_ref, EXTERNAL_MCP_RUNTIME);
        assert_eq!(proposal.tools, ["ci.read_run.v1", "git.open_pr.v1"]);
        assert_eq!(
            proposal.grants,
            [
                "agent.tools.read",
                "edge.identity.read",
                "repo.push",
                "run.view"
            ]
        );
        assert!(proposal
            .tenant_policy_if_missing
            .iter()
            .all(|grant| !grant.is_empty()));
    }

    #[test]
    fn page_queries_are_canonical_and_unambiguous() {
        assert!(parse_page_query("limit=01").is_err());
        assert!(parse_page_query("limit=1&limit=2").is_err());
        assert!(parse_page_query("other=1").is_err());
    }

    #[test]
    fn state_change_bodies_are_exact_empty_objects() {
        assert!(parse_empty_json_object(br#"{}"#, "agent lifecycle").is_ok());
        assert!(parse_empty_json_object(br#"{"ttl":3600}"#, "agent run").is_err());
        assert!(parse_empty_json_object(b"null", "agent run").is_err());
        assert!(parse_empty_json_object(b"", "agent run").is_err());
    }

    #[test]
    fn selection_refuses_unknown_duplicate_or_amplified_tools() {
        let authority = &["agent.tools.read", "edge.identity.read", "run.view"];

        assert!(matches!(
            proposal(authority, &["not.real"]),
            Err(EdgeError::BadRequest(_))
        ));
        assert!(matches!(
            proposal(authority, &["ci.read_run", "ci.read_run"]),
            Err(EdgeError::BadRequest(_))
        ));
        assert!(matches!(
            proposal(authority, &["git.open_pr"]),
            Err(EdgeError::Forbidden(_))
        ));
    }
}
