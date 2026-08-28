use chrono::{DateTime, Duration, Utc};
use myelin_events::clock::system_clock_reading;
use myelin_identity::Principal;
use myelin_identity_service::workspace_ssh_public_key_fingerprint;
use myelin_storage::{
    AgentThreadState, CreateWorkspaceSshGrantOutcome, DurableAgentThread,
    DurableAgentThreadBacking, DurableWorkspaceSession, DurableWorkspaceSshGrant,
    ListWorkspaceSessionsOutcome, NewWorkspaceSshGrant, WorkspaceSshRouteKey,
    MAX_WORKSPACE_SSH_GRANT_SECONDS,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Uuid;
use std::sync::Arc;
use tokio::runtime::Handle;

use crate::agent_thread_http::{
    no_store, parse_stored_timestamp, require_empty_query, thread_param,
    MAX_AGENT_THREAD_JSON_BYTES,
};
use crate::catalogue::{page_envelope, Handler, HandlerCtx, Page};
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::runtime::drive_edge_future;
use crate::{EdgeError, Method};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSshEndpoint {
    host: String,
    port: u16,
    host_public_key: String,
    host_key_fingerprint: String,
}

impl WorkspaceSshEndpoint {
    pub fn new(host: String, port: u16, host_public_key: String) -> Result<Self, String> {
        if host.is_empty()
            || host.len() > 253
            || !host.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'[' | b']')
            })
        {
            return Err("workspace SSH host must be a bounded DNS name or IP literal".into());
        }
        if port == 0 {
            return Err("workspace SSH port must be non-zero".into());
        }
        let host_key_fingerprint = workspace_ssh_public_key_fingerprint(&host_public_key)
            .map_err(|_| "workspace SSH host key must be a valid ssh-ed25519 public key")?;
        Ok(Self {
            host,
            port,
            host_public_key,
            host_key_fingerprint,
        })
    }
}

pub(crate) struct WorkspaceSshHttpInputs {
    pub threads: DurableAgentThreadBacking,
    pub routes: WorkspaceSshRouteKey,
    pub endpoint: WorkspaceSshEndpoint,
    pub runtime: Handle,
}

#[derive(Clone)]
struct WorkspaceSshHttpApi {
    threads: DurableAgentThreadBacking,
    routes: WorkspaceSshRouteKey,
    endpoint: WorkspaceSshEndpoint,
    runtime: Handle,
}

impl WorkspaceSshHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: std::future::Future<Output = T>,
    {
        drive_edge_future(&self.runtime, future, "workspace SSH HTTP")
    }

    fn owner_thread(
        &self,
        principal: &Principal,
        thread_id: Uuid,
    ) -> Result<DurableAgentThread, EdgeError> {
        self.drive(self.threads.get_for_owner(
            &principal.tenant.0,
            &principal.principal_id.0,
            thread_id,
        ))?
        .map_err(|_| EdgeError::Internal("agent thread lookup failed".into()))?
        .ok_or_else(|| EdgeError::NotFound("agent thread not found".into()))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateWorkspaceSshAccessBody {
    public_key: String,
}

struct WorkspaceSshAccessCreateHandler {
    api: WorkspaceSshHttpApi,
}

struct WorkspaceSessionListHandler {
    api: WorkspaceSshHttpApi,
}

impl Handler for WorkspaceSessionListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "workspace session history accepts no request body".into(),
            ));
        }
        let page = Page::parse(&ctx.request.query, "workspace session history")?;
        if page
            .cursor
            .as_deref()
            .is_some_and(|cursor| !canonical_ulid(cursor))
        {
            return Err(EdgeError::BadRequest(
                "workspace session cursor must be a canonical ULID".into(),
            ));
        }
        let thread_id = thread_param(ctx)?;
        self.api.owner_thread(ctx.principal, thread_id)?;
        let outcome = self
            .api
            .drive(
                self.api.threads.list_workspace_sessions_for_owner(
                    &ctx.principal.tenant.0,
                    &ctx.principal.principal_id.0,
                    thread_id,
                    page.cursor,
                    u32::try_from(page.limit + 1)
                        .expect("the bounded HTTP page limit fits a storage page"),
                ),
            )?
            .map_err(|_| EdgeError::Internal("workspace session history failed".into()))?;
        let ListWorkspaceSessionsOutcome::Page(mut sessions) = outcome else {
            return Err(EdgeError::BadRequest(
                "workspace session cursor does not identify this thread's history".into(),
            ));
        };
        let has_more = sessions.len() > page.limit;
        sessions.truncate(page.limit);
        let next_cursor = has_more
            .then(|| sessions.last().map(|session| session.session_id.clone()))
            .flatten();
        let items = sessions
            .iter()
            .map(|session| workspace_session_json(&ctx.principal.tenant.0, session))
            .collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next_cursor, page.limit),
        )))
    }
}

impl Handler for WorkspaceSshAccessCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body = parse_body(&ctx.request.body)?;
        let public_key_fingerprint = workspace_ssh_public_key_fingerprint(&body.public_key)
            .map_err(|_| {
                EdgeError::BadRequest(
                    "workspace SSH access requires one valid ephemeral Ed25519 public key".into(),
                )
            })?;
        let thread_id = thread_param(ctx)?;
        let thread = self.api.owner_thread(ctx.principal, thread_id)?;
        let now_reading = system_clock_reading().map_err(|_| {
            EdgeError::Unavailable("workspace SSH clock is temporarily unavailable".into())
        })?;
        let now = DateTime::from_timestamp(now_reading.unix_seconds(), 0).ok_or_else(|| {
            EdgeError::Unavailable("workspace SSH clock is temporarily unavailable".into())
        })?;
        let workspace_expires_at = parse_stored_timestamp(&thread.expires_at)?;
        if thread.state != AgentThreadState::Ready || workspace_expires_at <= now {
            return Err(EdgeError::Conflict(
                "agent thread workspace is not available for SSH access".into(),
            ));
        }
        let expires_at = workspace_ssh_grant_expiry(
            now,
            workspace_expires_at,
            ctx.identity.capability().expires_at_unix,
        )?;

        let grant_id = Uuid::new_v4();
        let route_username = self
            .api
            .routes
            .seal(&ctx.principal.tenant.0, grant_id)
            .map_err(|_| EdgeError::Internal("workspace SSH route creation failed".into()))?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let outcome = self
            .api
            .drive(self.api.threads.create_ssh_grant(
                &ctx.principal.tenant.0,
                NewWorkspaceSshGrant {
                    grant_id,
                    route_username,
                    thread_id,
                    owner_principal_id: ctx.principal.principal_id.0.clone(),
                    public_key_fingerprint,
                    client_nonce,
                    issued_at: now,
                    expires_at,
                },
            ))?
            .map_err(|_| EdgeError::Internal("workspace SSH grant storage failed".into()))?;
        let (grant, created) = match outcome {
            CreateWorkspaceSshGrantOutcome::Created(grant) => (grant, true),
            CreateWorkspaceSshGrantOutcome::Replayed(grant) => (grant, false),
            CreateWorkspaceSshGrantOutcome::Conflict => {
                return Err(EdgeError::Conflict(
                    "that idempotency key was already used for different SSH access".into(),
                ))
            }
            CreateWorkspaceSshGrantOutcome::ThreadUnavailable => {
                return Err(EdgeError::Conflict(
                    "agent thread changed while SSH access was being granted".into(),
                ))
            }
        };
        Ok(no_store(EdgeResponse::json(
            if created { 201 } else { 200 },
            &access_json(&self.api.endpoint, &grant, created),
        )))
    }
}

fn workspace_ssh_grant_expiry(
    now: DateTime<Utc>,
    workspace_expires_at: DateTime<Utc>,
    capability_expires_at_unix: i64,
) -> Result<DateTime<Utc>, EdgeError> {
    let capability_expires_at = DateTime::from_timestamp(capability_expires_at_unix, 0)
        .ok_or_else(|| EdgeError::Unauthorized("browser session has an invalid expiry".into()))?;
    let expires_at = (now + Duration::seconds(MAX_WORKSPACE_SSH_GRANT_SECONDS))
        .min(workspace_expires_at)
        .min(capability_expires_at);
    if expires_at <= now {
        return Err(EdgeError::Conflict(
            "browser-approved session expires before SSH access can begin".into(),
        ));
    }
    Ok(expires_at)
}

pub(crate) fn register_workspace_ssh_access(
    builder: GatewayBuilder,
    inputs: WorkspaceSshHttpInputs,
) -> GatewayBuilder {
    let api = WorkspaceSshHttpApi {
        threads: inputs.threads,
        routes: inputs.routes,
        endpoint: inputs.endpoint,
        runtime: inputs.runtime,
    };
    builder
        .route(
            Method::Post,
            "/v1/agent-threads/{thread}/ssh-access",
            "identity.agent_thread.ssh_access.create",
            Arc::new(WorkspaceSshAccessCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/agent-threads/{thread}/workspace-sessions",
            "identity.agent_thread.workspace_sessions.list",
            Arc::new(WorkspaceSessionListHandler { api }),
        )
}

fn workspace_session_json(tenant: &str, session: &DurableWorkspaceSession) -> Value {
    json!({
        "id": session.session_id,
        "ref": format!(
            "myelin://{tenant}/agent/session/{}",
            session.session_id
        ),
        "method": session.access_method,
        "mode": session.mode.token(),
        "terminal": session.terminal,
        "workspace": {
            "id": session.workspace_id,
            "generation": session.workspace_generation,
        },
        "started_at": session.started_at,
    })
}

fn canonical_ulid(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
}

fn access_json(
    endpoint: &WorkspaceSshEndpoint,
    grant: &DurableWorkspaceSshGrant,
    created: bool,
) -> Value {
    json!({
        "access": {
            "host": endpoint.host,
            "port": endpoint.port,
            "username": grant.route_username,
            "expires_at": grant.expires_at,
            "public_key_fingerprint": grant.public_key_fingerprint,
            "host_public_key": endpoint.host_public_key,
            "host_key_fingerprint": endpoint.host_key_fingerprint,
        },
        "workspace": {
            "id": grant.workspace_id,
            "generation": grant.workspace_generation,
        },
        "created": created,
    })
}

fn parse_body(body: &[u8]) -> Result<CreateWorkspaceSshAccessBody, EdgeError> {
    if body.is_empty() {
        return Err(EdgeError::BadRequest(
            "workspace SSH access request body is empty".into(),
        ));
    }
    if body.len() > MAX_AGENT_THREAD_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(
            "workspace SSH access request exceeds the interactive body limit".into(),
        ));
    }
    serde_json::from_slice(body).map_err(|error| {
        EdgeError::BadRequest(format!("invalid workspace SSH access request: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity_service::WorkspaceSshHostIdentity;
    use myelin_storage::SealKey;

    #[test]
    fn ssh_access_never_outlives_or_repairs_its_authority() {
        let now = DateTime::parse_from_rfc3339("2026-08-26T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let workspace_expires_at = now + Duration::days(3);

        let bounded = workspace_ssh_grant_expiry(
            now,
            workspace_expires_at,
            (now + Duration::seconds(90)).timestamp(),
        )
        .unwrap();
        assert_eq!(bounded, now + Duration::seconds(90));

        assert!(matches!(
            workspace_ssh_grant_expiry(now, workspace_expires_at, i64::MAX),
            Err(EdgeError::Unauthorized(_))
        ));
        assert!(matches!(
            workspace_ssh_grant_expiry(now, workspace_expires_at, now.timestamp()),
            Err(EdgeError::Conflict(_))
        ));
    }

    #[test]
    fn response_projects_only_connection_material_for_the_exact_workspace() {
        let host = WorkspaceSshHostIdentity::from_seal_key(&SealKey::from_bytes([0x41; 32]));
        let endpoint =
            WorkspaceSshEndpoint::new("ssh.myelin.example".into(), 22, host.public_key()).unwrap();
        let grant = DurableWorkspaceSshGrant {
            grant_id: "11111111-1111-4111-8111-111111111111".into(),
            route_username: "ws1_opaque-route-claim".into(),
            thread_id: "22222222-2222-4222-8222-222222222222".into(),
            owner_principal_id: "p:alice".into(),
            workspace_id: "33333333-3333-4333-8333-333333333333".into(),
            workspace_generation: 4,
            public_key_fingerprint: format!("SHA256:{}", "A".repeat(43)),
            issued_at: "2026-08-22T12:00:00Z".into(),
            expires_at: "2026-08-22T12:05:00Z".into(),
        };

        let response = access_json(&endpoint, &grant, true);
        assert_eq!(response["access"]["host"], "ssh.myelin.example");
        assert_eq!(response["access"]["port"], 22);
        assert_eq!(response["access"]["username"], grant.route_username);
        assert_eq!(response["workspace"]["id"], grant.workspace_id);
        assert_eq!(response["workspace"]["generation"], 4);
        assert_eq!(response["created"], true);
        assert_eq!(
            response["access"]["host_key_fingerprint"],
            host.fingerprint()
        );
        assert!(response.get("grant_id").is_none());
        assert!(response.get("owner_principal_id").is_none());
        assert!(response.get("private_key").is_none());
    }

    #[test]
    fn input_is_one_strict_bounded_public_key() {
        let host = WorkspaceSshHostIdentity::from_seal_key(&SealKey::from_bytes([0x42; 32]));
        let body = serde_json::to_vec(&json!({ "public_key": host.public_key() })).unwrap();
        let parsed = parse_body(&body).unwrap();
        assert_eq!(
            workspace_ssh_public_key_fingerprint(&parsed.public_key).unwrap(),
            host.fingerprint()
        );

        for body in [
            br#"{}"#.as_slice(),
            br#"{"public_key":"ssh-rsa AAAA"}"#,
            br#"{"public_key":"ssh-ed25519 AAAA","private_key":"never"}"#,
        ] {
            let refused = parse_body(body)
                .and_then(|body| {
                    workspace_ssh_public_key_fingerprint(&body.public_key)
                        .map_err(|_| EdgeError::BadRequest("invalid key".into()))
                })
                .is_err();
            assert!(
                refused,
                "unexpectedly accepted {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn session_history_projects_accountability_without_connection_material() {
        let session = DurableWorkspaceSession {
            session_id: "01J00000000000000000000001".into(),
            thread_id: "22222222-2222-4222-8222-222222222222".into(),
            owner_principal_id: "p:alice".into(),
            workspace_id: "33333333-3333-4333-8333-333333333333".into(),
            workspace_generation: 4,
            access_method: "ssh".into(),
            mode: myelin_storage::WorkspaceSessionMode::Command,
            terminal: true,
            started_at: "2026-08-22T12:00:00.000Z".into(),
        };

        let projected = workspace_session_json("acme", &session);
        assert_eq!(
            projected,
            json!({
                "id": session.session_id,
                "ref": format!(
                    "myelin://acme/agent/session/{}",
                    session.session_id
                ),
                "method": "ssh",
                "mode": "command",
                "terminal": true,
                "workspace": { "id": session.workspace_id, "generation": 4 },
                "started_at": session.started_at,
            })
        );
        myelin_refs::parse_scoped(projected["ref"].as_str().unwrap())
            .expect("workspace session history emits a canonical ArtifactRef");
        let body = projected.to_string();
        for sensitive_name in [
            "fingerprint",
            "grant",
            "host",
            "locator",
            "public_key",
            "route",
            "username",
        ] {
            assert!(!body.contains(sensitive_name));
        }
    }
}
