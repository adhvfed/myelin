use chrono::{DateTime, Duration, Timelike, Utc};
use myelin_identity::Principal;
use myelin_identity_service::workspace_ssh_public_key_fingerprint;
use myelin_storage::{
    AgentThreadState, CreateWorkspaceSshGrantOutcome, DurableAgentThread,
    DurableAgentThreadBacking, DurableWorkspaceSshGrant, NewWorkspaceSshGrant,
    WorkspaceSshRouteKey, MAX_WORKSPACE_SSH_GRANT_SECONDS,
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
use crate::catalogue::{Handler, HandlerCtx};
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
        let now = Utc::now()
            .with_nanosecond(0)
            .expect("zero nanoseconds is a valid timestamp");
        let workspace_expires_at = parse_stored_timestamp(&thread.expires_at)?;
        if thread.state != AgentThreadState::Ready || workspace_expires_at <= now {
            return Err(EdgeError::Conflict(
                "agent thread workspace is not available for SSH access".into(),
            ));
        }
        let browser_expires_at =
            DateTime::from_timestamp(ctx.identity.capability().expires_at_unix, 0)
                .unwrap_or(DateTime::<Utc>::MAX_UTC);
        let expires_at = (now + Duration::seconds(MAX_WORKSPACE_SSH_GRANT_SECONDS))
            .min(workspace_expires_at)
            .min(browser_expires_at);
        if expires_at <= now {
            return Err(EdgeError::Conflict(
                "browser-approved session expires before SSH access can begin".into(),
            ));
        }

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
    builder.route(
        Method::Post,
        "/v1/agent-threads/{thread}/ssh-access",
        "identity.agent_thread.ssh_access.create",
        Arc::new(WorkspaceSshAccessCreateHandler { api }),
    )
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
}
