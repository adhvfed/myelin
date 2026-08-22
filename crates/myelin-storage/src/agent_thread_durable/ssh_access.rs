use chrono::{DateTime, Duration, SecondsFormat, Utc};
use sqlx::types::Uuid;
use sqlx::Row;

use super::{valid_client_nonce, DurableAgentThreadBacking};
use crate::{PgError, ProviderError, ACTIVE_PRINCIPAL_STATUS_JSON, HUMAN_PRINCIPAL_KIND_JSON};

pub const MAX_WORKSPACE_SSH_GRANT_SECONDS: i64 = 5 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewWorkspaceSshGrant {
    pub grant_id: Uuid,
    pub route_username: String,
    pub thread_id: Uuid,
    pub owner_principal_id: String,
    pub public_key_fingerprint: String,
    pub client_nonce: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableWorkspaceSshGrant {
    pub grant_id: String,
    pub route_username: String,
    pub thread_id: String,
    pub owner_principal_id: String,
    pub workspace_id: String,
    pub workspace_generation: u32,
    pub public_key_fingerprint: String,
    pub issued_at: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateWorkspaceSshGrantOutcome {
    Created(DurableWorkspaceSshGrant),
    Replayed(DurableWorkspaceSshGrant),
    Conflict,
    ThreadUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveWorkspaceSshAdmission {
    pub grant_id: String,
    pub thread_id: String,
    pub owner_principal_id: String,
    pub workspace_id: String,
    pub workspace_generation: u32,
    pub storage_locator: String,
    pub expires_at: String,
}

pub(super) struct LiveWorkspaceSshAuthorityRequest<'a> {
    pub tenant: &'a str,
    pub region: &'a str,
    pub grant_id: Uuid,
    pub route_username: &'a str,
    pub public_key_fingerprint: &'a str,
    pub admitted_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
}

impl DurableAgentThreadBacking {
    pub async fn create_ssh_grant(
        &self,
        tenant: &str,
        proposal: NewWorkspaceSshGrant,
    ) -> Result<CreateWorkspaceSshGrantOutcome, ProviderError> {
        validate_proposal(&proposal)?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    if let Some(existing) = by_nonce(
                        connection,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.client_nonce,
                    )
                    .await?
                    {
                        return Ok(if same_intent(&existing, &proposal) {
                            CreateWorkspaceSshGrantOutcome::Replayed(existing)
                        } else {
                            CreateWorkspaceSshGrantOutcome::Conflict
                        });
                    }

                    let row = sqlx::query(
                        "INSERT INTO agent_thread_ssh_grant (
                           tenant_id, region, grant_id, route_username, thread_id,
                           owner_principal_id, workspace_id, workspace_generation,
                           public_key_fingerprint, client_nonce, issued_at, expires_at
                         )
                         SELECT thread.tenant_id, thread.region, $4, $5, thread.thread_id,
                                thread.owner_principal_id, thread.workspace_id,
                                thread.workspace_generation, $6, $7, $8, $9
                           FROM agent_thread thread
                           JOIN principal owner
                             ON owner.tenant_id = thread.tenant_id
                            AND owner.region = thread.region
                            AND owner.principal_id = thread.owner_principal_id
                           JOIN principal agent
                             ON agent.tenant_id = thread.tenant_id
                            AND agent.region = thread.region
                            AND agent.principal_id = 'agent:' || thread.agent_id::text
                          WHERE thread.tenant_id = $1 AND thread.region = $2
                            AND thread.thread_id = $3 AND thread.owner_principal_id = $10
                            AND thread.state = 'ready' AND thread.expires_at >= $9
                            AND owner.kind = $11 AND owner.status = $12 AND agent.status = $12
                         ON CONFLICT DO NOTHING
                         RETURNING grant_id, route_username, thread_id, owner_principal_id,
                                   workspace_id, workspace_generation, public_key_fingerprint,
                                   issued_at, expires_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(proposal.thread_id)
                    .bind(proposal.grant_id)
                    .bind(&proposal.route_username)
                    .bind(&proposal.public_key_fingerprint)
                    .bind(&proposal.client_nonce)
                    .bind(proposal.issued_at)
                    .bind(proposal.expires_at)
                    .bind(&proposal.owner_principal_id)
                    .bind(HUMAN_PRINCIPAL_KIND_JSON)
                    .bind(ACTIVE_PRINCIPAL_STATUS_JSON)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error("create private workspace SSH grant"))?;
                    if let Some(row) = row {
                        return grant_from_row(&row).map(CreateWorkspaceSshGrantOutcome::Created);
                    }

                    if let Some(existing) = by_nonce(
                        connection,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.client_nonce,
                    )
                    .await?
                    {
                        return Ok(if same_intent(&existing, &proposal) {
                            CreateWorkspaceSshGrantOutcome::Replayed(existing)
                        } else {
                            CreateWorkspaceSshGrantOutcome::Conflict
                        });
                    }
                    Ok(CreateWorkspaceSshGrantOutcome::ThreadUnavailable)
                })
            })
            .await
    }

    pub async fn live_ssh_admission(
        &self,
        tenant: &str,
        grant_id: Uuid,
        route_username: &str,
        public_key_fingerprint: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<LiveWorkspaceSshAdmission>, ProviderError> {
        self.live_ssh_authority(
            tenant,
            grant_id,
            route_username,
            public_key_fingerprint,
            now,
            now,
        )
        .await
    }

    /// Rechecks an already admitted SSH session without turning the short-lived
    /// connection grant into the session lifetime.
    ///
    /// `admitted_at` is the instant at which the gateway accepted the key. The
    /// grant must have been valid then. The thread and its principals must still
    /// be live at `now`, and explicit grant revocation takes effect immediately.
    pub async fn live_ssh_session(
        &self,
        tenant: &str,
        grant_id: Uuid,
        route_username: &str,
        public_key_fingerprint: &str,
        admitted_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Option<LiveWorkspaceSshAdmission>, ProviderError> {
        if now < admitted_at {
            return Ok(None);
        }
        self.live_ssh_authority(
            tenant,
            grant_id,
            route_username,
            public_key_fingerprint,
            admitted_at,
            now,
        )
        .await
    }

    async fn live_ssh_authority(
        &self,
        tenant: &str,
        grant_id: Uuid,
        route_username: &str,
        public_key_fingerprint: &str,
        admitted_at: DateTime<Utc>,
        observed_at: DateTime<Utc>,
    ) -> Result<Option<LiveWorkspaceSshAdmission>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let route_username = route_username.to_string();
        let fingerprint = public_key_fingerprint.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    lookup_live_authority(
                        connection,
                        &LiveWorkspaceSshAuthorityRequest {
                            tenant: &tenant,
                            region: &region,
                            grant_id,
                            route_username: &route_username,
                            public_key_fingerprint: &fingerprint,
                            admitted_at,
                            observed_at,
                        },
                    )
                    .await
                })
            })
            .await
    }
}

pub(super) async fn lookup_live_authority(
    connection: &mut sqlx::PgConnection,
    request: &LiveWorkspaceSshAuthorityRequest<'_>,
) -> Result<Option<LiveWorkspaceSshAdmission>, PgError> {
    let row = sqlx::query(
        "SELECT access.grant_id, access.thread_id, access.owner_principal_id,
                access.workspace_id, access.workspace_generation,
                thread.storage_locator, access.expires_at
           FROM agent_thread_ssh_grant access
           JOIN agent_thread thread
             ON thread.tenant_id = access.tenant_id
            AND thread.region = access.region
            AND thread.thread_id = access.thread_id
           JOIN principal owner
             ON owner.tenant_id = thread.tenant_id
            AND owner.region = thread.region
            AND owner.principal_id = thread.owner_principal_id
           JOIN principal agent
             ON agent.tenant_id = thread.tenant_id
            AND agent.region = thread.region
            AND agent.principal_id = 'agent:' || thread.agent_id::text
          WHERE access.tenant_id = $1 AND access.region = $2
            AND access.grant_id = $3 AND access.route_username = $4
            AND access.public_key_fingerprint = $5
            AND access.revoked_at IS NULL
            AND access.issued_at <= $6 AND access.expires_at > $6
            AND thread.state = 'ready' AND thread.expires_at > $7
            AND access.owner_principal_id = thread.owner_principal_id
            AND access.workspace_id = thread.workspace_id
            AND access.workspace_generation = thread.workspace_generation
            AND owner.kind = $8 AND owner.status = $9 AND agent.status = $9
          FOR SHARE OF access, thread, owner, agent",
    )
    .bind(request.tenant)
    .bind(request.region)
    .bind(request.grant_id)
    .bind(request.route_username)
    .bind(request.public_key_fingerprint)
    .bind(request.admitted_at)
    .bind(request.observed_at)
    .bind(HUMAN_PRINCIPAL_KIND_JSON)
    .bind(ACTIVE_PRINCIPAL_STATUS_JSON)
    .fetch_optional(connection)
    .await
    .map_err(query_error("admit private workspace SSH key"))?;
    row.as_ref().map(admission_from_row).transpose()
}

fn validate_proposal(proposal: &NewWorkspaceSshGrant) -> Result<(), ProviderError> {
    let fingerprint_body = proposal
        .public_key_fingerprint
        .strip_prefix("SHA256:")
        .filter(|body| body.len() == 43)
        .filter(|body| {
            body.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
        });
    if fingerprint_body.is_none() {
        return Err(query("workspace SSH fingerprint is not canonical SHA-256"));
    }
    if !proposal.route_username.starts_with("ws1_")
        || proposal.route_username.len() < 16
        || proposal.route_username.len() > 384
        || !proposal
            .route_username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(query(
            "workspace SSH route username is not an opaque v1 claim",
        ));
    }
    if !valid_client_nonce(&proposal.client_nonce) {
        return Err(query(
            "workspace SSH client nonce is not URL-safe bounded text",
        ));
    }
    let latest_expiry = proposal
        .issued_at
        .checked_add_signed(Duration::seconds(MAX_WORKSPACE_SSH_GRANT_SECONDS))
        .ok_or_else(|| query("workspace SSH grant expiry overflowed"))?;
    if proposal.expires_at <= proposal.issued_at || proposal.expires_at > latest_expiry {
        return Err(query(
            "workspace SSH grant is outside its five-minute admission window",
        ));
    }
    Ok(())
}

fn same_intent(grant: &DurableWorkspaceSshGrant, proposal: &NewWorkspaceSshGrant) -> bool {
    grant.thread_id == proposal.thread_id.to_string()
        && grant.owner_principal_id == proposal.owner_principal_id
        && grant.public_key_fingerprint == proposal.public_key_fingerprint
}

async fn by_nonce(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    owner: &str,
    client_nonce: &str,
) -> Result<Option<DurableWorkspaceSshGrant>, PgError> {
    let row = sqlx::query(
        "SELECT grant_id, route_username, thread_id, owner_principal_id, workspace_id,
                workspace_generation, public_key_fingerprint, issued_at, expires_at
           FROM agent_thread_ssh_grant
          WHERE tenant_id = $1 AND region = $2 AND owner_principal_id = $3
            AND client_nonce = $4",
    )
    .bind(tenant)
    .bind(region)
    .bind(owner)
    .bind(client_nonce)
    .fetch_optional(connection)
    .await
    .map_err(query_error(
        "read private workspace SSH grant by retry identity",
    ))?;
    row.as_ref().map(grant_from_row).transpose()
}

fn grant_from_row(row: &sqlx::postgres::PgRow) -> Result<DurableWorkspaceSshGrant, PgError> {
    Ok(DurableWorkspaceSshGrant {
        grant_id: uuid(row, "grant_id")?,
        route_username: column(row, "route_username")?,
        thread_id: uuid(row, "thread_id")?,
        owner_principal_id: column(row, "owner_principal_id")?,
        workspace_id: uuid(row, "workspace_id")?,
        workspace_generation: generation(row)?,
        public_key_fingerprint: column(row, "public_key_fingerprint")?,
        issued_at: timestamp(row, "issued_at")?,
        expires_at: timestamp(row, "expires_at")?,
    })
}

fn admission_from_row(row: &sqlx::postgres::PgRow) -> Result<LiveWorkspaceSshAdmission, PgError> {
    Ok(LiveWorkspaceSshAdmission {
        grant_id: uuid(row, "grant_id")?,
        thread_id: uuid(row, "thread_id")?,
        owner_principal_id: column(row, "owner_principal_id")?,
        workspace_id: uuid(row, "workspace_id")?,
        workspace_generation: generation(row)?,
        storage_locator: column(row, "storage_locator")?,
        expires_at: timestamp(row, "expires_at")?,
    })
}

fn uuid(row: &sqlx::postgres::PgRow, name: &'static str) -> Result<String, PgError> {
    row.try_get::<Uuid, _>(name)
        .map(|value| value.to_string())
        .map_err(row_error(name))
}

fn column(row: &sqlx::postgres::PgRow, name: &'static str) -> Result<String, PgError> {
    row.try_get(name).map_err(row_error(name))
}

fn generation(row: &sqlx::postgres::PgRow) -> Result<u32, PgError> {
    let value = row
        .try_get::<i32, _>("workspace_generation")
        .map_err(row_error("workspace_generation"))?;
    u32::try_from(value).map_err(|_| PgError::Query("SSH grant has an invalid generation".into()))
}

fn timestamp(row: &sqlx::postgres::PgRow, name: &'static str) -> Result<String, PgError> {
    row.try_get::<DateTime<Utc>, _>(name)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
        .map_err(row_error(name))
}

fn query(message: &str) -> ProviderError {
    PgError::Query(message.into()).into()
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{operation}: {error}"))
}

fn row_error(column: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| {
        PgError::Query(format!(
            "decode private workspace SSH grant `{column}`: {error}"
        ))
    }
}
