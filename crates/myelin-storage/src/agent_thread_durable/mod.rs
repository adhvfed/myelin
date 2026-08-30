mod expiry;
mod model;
mod run;
mod schema;
mod ssh_access;
mod ssh_route;
mod workspace_session;

pub use expiry::{
    AgentThreadExpiry, AgentThreadExpiryCompletion, AgentThreadExpiryFailure,
    AGENT_THREAD_EXPIRY_GRACE_SECONDS,
};
pub use model::{
    ActivateAgentThreadOutcome, AgentThreadGitBinding, AgentThreadProvisioningFailure,
    AgentThreadState, CreateAgentThreadOutcome, DurableAgentThread, NewAgentThread,
    RetryAgentThreadOutcome, MAX_AGENT_THREAD_GIT_REF_BYTES, MAX_AGENT_THREAD_NAME_BYTES,
    MAX_AGENT_THREAD_REPOSITORY_BYTES, MAX_AGENT_THREAD_RETENTION_DAYS,
    MIN_AGENT_THREAD_RETENTION_DAYS,
};
pub use run::{AgentThreadRunBinding, BindAgentThreadRunOutcome};
pub use schema::{
    agent_thread_durable_migrations, agent_thread_git_binding_migrations,
    agent_thread_publication_migrations, agent_thread_recent_list_migrations,
    agent_thread_session_launch_migrations, agent_thread_ssh_single_use_migrations,
    AGENT_THREAD_MIGRATION, AGENT_THREAD_PUBLICATION_BACKFILL_MIGRATION,
    AGENT_THREAD_PUBLICATION_EXPAND_MIGRATION, AGENT_THREAD_PUBLICATION_INVARIANT_MIGRATION,
    AGENT_THREAD_PUBLICATION_RECOVERY_INDEX, AGENT_THREAD_PUBLICATION_RECOVERY_INDEX_MIGRATION,
    AGENT_THREAD_RECENT_LIST_INDEX_MIGRATION, AGENT_THREAD_RLS_POLICY, AGENT_THREAD_RUN_MIGRATION,
    AGENT_THREAD_SESSION_LAUNCH_BACKFILL_MIGRATION, AGENT_THREAD_SESSION_LAUNCH_EXPAND_MIGRATION,
    AGENT_THREAD_SESSION_LAUNCH_INDEX_MIGRATION, AGENT_THREAD_SESSION_LAUNCH_INVARIANT_MIGRATION,
    AGENT_THREAD_SSH_GRANT_MIGRATION, AGENT_THREAD_SSH_SINGLE_USE_MIGRATION,
    AGENT_THREAD_WORKSPACE_SESSION_MIGRATION,
};
pub use ssh_access::{
    CreateWorkspaceSshGrantOutcome, DurableWorkspaceSshGrant, LiveWorkspaceSshAdmission,
    NewWorkspaceSshGrant, MAX_WORKSPACE_SSH_GRANT_SECONDS,
};
pub use ssh_route::{WorkspaceSshRoute, WorkspaceSshRouteError, WorkspaceSshRouteKey};
pub use workspace_session::{
    DurableWorkspaceSession, ListWorkspaceSessionsOutcome, NewWorkspaceSshSession,
    ReservedWorkspaceSshSession, WorkspaceSessionMode, WORKSPACE_SSH_SESSION_STARTED,
};

use chrono::{DateTime, Duration, Utc};
use sqlx::types::Uuid;
use sqlx::Row;

use crate::{
    PgError, ProviderError, SubstrateProvider, ACTIVE_PRINCIPAL_STATUS_JSON,
    HUMAN_PRINCIPAL_KIND_JSON,
};

const RESUMABLE_AGENT_RUNTIME_REF: &str = "external:mcp";
const EXACT_AGENT_THREAD_BATCH_MAX: usize = 10_000;
const MAX_AGENT_THREAD_RECOVERY_BATCH: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListAgentThreadsOutcome {
    Page(Vec<DurableAgentThread>),
    CursorNotFound,
}

#[derive(Clone)]
pub struct DurableAgentThreadBacking {
    provider: SubstrateProvider,
}

impl DurableAgentThreadBacking {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self { provider }
    }

    pub async fn create(
        &self,
        tenant: &str,
        proposal: NewAgentThread,
    ) -> Result<CreateAgentThreadOutcome, ProviderError> {
        validate_proposal(&proposal)?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    if let Some(existing) = by_nonce(
                        conn,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.client_nonce,
                    )
                    .await?
                    {
                        if !same_intent(&existing, &proposal) {
                            return Ok(CreateAgentThreadOutcome::Conflict);
                        }
                        if existing.state != AgentThreadState::Failed {
                            return Ok(CreateAgentThreadOutcome::Replayed(existing));
                        }
                        let restarted = sqlx::query(
                            "UPDATE agent_thread
                                SET state = 'provisioning', failure_reason = NULL,
                                    updated_at = CURRENT_TIMESTAMP
                              WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                                AND owner_principal_id = $4 AND state = 'failed'
                            RETURNING tenant_id, region, thread_id, owner_principal_id, agent_id,
                              conversation_id, workspace_id, workspace_generation, name, project_id,
                              repository_slug, base_ref, base_oid, workspace_branch,
                              retention_days, state, storage_locator, publication_completed_at,
                              failure_reason, created_at,
                              expires_at, updated_at",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(Uuid::parse_str(&existing.thread_id).map_err(|_| {
                            PgError::Query("stored agent thread id is invalid".into())
                        })?)
                        .bind(&proposal.owner_principal_id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(query_error("restart failed agent thread provisioning"))?;
                        let restarted = restarted
                            .as_ref()
                            .map(row_to_thread)
                            .transpose()?
                            .unwrap_or(existing);
                        return Ok(CreateAgentThreadOutcome::Replayed(restarted));
                    }

                    let owner = sqlx::query_as::<_, (String, String)>(
                        "SELECT kind, status FROM principal
                          WHERE tenant_id = $1 AND region = $2 AND principal_id = $3
                          FOR UPDATE",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&proposal.owner_principal_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("verify agent thread owner"))?;
                    if !owner.is_some_and(|(kind, status)| {
                        kind == HUMAN_PRINCIPAL_KIND_JSON && status == ACTIVE_PRINCIPAL_STATUS_JSON
                    }) {
                        return Ok(CreateAgentThreadOutcome::OwnerUnavailable);
                    }

                    let agent = sqlx::query_as::<_, (String, String)>(
                        "SELECT principal.status, agent.runtime_ref FROM identity_agent agent
                           JOIN principal
                             ON principal.tenant_id = agent.tenant_id
                            AND principal.region = agent.region
                            AND principal.principal_id = 'agent:' || agent.agent_id::text
                          WHERE agent.tenant_id = $1 AND agent.region = $2
                            AND agent.agent_id = $3 AND agent.created_by = $4
                          FOR UPDATE OF principal",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(proposal.agent_id)
                    .bind(&proposal.owner_principal_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("verify agent thread agent"))?;
                    let Some((agent_status, runtime_ref)) = agent else {
                        return Ok(CreateAgentThreadOutcome::AgentUnavailable);
                    };
                    if agent_status != ACTIVE_PRINCIPAL_STATUS_JSON {
                        return Ok(CreateAgentThreadOutcome::AgentUnavailable);
                    }
                    if runtime_ref != RESUMABLE_AGENT_RUNTIME_REF {
                        return Ok(CreateAgentThreadOutcome::AgentRuntimeUnsupported);
                    }

                    if live_name_exists(
                        conn,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.name,
                    )
                    .await?
                    {
                        return Ok(CreateAgentThreadOutcome::NameConflict);
                    }

                    let row = sqlx::query(
                        "INSERT INTO agent_thread (
                           tenant_id, region, thread_id, owner_principal_id, agent_id,
                           conversation_id, workspace_id, workspace_generation, name, project_id,
                           repository_slug, base_ref, base_oid, workspace_branch,
                           retention_days, client_nonce, state, created_at, expires_at, updated_at
                         ) VALUES ($1,$2,$3,$4,$5,$6,$7,1,$8,$9,$10,$11,$12,$13,$14,$15,'provisioning',$16,$17,$16)
                         ON CONFLICT DO NOTHING
                         RETURNING tenant_id, region, thread_id, owner_principal_id, agent_id,
                           conversation_id, workspace_id, workspace_generation, name, project_id,
                           repository_slug, base_ref, base_oid, workspace_branch,
                           retention_days, state, storage_locator, publication_completed_at,
                           failure_reason, created_at,
                           expires_at, updated_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(proposal.thread_id)
                    .bind(&proposal.owner_principal_id)
                    .bind(proposal.agent_id)
                    .bind(&proposal.conversation_id)
                    .bind(proposal.workspace_id)
                    .bind(&proposal.name)
                    .bind(proposal.project_id)
                    .bind(proposal.git.as_ref().map(|git| git.repository.as_str()))
                    .bind(proposal.git.as_ref().map(|git| git.base_ref.as_str()))
                    .bind(proposal.git.as_ref().map(|git| git.base_oid.as_str()))
                    .bind(proposal.git.as_ref().map(|git| git.branch.as_str()))
                    .bind(proposal.retention_days)
                    .bind(&proposal.client_nonce)
                    .bind(proposal.created_at)
                    .bind(proposal.expires_at)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("create agent thread"))?;
                    if let Some(row) = row {
                        return row_to_thread(&row).map(CreateAgentThreadOutcome::Created);
                    }

                    if let Some(existing) = by_nonce(
                        conn,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.client_nonce,
                    )
                    .await?
                    {
                        return Ok(if same_intent(&existing, &proposal) {
                            CreateAgentThreadOutcome::Replayed(existing)
                        } else {
                            CreateAgentThreadOutcome::Conflict
                        });
                    }
                    Ok(CreateAgentThreadOutcome::NameConflict)
                })
            })
            .await
    }

    pub async fn activate(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
        workspace_id: Uuid,
        conversation_id: &str,
        storage_locator: &str,
    ) -> Result<ActivateAgentThreadOutcome, ProviderError> {
        validate_storage_locator(storage_locator)?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        let conversation = conversation_id.to_string();
        let locator = storage_locator.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let current = by_id(conn, &tenant, &region, thread_id).await?;
                    let Some(current) = current else {
                        return Ok(ActivateAgentThreadOutcome::NotFound);
                    };
                    if current.owner_principal_id != owner
                        || current.workspace_id != workspace_id.to_string()
                        || current.conversation_id != conversation
                    {
                        return Ok(ActivateAgentThreadOutcome::NotFound);
                    }
                    if current.state == AgentThreadState::Ready {
                        return Ok(
                            if current.storage_locator.as_deref() == Some(locator.as_str()) {
                                ActivateAgentThreadOutcome::AlreadyReady(current)
                            } else {
                                ActivateAgentThreadOutcome::Conflict
                            },
                        );
                    }
                    if current.state != AgentThreadState::Provisioning {
                        return Ok(ActivateAgentThreadOutcome::Conflict);
                    }
                    let row = sqlx::query(
                        "UPDATE agent_thread
                            SET state = 'ready', storage_locator = $6, failure_reason = NULL,
                                updated_at = CURRENT_TIMESTAMP
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                            AND owner_principal_id = $4 AND workspace_id = $5
                            AND conversation_id = $7 AND state = 'provisioning'
                        RETURNING tenant_id, region, thread_id, owner_principal_id, agent_id,
                          conversation_id, workspace_id, workspace_generation, name, project_id,
                          repository_slug, base_ref, base_oid, workspace_branch,
                          retention_days, state, storage_locator, publication_completed_at,
                          failure_reason, created_at,
                          expires_at, updated_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(thread_id)
                    .bind(&owner)
                    .bind(workspace_id)
                    .bind(&locator)
                    .bind(&conversation)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("activate agent thread"))?;
                    row.as_ref()
                        .map(row_to_thread)
                        .transpose()?
                        .map(ActivateAgentThreadOutcome::Activated)
                        .ok_or_else(|| {
                            PgError::Query("agent thread activation lost its locked row".into())
                        })
                })
            })
            .await
    }

    pub async fn record_provisioning_failure(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
        failure: AgentThreadProvisioningFailure,
        observed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let changed = sqlx::query(
                        "UPDATE agent_thread
                            SET state = 'failed', failure_reason = $5, updated_at = $6
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                            AND owner_principal_id = $4 AND state = 'provisioning'",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(thread_id)
                    .bind(&owner)
                    .bind(failure.token())
                    .bind(observed_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("record agent thread provisioning failure"))?
                    .rows_affected();
                    Ok(changed == 1)
                })
            })
            .await
    }

    pub async fn complete_publication(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
        conversation_id: &str,
        completed_at: DateTime<Utc>,
    ) -> Result<Option<DurableAgentThread>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        let conversation = conversation_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "UPDATE agent_thread
                            SET publication_completed_at = COALESCE(publication_completed_at, $6),
                                updated_at = CASE WHEN publication_completed_at IS NULL
                                  THEN GREATEST(updated_at, $6) ELSE updated_at END
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                            AND owner_principal_id = $4 AND conversation_id = $5
                            AND state = 'ready' AND storage_locator IS NOT NULL
                        RETURNING tenant_id, region, thread_id, owner_principal_id, agent_id,
                          conversation_id, workspace_id, workspace_generation, name, project_id,
                          repository_slug, base_ref, base_oid, workspace_branch,
                          retention_days, state, storage_locator, publication_completed_at,
                          failure_reason, created_at, expires_at, updated_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(thread_id)
                    .bind(&owner)
                    .bind(&conversation)
                    .bind(completed_at)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("complete agent thread publication"))?;
                    row.as_ref().map(row_to_thread).transpose()
                })
            })
            .await
    }

    pub async fn claim_provisioning_recovery(
        &self,
        tenant: &str,
        stale_before: DateTime<Utc>,
        claimed_at: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<DurableAgentThread>, ProviderError> {
        if !(1..=MAX_AGENT_THREAD_RECOVERY_BATCH).contains(&limit) {
            return Err(query(
                "agent thread recovery limit must be between 1 and 100",
            ));
        }
        if stale_before > claimed_at {
            return Err(query(
                "agent thread recovery lease boundary is in the future",
            ));
        }
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "WITH recoverable AS (
                           SELECT thread_id
                             FROM agent_thread
                            WHERE tenant_id = $1 AND region = $2
                              AND expires_at > $4 AND updated_at <= $3
                              AND (state IN ('provisioning', 'failed')
                                OR (state = 'ready' AND publication_completed_at IS NULL))
                            ORDER BY updated_at, thread_id
                            LIMIT $5
                            FOR UPDATE SKIP LOCKED
                         )
                         UPDATE agent_thread thread
                            SET state = CASE WHEN thread.state = 'failed'
                                             THEN 'provisioning' ELSE thread.state END,
                                failure_reason = NULL, updated_at = $4
                           FROM recoverable
                          WHERE thread.tenant_id = $1 AND thread.region = $2
                            AND thread.thread_id = recoverable.thread_id
                        RETURNING thread.tenant_id, thread.region, thread.thread_id,
                          thread.owner_principal_id, thread.agent_id, thread.conversation_id,
                          thread.workspace_id, thread.workspace_generation, thread.name,
                          thread.project_id, thread.repository_slug, thread.base_ref,
                          thread.base_oid, thread.workspace_branch, thread.retention_days, thread.state,
                          thread.storage_locator, thread.publication_completed_at,
                          thread.failure_reason, thread.created_at, thread.expires_at,
                          thread.updated_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(stale_before)
                    .bind(claimed_at)
                    .bind(i64::from(limit))
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(query_error("claim agent thread provisioning recovery"))?;
                    rows.iter().map(row_to_thread).collect()
                })
            })
            .await
    }

    /// Reopens one owner's durable provisioning receipt without requiring the
    /// original request nonce. The thread, conversation, and workspace
    /// identities remain unchanged, so a response-lost retry is the same
    /// provisioning operation rather than a replacement thread.
    pub async fn retry_provisioning(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
        observed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<RetryAgentThreadOutcome, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let current = by_id(conn, &tenant, &region, thread_id).await?;
                    let Some(current) = current else {
                        return Ok(RetryAgentThreadOutcome::NotFound);
                    };
                    if current.owner_principal_id != owner {
                        return Ok(RetryAgentThreadOutcome::NotFound);
                    }
                    let expires_at = DateTime::parse_from_rfc3339(&current.expires_at)
                        .map_err(|_| PgError::Query("stored agent thread expiry is invalid".into()))?
                        .with_timezone(&Utc);
                    if expires_at <= observed_at {
                        return Ok(RetryAgentThreadOutcome::Unavailable);
                    }
                    match current.state {
                        AgentThreadState::Ready => {
                            Ok(RetryAgentThreadOutcome::AlreadyReady(current))
                        }
                        AgentThreadState::Provisioning => {
                            Ok(RetryAgentThreadOutcome::Retrying(current))
                        }
                        AgentThreadState::Failed => {
                            let row = sqlx::query(
                                "UPDATE agent_thread
                                    SET state = 'provisioning', failure_reason = NULL,
                                        updated_at = $5
                                  WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                                    AND owner_principal_id = $4 AND state = 'failed'
                                RETURNING tenant_id, region, thread_id, owner_principal_id, agent_id,
                                  conversation_id, workspace_id, workspace_generation, name, project_id,
                                  repository_slug, base_ref, base_oid, workspace_branch,
                                  retention_days, state, storage_locator, publication_completed_at,
                                  failure_reason, created_at,
                                  expires_at, updated_at",
                            )
                            .bind(&tenant)
                            .bind(&region)
                            .bind(thread_id)
                            .bind(&owner)
                            .bind(observed_at)
                            .fetch_optional(&mut *conn)
                            .await
                            .map_err(query_error("retry failed agent thread provisioning"))?;
                            row.as_ref()
                                .map(row_to_thread)
                                .transpose()?
                                .map(RetryAgentThreadOutcome::Retrying)
                                .ok_or_else(|| {
                                    PgError::Query(
                                        "agent thread retry lost its locked receipt".into(),
                                    )
                                })
                        }
                        AgentThreadState::Expiring | AgentThreadState::Deleted => {
                            Ok(RetryAgentThreadOutcome::Unavailable)
                        }
                    }
                })
            })
            .await
    }

    pub async fn get_for_owner(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
    ) -> Result<Option<DurableAgentThread>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    Ok(by_id(conn, &tenant, &region, thread_id)
                        .await?
                        .filter(|thread| thread.owner_principal_id == owner))
                })
            })
            .await
    }

    pub async fn get_for_owner_by_nonce(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        client_nonce: &str,
    ) -> Result<Option<DurableAgentThread>, ProviderError> {
        if !valid_client_nonce(client_nonce) {
            return Err(query(
                "agent thread client nonce is not URL-safe bounded text",
            ));
        }
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        let nonce = client_nonce.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move { by_nonce(conn, &tenant, &region, &owner, &nonce).await })
            })
            .await
    }

    pub async fn list_for_owner(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        before: Option<Uuid>,
        limit: u32,
    ) -> Result<ListAgentThreadsOutcome, ProviderError> {
        if limit == 0 || limit > 101 {
            return Err(query("agent thread page limit must be between 1 and 101"));
        }
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    if let Some(cursor) = before {
                        let exists = sqlx::query_scalar::<_, bool>(
                            "SELECT EXISTS (
                               SELECT 1 FROM agent_thread
                                WHERE tenant_id = $1 AND region = $2
                                  AND owner_principal_id = $3 AND thread_id = $4
                             )",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&owner)
                        .bind(cursor)
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(query_error("resolve agent thread cursor"))?;
                        if !exists {
                            return Ok(ListAgentThreadsOutcome::CursorNotFound);
                        }
                    }
                    let rows = sqlx::query(
                        "SELECT tenant_id, region, thread_id, owner_principal_id, agent_id,
                           conversation_id, workspace_id, workspace_generation, name, project_id,
                           repository_slug, base_ref, base_oid, workspace_branch,
                           retention_days, state, storage_locator, publication_completed_at,
                           failure_reason, created_at,
                           expires_at, updated_at
                        FROM agent_thread
                        WHERE tenant_id = $1 AND region = $2 AND owner_principal_id = $3
                          AND state <> 'deleted'
                          AND ($4::uuid IS NULL OR (created_at, conversation_id, thread_id) < (
                            SELECT cursor.created_at, cursor.conversation_id, cursor.thread_id
                              FROM agent_thread cursor
                             WHERE cursor.tenant_id = $1 AND cursor.region = $2
                               AND cursor.owner_principal_id = $3 AND cursor.thread_id = $4
                          ))
                        ORDER BY created_at DESC, conversation_id DESC, thread_id DESC LIMIT $5",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&owner)
                    .bind(before)
                    .bind(i64::from(limit))
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(query_error("list agent threads"))?;
                    rows.iter()
                        .map(row_to_thread)
                        .collect::<Result<Vec<_>, _>>()
                        .map(ListAgentThreadsOutcome::Page)
                })
            })
            .await
    }

    /// Reads one exact bounded set for an owner without turning reference projection into an N+1
    /// lookup. Deleted rows remain available through `get_for_owner` as lifecycle receipts, but
    /// are deliberately absent here because their workspaces can no longer be resumed.
    pub async fn get_live_exact_for_owner(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_ids: &[Uuid],
    ) -> Result<Vec<DurableAgentThread>, ProviderError> {
        let thread_ids = bounded_exact_thread_batch(thread_ids)?;
        if thread_ids.is_empty() {
            return Ok(Vec::new());
        }
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT tenant_id, region, thread_id, owner_principal_id, agent_id,
                           conversation_id, workspace_id, workspace_generation, name, project_id,
                           repository_slug, base_ref, base_oid, workspace_branch,
                           retention_days, state, storage_locator, publication_completed_at,
                           failure_reason, created_at,
                           expires_at, updated_at
                         FROM agent_thread
                        WHERE tenant_id = $1 AND region = $2 AND owner_principal_id = $3
                          AND thread_id = ANY($4::uuid[]) AND state <> 'deleted'
                        ORDER BY thread_id",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&owner)
                    .bind(thread_ids)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(query_error("read exact live agent threads"))?;
                    rows.iter().map(row_to_thread).collect()
                })
            })
            .await
    }
}

fn bounded_exact_thread_batch(thread_ids: &[Uuid]) -> Result<Vec<Uuid>, ProviderError> {
    if thread_ids.len() > EXACT_AGENT_THREAD_BATCH_MAX {
        return Err(query(&format!(
            "at most {EXACT_AGENT_THREAD_BATCH_MAX} agent threads may be read at once"
        )));
    }
    let mut thread_ids = thread_ids.to_vec();
    thread_ids.sort_unstable();
    thread_ids.dedup();
    Ok(thread_ids)
}

fn validate_proposal(proposal: &NewAgentThread) -> Result<(), ProviderError> {
    if proposal.name.trim() != proposal.name
        || proposal.name.is_empty()
        || proposal.name.len() > MAX_AGENT_THREAD_NAME_BYTES
        || proposal.name.chars().any(char::is_control)
    {
        return Err(query("agent thread name is not clean bounded text"));
    }
    if !(MIN_AGENT_THREAD_RETENTION_DAYS..=MAX_AGENT_THREAD_RETENTION_DAYS)
        .contains(&proposal.retention_days)
    {
        return Err(query("agent thread retention is outside its durable bound"));
    }
    if !canonical_ulid(&proposal.conversation_id) {
        return Err(query(
            "agent thread conversation id is not a canonical ULID",
        ));
    }
    if !valid_client_nonce(&proposal.client_nonce) {
        return Err(query(
            "agent thread client nonce is not URL-safe bounded text",
        ));
    }
    if let Some(git) = &proposal.git {
        if git.repository.is_empty()
            || git.repository.len() > model::MAX_AGENT_THREAD_REPOSITORY_BYTES
            || git.repository.chars().any(char::is_control)
            || !git.base_ref.starts_with("refs/heads/")
            || git.base_ref.len() > model::MAX_AGENT_THREAD_GIT_REF_BYTES
            || git.base_ref.chars().any(char::is_control)
            || git.base_oid.len() != 40
            || git
                .base_oid
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
            || git.branch.is_empty()
            || git.branch.len() > model::MAX_AGENT_THREAD_GIT_REF_BYTES
            || git.branch.chars().any(char::is_control)
        {
            return Err(query(
                "agent thread Git binding is not a canonical bounded receipt",
            ));
        }
    }
    let expected_expiry = proposal
        .created_at
        .checked_add_signed(Duration::days(i64::from(proposal.retention_days)))
        .ok_or_else(|| query("agent thread retention overflowed"))?;
    if proposal.expires_at != expected_expiry {
        return Err(query(
            "agent thread expiry does not match its declared retention",
        ));
    }
    Ok(())
}

fn valid_client_nonce(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn validate_storage_locator(locator: &str) -> Result<(), ProviderError> {
    if locator.is_empty() || locator.len() > 1024 || locator.chars().any(char::is_control) {
        return Err(query(
            "agent thread storage locator is not clean bounded text",
        ));
    }
    Ok(())
}

fn canonical_ulid(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
}

fn same_intent(thread: &DurableAgentThread, proposal: &NewAgentThread) -> bool {
    thread.owner_principal_id == proposal.owner_principal_id
        && thread.agent_id == proposal.agent_id.to_string()
        && thread.name == proposal.name
        && thread.project_id == proposal.project_id.map(|id| id.to_string())
        && same_git_intent(thread.git.as_ref(), proposal.git.as_ref())
        && thread.retention_days == proposal.retention_days
}

fn same_git_intent(
    stored: Option<&AgentThreadGitBinding>,
    proposed: Option<&AgentThreadGitBinding>,
) -> bool {
    match (stored, proposed) {
        (None, None) => true,
        (Some(stored), Some(proposed)) => {
            stored.repository == proposed.repository
                && stored.base_ref == proposed.base_ref
                && stored.branch == proposed.branch
        }
        _ => false,
    }
}

async fn live_name_exists(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    owner: &str,
    name: &str,
) -> Result<bool, PgError> {
    sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM agent_thread
            WHERE tenant_id = $1 AND region = $2 AND owner_principal_id = $3
              AND lower(name) = lower($4)
              AND state IN ('provisioning', 'ready', 'expiring', 'failed'))",
    )
    .bind(tenant)
    .bind(region)
    .bind(owner)
    .bind(name)
    .fetch_one(&mut *conn)
    .await
    .map_err(query_error("check live agent thread name"))
}

async fn by_nonce(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    owner: &str,
    client_nonce: &str,
) -> Result<Option<DurableAgentThread>, PgError> {
    let row = sqlx::query(
        "SELECT tenant_id, region, thread_id, owner_principal_id, agent_id,
           conversation_id, workspace_id, workspace_generation, name, project_id,
           repository_slug, base_ref, base_oid, workspace_branch,
           retention_days, state, storage_locator, publication_completed_at, failure_reason, created_at,
           expires_at, updated_at
         FROM agent_thread
        WHERE tenant_id = $1 AND region = $2 AND owner_principal_id = $3 AND client_nonce = $4
        FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(owner)
    .bind(client_nonce)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("read agent thread by retry identity"))?;
    row.as_ref().map(row_to_thread).transpose()
}

async fn by_id(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    thread_id: Uuid,
) -> Result<Option<DurableAgentThread>, PgError> {
    let row = sqlx::query(
        "SELECT tenant_id, region, thread_id, owner_principal_id, agent_id,
           conversation_id, workspace_id, workspace_generation, name, project_id,
           repository_slug, base_ref, base_oid, workspace_branch,
           retention_days, state, storage_locator, publication_completed_at, failure_reason, created_at,
           expires_at, updated_at
         FROM agent_thread
        WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
        FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(thread_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("read agent thread"))?;
    row.as_ref().map(row_to_thread).transpose()
}

fn row_to_thread(row: &sqlx::postgres::PgRow) -> Result<DurableAgentThread, PgError> {
    let state =
        AgentThreadState::parse(row.get::<String, _>("state").as_str()).map_err(PgError::Query)?;
    let generation = row.get::<i32, _>("workspace_generation");
    let workspace_generation = u32::try_from(generation)
        .map_err(|_| PgError::Query("agent thread workspace generation is invalid".into()))?;
    let git = match (
        row.get::<Option<String>, _>("repository_slug"),
        row.get::<Option<String>, _>("base_ref"),
        row.get::<Option<String>, _>("base_oid"),
        row.get::<Option<String>, _>("workspace_branch"),
    ) {
        (None, None, None, None) => None,
        (Some(repository), Some(base_ref), Some(base_oid), Some(branch)) => {
            Some(AgentThreadGitBinding {
                repository,
                base_ref,
                base_oid,
                branch,
            })
        }
        _ => {
            return Err(PgError::Query(
                "agent thread has an incomplete durable Git binding".into(),
            ))
        }
    };
    Ok(DurableAgentThread {
        thread_id: row.get::<Uuid, _>("thread_id").to_string(),
        owner_principal_id: row.get("owner_principal_id"),
        agent_id: row.get::<Uuid, _>("agent_id").to_string(),
        conversation_id: row.get("conversation_id"),
        workspace_id: row.get::<Uuid, _>("workspace_id").to_string(),
        workspace_generation,
        name: row.get("name"),
        project_id: row
            .get::<Option<Uuid>, _>("project_id")
            .map(|id| id.to_string()),
        git,
        retention_days: row.get("retention_days"),
        state,
        storage_locator: row.get("storage_locator"),
        publication_completed_at: row
            .get::<Option<DateTime<Utc>>, _>("publication_completed_at")
            .map(DurableAgentThread::timestamp),
        failure_reason: row.get("failure_reason"),
        created_at: DurableAgentThread::timestamp(row.get("created_at")),
        expires_at: DurableAgentThread::timestamp(row.get("expires_at")),
        updated_at: DurableAgentThread::timestamp(row.get("updated_at")),
    })
}

fn query(message: &str) -> ProviderError {
    PgError::Query(message.into()).into()
}

fn query_error(context: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::types::chrono::TimeZone;

    fn proposal() -> NewAgentThread {
        let created_at = Utc.timestamp_opt(1_775_865_600, 0).single().unwrap();
        NewAgentThread {
            thread_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            owner_principal_id: "p:alice".into(),
            agent_id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
            conversation_id: "01J00000000000000000000000".into(),
            workspace_id: Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
            name: "Investigate checkout race".into(),
            project_id: None,
            git: None,
            retention_days: 3,
            client_nonce: "thread-retry".into(),
            created_at,
            expires_at: created_at + Duration::days(3),
        }
    }

    #[test]
    fn a_thread_expiry_is_exactly_its_declared_retention() {
        assert!(validate_proposal(&proposal()).is_ok());
        let invalid = NewAgentThread {
            expires_at: proposal().expires_at + Duration::seconds(1),
            ..proposal()
        };
        assert!(validate_proposal(&invalid).is_err());
    }

    #[test]
    fn names_nonces_and_conversation_ids_are_bounded_at_the_store_boundary() {
        for invalid in [
            NewAgentThread {
                name: " surrounding ".into(),
                ..proposal()
            },
            NewAgentThread {
                client_nonce: "spaces are not a retry identity".into(),
                ..proposal()
            },
            NewAgentThread {
                conversation_id: "not-an-ulid".into(),
                ..proposal()
            },
        ] {
            assert!(validate_proposal(&invalid).is_err());
        }
    }

    #[test]
    fn exact_thread_batches_are_bounded_and_deduplicated() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        assert!(bounded_exact_thread_batch(&[]).unwrap().is_empty());
        assert_eq!(
            bounded_exact_thread_batch(&[second, first, second]).unwrap(),
            vec![first, second]
        );
        assert!(
            bounded_exact_thread_batch(&vec![first; EXACT_AGENT_THREAD_BATCH_MAX + 1]).is_err()
        );
    }
}
