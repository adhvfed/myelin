use std::sync::Arc;

use chrono::{DateTime, Duration, Timelike, Utc};
use myelin_agent_service::{workspace::AgentWorkspaceProvisioner, PlatformToolCatalogue};
use myelin_chat::conversation::{Conversation, ConversationKind};
use myelin_chat::events::pseudonymized_event_principal;
use myelin_chat::store::{ConversationId, SystemUlidSource, UlidSource};
use myelin_events::{Actor, EventId, IdMinter, Timestamp};
use myelin_identity::{Principal, PrincipalId};
use myelin_identity_service::{AgentSessionIssuer, AgentSessionRequest};
use myelin_storage::{
    ActivateAgentThreadOutcome, AgentThreadRunBinding, AgentThreadState, BindAgentThreadRunOutcome,
    CreateAgentThreadOutcome, DurableAgentThread, DurableAgentThreadBacking, NewAgentThread,
    MAX_AGENT_THREAD_NAME_BYTES, MAX_AGENT_THREAD_RETENTION_DAYS, MIN_AGENT_THREAD_RETENTION_DAYS,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Uuid;
use tokio::runtime::Handle;

use crate::agent_http::{agent_session_json, map_session_error};
use crate::catalogue::{page_envelope, Handler, HandlerCtx, DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::chat_http::{DurableChatMutationApi, DurableChatReadApi, PrivateConversationCreation};
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::runtime::drive_edge_future;
use crate::{EdgeError, Method};

const MAX_AGENT_THREAD_JSON_BYTES: usize = 16 * 1024;
const DEFAULT_RETENTION_DAYS: i16 = 3;

#[derive(Clone)]
struct AgentThreadHttpApi {
    threads: DurableAgentThreadBacking,
    chat_reads: DurableChatReadApi,
    chat_mutations: DurableChatMutationApi,
    workspaces: Arc<dyn AgentWorkspaceProvisioner>,
    sessions: AgentSessionIssuer,
    catalogue: Arc<PlatformToolCatalogue>,
    event_ids: Arc<dyn IdMinter>,
    conversation_ids: Arc<dyn UlidSource>,
    runtime: Handle,
}

impl AgentThreadHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: std::future::Future<Output = T>,
    {
        drive_edge_future(&self.runtime, future, "agent thread HTTP")
    }

    fn create(
        &self,
        principal: &Principal,
        body: CreateAgentThreadBody,
        client_nonce: String,
    ) -> Result<(DurableAgentThread, bool), EdgeError> {
        let intent = ValidatedThreadIntent::new(principal, body, client_nonce)?;
        if let Some(project_id) = intent.project_id.as_deref() {
            if !self.chat_reads.may_view_project(principal, project_id) {
                return Err(EdgeError::NotFound("project not found".into()));
            }
        }
        let proposal = intent.proposal(self.conversation_ids.mint().as_str())?;
        let outcome = self
            .drive(self.threads.create(&principal.tenant.0, proposal))?
            .map_err(|_| EdgeError::Internal("agent thread storage failed".into()))?;
        let (thread, created) = match outcome {
            CreateAgentThreadOutcome::Created(thread) => (thread, true),
            CreateAgentThreadOutcome::Replayed(thread) => (thread, false),
            CreateAgentThreadOutcome::Conflict => {
                return Err(EdgeError::Conflict(
                    "that idempotency key was already used for a different agent thread".into(),
                ))
            }
            CreateAgentThreadOutcome::NameConflict => {
                return Err(EdgeError::Conflict(
                    "a live agent thread already has that name".into(),
                ))
            }
            CreateAgentThreadOutcome::OwnerUnavailable => {
                return Err(EdgeError::NotFound("agent not found".into()))
            }
            CreateAgentThreadOutcome::AgentUnavailable => {
                return Err(EdgeError::NotFound("agent not found".into()))
            }
        };
        if thread.state == AgentThreadState::Ready {
            return Ok((thread, created));
        }
        if thread.state != AgentThreadState::Provisioning {
            return Err(EdgeError::Conflict(
                "agent thread is not available for provisioning".into(),
            ));
        }

        self.create_private_conversation(principal, &thread)?;
        let workspace_id = parse_uuid("stored workspace id", &thread.workspace_id)
            .map_err(|_| EdgeError::Internal("stored agent workspace id is invalid".into()))?;
        let provisioned = self
            .workspaces
            .provision(&principal.tenant.0, workspace_id)
            .map_err(|_| EdgeError::Unavailable("agent workspace provisioning failed".into()))?;
        let activated = self
            .drive(
                self.threads.activate(
                    &principal.tenant.0,
                    &principal.principal_id.0,
                    parse_uuid("stored thread id", &thread.thread_id).map_err(|_| {
                        EdgeError::Internal("stored agent thread id is invalid".into())
                    })?,
                    workspace_id,
                    &thread.conversation_id,
                    &provisioned.locator,
                ),
            )?
            .map_err(|_| EdgeError::Internal("agent thread activation failed".into()))?;
        match activated {
            ActivateAgentThreadOutcome::Activated(ready)
            | ActivateAgentThreadOutcome::AlreadyReady(ready) => Ok((ready, created)),
            ActivateAgentThreadOutcome::NotFound | ActivateAgentThreadOutcome::Conflict => Err(
                EdgeError::Conflict("agent thread provisioning receipt changed".into()),
            ),
        }
    }

    fn create_private_conversation(
        &self,
        principal: &Principal,
        thread: &DurableAgentThread,
    ) -> Result<(), EdgeError> {
        let conversation_id = ConversationId::new(
            principal.tenant.0.clone(),
            principal.region.0.clone(),
            thread.conversation_id.clone(),
        );
        let event_principal = pseudonymized_event_principal(&principal.tenant.0, principal);
        let conversation = Conversation {
            home_cell: Conversation::home_cell_for(&conversation_id),
            id: conversation_id,
            kind: ConversationKind::ChannelPrivate,
            parent_project: thread.project_id.clone(),
            name: Some(thread.name.clone()),
            topic: Some("Private agent work".into()),
            linked_ref: Some(thread_ref(&principal.tenant.0, &thread.thread_id)),
            pinned_canvas: None,
            retention_days: Some(i32::from(thread.retention_days)),
            archived: false,
            created_by: event_principal.principal_id.0.clone(),
            acl_zookie: None,
        };
        self.chat_mutations.create_private_conversation(
            principal,
            PrivateConversationCreation {
                conversation,
                members: vec![
                    principal.principal_id.clone(),
                    PrincipalId(format!("agent:{}", thread.agent_id)),
                ],
                expires_at: Some(Timestamp(thread.expires_at.clone())),
                client_nonce: format!("agent-thread-chat-{}", thread.thread_id),
                event_id: EventId(self.event_ids.mint().0),
                actor: Actor(event_principal),
                now: Timestamp(thread.created_at.clone()),
            },
        )?;
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAgentThreadBody {
    name: String,
    agent_id: String,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default = "default_retention_days")]
    retention_days: i16,
}

struct ValidatedThreadIntent {
    owner_principal_id: String,
    name: String,
    agent_id: Uuid,
    project_id: Option<String>,
    retention_days: i16,
    client_nonce: String,
    now: DateTime<Utc>,
}

impl ValidatedThreadIntent {
    fn new(
        principal: &Principal,
        body: CreateAgentThreadBody,
        client_nonce: String,
    ) -> Result<Self, EdgeError> {
        validate_name(&body.name)?;
        let agent_id = parse_uuid("agent_id", &body.agent_id)?;
        if let Some(project_id) = body.project_id.as_deref() {
            parse_uuid("project_id", project_id)?;
        }
        if !(MIN_AGENT_THREAD_RETENTION_DAYS..=MAX_AGENT_THREAD_RETENTION_DAYS)
            .contains(&body.retention_days)
        {
            return Err(EdgeError::BadRequest(format!(
                "agent thread retention_days must be between {MIN_AGENT_THREAD_RETENTION_DAYS} and {MAX_AGENT_THREAD_RETENTION_DAYS}"
            )));
        }
        let now = Utc::now()
            .with_nanosecond(0)
            .expect("zero nanoseconds is a valid timestamp");
        Ok(Self {
            owner_principal_id: principal.principal_id.0.clone(),
            name: body.name,
            agent_id,
            project_id: body.project_id,
            retention_days: body.retention_days,
            client_nonce,
            now,
        })
    }

    fn proposal(&self, conversation_id: &str) -> Result<NewAgentThread, EdgeError> {
        let expires_at = self
            .now
            .checked_add_signed(Duration::days(i64::from(self.retention_days)))
            .ok_or_else(|| EdgeError::BadRequest("agent thread retention overflowed".into()))?;
        Ok(NewAgentThread {
            thread_id: Uuid::new_v4(),
            owner_principal_id: self.owner_principal_id.clone(),
            agent_id: self.agent_id,
            conversation_id: conversation_id.into(),
            workspace_id: Uuid::new_v4(),
            name: self.name.clone(),
            project_id: self
                .project_id
                .as_deref()
                .map(|value| parse_uuid("project_id", value))
                .transpose()?,
            retention_days: self.retention_days,
            client_nonce: self.client_nonce.clone(),
            created_at: self.now,
            expires_at,
        })
    }
}

struct AgentThreadCreateHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body = parse_body(&ctx.request.body)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let (thread, created) = self.api.create(ctx.principal, body, client_nonce)?;
        Ok(no_store(EdgeResponse::json(
            if created { 201 } else { 200 },
            &json!({
                "thread": thread_json(&ctx.principal.tenant.0, &thread),
                "created": created,
                "durable": true,
            }),
        )))
    }
}

struct AgentThreadGetHandler {
    api: AgentThreadHttpApi,
}

struct AgentThreadRunCreateHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadRunCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        crate::request::require_empty_json_object(
            &ctx.request.body,
            "private agent thread run",
            MAX_AGENT_THREAD_JSON_BYTES,
        )?;
        let thread_id = thread_param(ctx)?;
        let thread = self
            .api
            .drive(self.api.threads.get_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                thread_id,
            ))?
            .map_err(|_| EdgeError::Internal("agent thread lookup failed".into()))?
            .ok_or_else(|| EdgeError::NotFound("agent thread not found".into()))?;
        let now = Utc::now();
        let workspace_expires_at = parse_stored_timestamp(&thread.expires_at)?;
        if thread.state != AgentThreadState::Ready || workspace_expires_at <= now {
            return Err(EdgeError::Conflict(
                "agent thread workspace is not available for a new run".into(),
            ));
        }

        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let capability = ctx.identity.capability();
        let issued = self
            .api
            .drive(self.api.sessions.start_until(
                ctx.principal,
                AgentSessionRequest {
                    agent_id: thread.agent_id.clone(),
                    client_nonce,
                    trigger_credential_jti: capability.jti.clone(),
                    trigger_expires_at_unix: capability.expires_at_unix,
                    trigger_authority: capability.effective_authority.clone(),
                    now,
                },
                workspace_expires_at,
            ))?
            .map_err(map_session_error)?;
        let run_id = parse_uuid("stored run id", &issued.session.run_id)
            .map_err(|_| EdgeError::Internal("stored agent run id is invalid".into()))?;
        let binding = self
            .api
            .drive(self.api.threads.bind_run(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                thread_id,
                run_id,
                now,
            ))?
            .map_err(|_| EdgeError::Internal("agent thread run binding failed".into()))?;
        let binding = match binding {
            BindAgentThreadRunOutcome::Bound(binding)
            | BindAgentThreadRunOutcome::Replayed(binding) => binding,
            BindAgentThreadRunOutcome::NotFound | BindAgentThreadRunOutcome::Conflict => {
                self.api
                    .drive(self.api.sessions.revoke_unreturned(ctx.principal, &issued))?
                    .map_err(map_session_error)?;
                return Err(EdgeError::Conflict(
                    "agent thread changed while its run was starting".into(),
                ));
            }
        };
        Ok(no_store(EdgeResponse::json(
            if issued.created { 201 } else { 200 },
            &thread_run_json(
                &ctx.principal.tenant.0,
                &self.api.catalogue,
                &issued,
                &binding,
            ),
        )))
    }
}

impl Handler for AgentThreadGetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        require_empty_body(ctx)?;
        let thread_id = thread_param(ctx)?;
        let thread = self
            .api
            .drive(self.api.threads.get_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                thread_id,
            ))?
            .map_err(|_| EdgeError::Internal("agent thread lookup failed".into()))?
            .ok_or_else(|| EdgeError::NotFound("agent thread not found".into()))?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({ "thread": thread_json(&ctx.principal.tenant.0, &thread) }),
        )))
    }
}

struct AgentThreadListHandler {
    api: AgentThreadHttpApi,
}

impl Handler for AgentThreadListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx)?;
        let (limit, cursor) = page_query(&ctx.request.query)?;
        let mut threads = self
            .api
            .drive(self.api.threads.list_for_owner(
                &ctx.principal.tenant.0,
                &ctx.principal.principal_id.0,
                cursor,
                limit + 1,
            ))?
            .map_err(|_| EdgeError::Internal("agent thread list failed".into()))?;
        let has_more = threads.len() > limit as usize;
        threads.truncate(limit as usize);
        let next = has_more
            .then(|| threads.last().map(|thread| thread.thread_id.clone()))
            .flatten();
        let items = threads
            .iter()
            .map(|thread| thread_json(&ctx.principal.tenant.0, thread))
            .collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, limit as usize),
        )))
    }
}

pub fn register_agent_threads(
    builder: GatewayBuilder,
    threads: DurableAgentThreadBacking,
    chat_reads: DurableChatReadApi,
    chat_mutations: DurableChatMutationApi,
    workspaces: Arc<dyn AgentWorkspaceProvisioner>,
    sessions: AgentSessionIssuer,
    runtime: Handle,
) -> GatewayBuilder {
    let api = AgentThreadHttpApi {
        threads,
        chat_reads,
        chat_mutations,
        workspaces,
        sessions,
        catalogue: Arc::new(
            PlatformToolCatalogue::platform()
                .expect("the built-in platform ToolDef catalogue must be valid"),
        ),
        event_ids: Arc::new(myelin_events::UlidMinter::new()),
        conversation_ids: Arc::new(SystemUlidSource::new()),
        runtime,
    };
    builder
        .route(
            Method::Get,
            "/v1/agent-threads",
            "identity.agent_threads.list",
            Arc::new(AgentThreadListHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-threads",
            "identity.agent_thread.create",
            Arc::new(AgentThreadCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/agent-threads/{thread}",
            "identity.agent_thread.view",
            Arc::new(AgentThreadGetHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/agent-threads/{thread}/runs",
            "identity.agent_thread.run.create",
            Arc::new(AgentThreadRunCreateHandler { api }),
        )
}

fn thread_run_json(
    tenant: &str,
    catalogue: &PlatformToolCatalogue,
    issued: &myelin_identity_service::IssuedAgentSession,
    binding: &AgentThreadRunBinding,
) -> Value {
    let mut value = agent_session_json(tenant, catalogue, issued);
    value["run"]["context"] = json!({
        "thread_id": binding.thread_id,
        "thread_ref": thread_ref(tenant, &binding.thread_id),
        "conversation_id": binding.conversation_id,
        "conversation_ref": format!(
            "myelin://{tenant}/chat/channel/{}",
            binding.conversation_id
        ),
        "workspace": {
            "id": binding.workspace_id,
            "generation": binding.workspace_generation,
            "expires_at": binding.workspace_expires_at,
        },
    });
    value
}

fn thread_json(tenant: &str, thread: &DurableAgentThread) -> Value {
    json!({
        "id": thread.thread_id,
        "ref": thread_ref(tenant, &thread.thread_id),
        "name": thread.name,
        "agent_id": thread.agent_id,
        "agent_ref": format!("myelin://{tenant}/identity/agent/{}", thread.agent_id),
        "project_id": thread.project_id,
        "conversation_id": thread.conversation_id,
        "conversation_ref": format!(
            "myelin://{tenant}/chat/channel/{}",
            thread.conversation_id
        ),
        "workspace": {
            "id": thread.workspace_id,
            "generation": thread.workspace_generation,
            "state": thread.state.token(),
            "retention_days": thread.retention_days,
            "expires_at": thread.expires_at,
        },
        "created_at": thread.created_at,
        "updated_at": thread.updated_at,
    })
}

fn thread_ref(tenant: &str, thread_id: &str) -> String {
    format!("myelin://{tenant}/agent/thread/{thread_id}")
}

fn parse_body(body: &[u8]) -> Result<CreateAgentThreadBody, EdgeError> {
    if body.is_empty() {
        return Err(EdgeError::BadRequest(
            "agent thread request body is empty".into(),
        ));
    }
    if body.len() > MAX_AGENT_THREAD_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(
            "agent thread request exceeds the interactive body limit".into(),
        ));
    }
    serde_json::from_slice(body)
        .map_err(|error| EdgeError::BadRequest(format!("invalid agent thread request: {error}")))
}

fn parse_uuid(field: &str, value: &str) -> Result<Uuid, EdgeError> {
    let parsed = Uuid::parse_str(value).map_err(|_| {
        EdgeError::BadRequest(format!("agent thread {field} must be a canonical UUID"))
    })?;
    if parsed.to_string() != value {
        return Err(EdgeError::BadRequest(format!(
            "agent thread {field} must be a canonical UUID"
        )));
    }
    Ok(parsed)
}

fn parse_stored_timestamp(value: &str) -> Result<DateTime<Utc>, EdgeError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| EdgeError::Internal("stored agent thread expiry is invalid".into()))
}

fn validate_name(name: &str) -> Result<(), EdgeError> {
    if name.trim() == name
        && !name.is_empty()
        && name.len() <= MAX_AGENT_THREAD_NAME_BYTES
        && !name.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(format!(
            "agent thread name must contain 1..={MAX_AGENT_THREAD_NAME_BYTES} clean UTF-8 bytes without surrounding whitespace"
        )))
    }
}

fn default_retention_days() -> i16 {
    DEFAULT_RETENTION_DAYS
}

fn page_query(query: &str) -> Result<(u32, Option<Uuid>), EdgeError> {
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or_else(|| {
                EdgeError::BadRequest("malformed agent thread query parameter".into())
            })?;
            match name {
                "limit" if limit.is_none() => {
                    limit = Some(value.parse::<u32>().map_err(|_| {
                        EdgeError::BadRequest("agent thread limit must be an integer".into())
                    })?);
                }
                "cursor" if cursor.is_none() => cursor = Some(parse_uuid("cursor", value)?),
                "limit" | "cursor" => {
                    return Err(EdgeError::BadRequest(format!(
                        "duplicate agent thread {name}"
                    )))
                }
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown agent thread query parameter `{other}`"
                    )))
                }
            }
        }
    }
    let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT as u32);
    if !(1..=MAX_PAGE_LIMIT as u32).contains(&limit) {
        return Err(EdgeError::BadRequest(
            "agent thread limit must be between 1 and 100".into(),
        ));
    }
    Ok((limit, cursor))
}

fn thread_param(ctx: &HandlerCtx<'_>) -> Result<Uuid, EdgeError> {
    let value = ctx
        .params
        .get("thread")
        .ok_or_else(|| EdgeError::BadRequest("route did not bind an agent thread id".into()))?;
    parse_uuid("id", value)
}

fn require_empty_query(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.query.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "agent thread request accepts no query parameters".into(),
        ))
    }
}

fn require_empty_body(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.body.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "agent thread read accepts no request body".into(),
        ))
    }
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalKind, PrincipalStatus};
    use myelin_tenancy::TenantId;

    fn principal() -> Principal {
        let mut principal = Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        principal.status = PrincipalStatus::Active;
        principal
    }

    #[test]
    fn creation_intent_has_a_fixed_visible_expiry() {
        let body: CreateAgentThreadBody = serde_json::from_value(json!({
            "name": "Investigate checkout race",
            "agent_id": "11111111-1111-4111-8111-111111111111",
            "retention_days": 3,
        }))
        .unwrap();
        let intent = ValidatedThreadIntent::new(&principal(), body, "retry".into()).unwrap();
        let proposal = intent.proposal("01J00000000000000000000000").unwrap();
        assert_eq!(proposal.expires_at - proposal.created_at, Duration::days(3));
    }

    #[test]
    fn malformed_or_surprising_creation_input_is_refused() {
        for body in [
            br#"{}"#.as_slice(),
            br#"{"name":" x","agent_id":"11111111-1111-4111-8111-111111111111"}"#,
            br#"{"name":"x","agent_id":"not-a-uuid"}"#,
            br#"{"name":"x","agent_id":"11111111-1111-4111-8111-111111111111","retention_days":31}"#,
            br#"{"name":"x","agent_id":"11111111-1111-4111-8111-111111111111","extra":true}"#,
        ] {
            let refused = parse_body(body)
                .and_then(|body| ValidatedThreadIntent::new(&principal(), body, "retry".into()))
                .is_err();
            assert!(refused, "unexpectedly accepted {}", String::from_utf8_lossy(body));
        }
    }

    #[test]
    fn thread_json_never_projects_the_internal_storage_locator() {
        let thread = DurableAgentThread {
            thread_id: "11111111-1111-4111-8111-111111111111".into(),
            owner_principal_id: "p:alice".into(),
            agent_id: "22222222-2222-4222-8222-222222222222".into(),
            conversation_id: "01J00000000000000000000000".into(),
            workspace_id: "33333333-3333-4333-8333-333333333333".into(),
            workspace_generation: 1,
            name: "Investigate checkout race".into(),
            project_id: None,
            retention_days: 3,
            state: AgentThreadState::Ready,
            storage_locator: Some("workspace:v1:internal".into()),
            failure_reason: None,
            created_at: "2026-08-22T00:00:00Z".into(),
            expires_at: "2026-08-25T00:00:00Z".into(),
            updated_at: "2026-08-22T00:00:01Z".into(),
        };
        let rendered = thread_json("acme", &thread);
        assert!(!rendered.to_string().contains("internal"));
        assert_eq!(rendered["workspace"]["state"], "ready");
    }
}
