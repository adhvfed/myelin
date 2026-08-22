use chrono::{DateTime, SecondsFormat, Utc};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;
use sqlx::Row;

use super::ssh_access::{lookup_live_authority, LiveWorkspaceSshAuthorityRequest};
use super::{canonical_ulid, DurableAgentThreadBacking, LiveWorkspaceSshAdmission};
use crate::pgrelay::PgRelay;
use crate::{PgError, ProviderError};

pub const WORKSPACE_SSH_SESSION_STARTED: &str = "workspace.session.started";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceSessionMode {
    Shell,
    Command,
}

impl WorkspaceSessionMode {
    pub fn token(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Command => "command",
        }
    }

    fn parse(value: &str) -> Result<Self, PgError> {
        match value {
            "shell" => Ok(Self::Shell),
            "command" => Ok(Self::Command),
            _ => Err(query_error_value("workspace session has an invalid mode")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewWorkspaceSshSession {
    pub session_id: String,
    pub grant_id: Uuid,
    pub route_username: String,
    pub public_key_fingerprint: String,
    pub admitted_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub mode: WorkspaceSessionMode,
    pub terminal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableWorkspaceSession {
    pub session_id: String,
    pub thread_id: String,
    pub owner_principal_id: String,
    pub workspace_id: String,
    pub workspace_generation: u32,
    pub access_method: String,
    pub mode: WorkspaceSessionMode,
    pub terminal: bool,
    pub started_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedWorkspaceSshSession {
    pub admission: LiveWorkspaceSshAdmission,
    pub session: DurableWorkspaceSession,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListWorkspaceSessionsOutcome {
    Page(Vec<DurableWorkspaceSession>),
    CursorNotFound,
}

impl DurableAgentThreadBacking {
    pub async fn start_ssh_session(
        &self,
        tenant: &str,
        proposal: NewWorkspaceSshSession,
    ) -> Result<Option<StartedWorkspaceSshSession>, ProviderError> {
        validate_proposal(&proposal)?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let Some(admission) = lookup_live_authority(
                        connection,
                        &LiveWorkspaceSshAuthorityRequest {
                            tenant: &tenant,
                            region: &region,
                            grant_id: proposal.grant_id,
                            route_username: &proposal.route_username,
                            public_key_fingerprint: &proposal.public_key_fingerprint,
                            admitted_at: proposal.admitted_at,
                            observed_at: proposal.started_at,
                        },
                    )
                    .await?
                    else {
                        return Ok(None);
                    };

                    let row = sqlx::query(
                        "INSERT INTO agent_thread_workspace_session (
                           tenant_id, region, session_id, grant_id, thread_id,
                           owner_principal_id, workspace_id, workspace_generation,
                           access_method, session_mode, terminal, started_at
                         )
                         SELECT access.tenant_id, access.region, $4, access.grant_id,
                                access.thread_id, access.owner_principal_id, access.workspace_id,
                                access.workspace_generation, 'ssh', $5, $6, $7
                           FROM agent_thread_ssh_grant access
                          WHERE access.tenant_id = $1 AND access.region = $2
                            AND access.grant_id = $3
                         RETURNING session_id, thread_id, owner_principal_id, workspace_id,
                                   workspace_generation, access_method, session_mode, terminal,
                                   started_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(proposal.grant_id)
                    .bind(&proposal.session_id)
                    .bind(proposal.mode.token())
                    .bind(proposal.terminal)
                    .bind(proposal.started_at)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(query_error("record private workspace session"))?;
                    let session = session_from_row(&row)?;
                    let event = session_started_event(&tenant, &region, &session);
                    PgRelay::co_commit_in_tx(connection, &event.aggregate.0, &event).await?;
                    Ok(Some(StartedWorkspaceSshSession { admission, session }))
                })
            })
            .await
    }

    pub async fn list_workspace_sessions_for_owner(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
        before: Option<String>,
        limit: u32,
    ) -> Result<ListWorkspaceSessionsOutcome, ProviderError> {
        if limit == 0 || limit > 101 {
            return Err(query_error_value(
                "workspace session page limit must be between 1 and 101",
            )
            .into());
        }
        if before
            .as_deref()
            .is_some_and(|value| !canonical_ulid(value))
        {
            return Err(
                query_error_value("workspace session cursor is not a canonical ULID").into(),
            );
        }
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    if let Some(cursor) = before.as_deref() {
                        let exists = sqlx::query_scalar::<_, bool>(
                            "SELECT EXISTS (
                               SELECT 1 FROM agent_thread_workspace_session
                                WHERE tenant_id = $1 AND region = $2
                                  AND owner_principal_id = $3 AND thread_id = $4
                                  AND session_id = $5
                             )",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&owner)
                        .bind(thread_id)
                        .bind(cursor)
                        .fetch_one(&mut *connection)
                        .await
                        .map_err(query_error("resolve workspace session cursor"))?;
                        if !exists {
                            return Ok(ListWorkspaceSessionsOutcome::CursorNotFound);
                        }
                    }
                    let rows = sqlx::query(
                        "SELECT session_id, thread_id, owner_principal_id, workspace_id,
                                workspace_generation, access_method, session_mode, terminal,
                                started_at
                           FROM agent_thread_workspace_session
                          WHERE tenant_id = $1 AND region = $2 AND owner_principal_id = $3
                            AND thread_id = $4
                            AND ($5::text IS NULL OR session_id < $5)
                          ORDER BY session_id DESC LIMIT $6",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&owner)
                    .bind(thread_id)
                    .bind(before)
                    .bind(i64::from(limit))
                    .fetch_all(&mut *connection)
                    .await
                    .map_err(query_error("list private workspace sessions"))?;
                    let sessions = rows
                        .iter()
                        .map(session_from_row)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(ListWorkspaceSessionsOutcome::Page(sessions))
                })
            })
            .await
    }
}

fn validate_proposal(proposal: &NewWorkspaceSshSession) -> Result<(), ProviderError> {
    if !canonical_ulid(&proposal.session_id) {
        return Err(query_error_value("workspace session id is not a canonical ULID").into());
    }
    if proposal.started_at < proposal.admitted_at {
        return Err(query_error_value("workspace session started before SSH admission").into());
    }
    Ok(())
}

fn session_started_event(
    tenant: &str,
    region: &str,
    session: &DurableWorkspaceSession,
) -> EventEnvelope {
    let workspace_ref = format!("myelin://{tenant}/agent/workspace/{}", session.workspace_id);
    let mut actor = Principal::stub(
        PrincipalId(session.owner_principal_id.clone()),
        PrincipalKind::Human,
        TenantId(tenant.to_string()),
    );
    actor.region = Region(region.to_string());
    EventEnvelope {
        event_id: EventId(session.session_id.clone()),
        type_: EventType(WORKSPACE_SSH_SESSION_STARTED.into()),
        schema_ver: 1,
        tenant: TenantId(tenant.to_string()),
        region: Region(region.to_string()),
        actor: Actor(actor),
        subject: ArtifactRef(workspace_ref.clone()),
        aggregate: AggregateKey(format!(
            "agent-workspace:{}:{}",
            session.workspace_id, session.workspace_generation
        )),
        causation_id: None,
        correlation_id: CorrelationId(session.session_id.clone()),
        caused_by: Some(CausedBy(format!(
            "workspace:ssh-session:{}",
            session.session_id
        ))),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp(session.started_at.clone()),
        recorded_at: Timestamp(session.started_at.clone()),
        payload: serde_json::json!({
            "thread_ref": format!("myelin://{tenant}/agent/thread/{}", session.thread_id),
            "workspace_ref": workspace_ref,
            "workspace_generation": session.workspace_generation,
            "method": session.access_method,
            "mode": session.mode.token(),
            "terminal": session.terminal,
        }),
    }
}

fn session_from_row(row: &sqlx::postgres::PgRow) -> Result<DurableWorkspaceSession, PgError> {
    let generation = row
        .try_get::<i32, _>("workspace_generation")
        .map_err(row_error("workspace_generation"))?;
    Ok(DurableWorkspaceSession {
        session_id: row.try_get("session_id").map_err(row_error("session_id"))?,
        thread_id: row
            .try_get::<Uuid, _>("thread_id")
            .map(|value| value.to_string())
            .map_err(row_error("thread_id"))?,
        owner_principal_id: row
            .try_get("owner_principal_id")
            .map_err(row_error("owner_principal_id"))?,
        workspace_id: row
            .try_get::<Uuid, _>("workspace_id")
            .map(|value| value.to_string())
            .map_err(row_error("workspace_id"))?,
        workspace_generation: u32::try_from(generation)
            .map_err(|_| query_error_value("workspace session has an invalid generation"))?,
        access_method: row
            .try_get("access_method")
            .map_err(row_error("access_method"))?,
        mode: WorkspaceSessionMode::parse(
            row.try_get::<String, _>("session_mode")
                .map_err(row_error("session_mode"))?
                .as_str(),
        )?,
        terminal: row.try_get("terminal").map_err(row_error("terminal"))?,
        started_at: row
            .try_get::<DateTime<Utc>, _>("started_at")
            .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
            .map_err(row_error("started_at"))?,
    })
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{operation}: {error}"))
}

fn row_error(column: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| {
        PgError::Query(format!(
            "decode private workspace session `{column}`: {error}"
        ))
    }
}

fn query_error_value(message: &str) -> PgError {
    PgError::Query(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_access_event_is_canonical_and_minimized() {
        let session = DurableWorkspaceSession {
            session_id: "01J00000000000000000000001".into(),
            thread_id: Uuid::from_u128(1).to_string(),
            owner_principal_id: "p:alice".into(),
            workspace_id: Uuid::from_u128(2).to_string(),
            workspace_generation: 3,
            access_method: "ssh".into(),
            mode: WorkspaceSessionMode::Command,
            terminal: false,
            started_at: "2026-08-22T12:00:00.000Z".into(),
        };

        let event = session_started_event("acme", "eu-west", &session);
        assert_eq!(event.type_.0, WORKSPACE_SSH_SESSION_STARTED);
        assert!(myelin_events::validate_event_type(&event.type_.0).is_ok());
        assert_eq!(
            event.subject.0,
            format!("myelin://acme/agent/workspace/{}", session.workspace_id)
        );
        let payload = event.payload.to_string();
        for sensitive_name in [
            "fingerprint",
            "grant",
            "host",
            "locator",
            "public_key",
            "route",
            "username",
        ] {
            assert!(!payload.contains(sensitive_name));
        }
    }
}
