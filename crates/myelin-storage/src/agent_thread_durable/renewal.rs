use chrono::{DateTime, Duration, SecondsFormat, Timelike, Utc};
use sqlx::types::Uuid;
use sqlx::Row;

use super::{by_id, AgentThreadState, DurableAgentThread, DurableAgentThreadBacking};
use crate::{PgError, ProviderError};

const MAX_RENEWAL_BATCH: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThreadRenewal {
    pub renewal_id: Uuid,
    pub thread_id: Uuid,
    pub owner_principal_id: String,
    pub agent_id: Uuid,
    pub conversation_id: String,
    pub project_id: Option<Uuid>,
    pub review_ready: bool,
    pub retention_days: i16,
    pub previous_expires_at: String,
    pub expires_at: String,
    pub completed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BeginAgentThreadRenewalOutcome {
    Begun {
        renewal: AgentThreadRenewal,
        thread: DurableAgentThread,
    },
    Replayed {
        renewal: AgentThreadRenewal,
        thread: DurableAgentThread,
    },
    AlreadyRetained(DurableAgentThread),
    Conflict,
    NotFound,
    Unavailable,
}

impl DurableAgentThreadBacking {
    #[allow(clippy::too_many_arguments)]
    pub async fn begin_renewal(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
        renewal_id: Uuid,
        retention_days: i16,
        client_nonce: &str,
        observed_at: DateTime<Utc>,
    ) -> Result<BeginAgentThreadRenewalOutcome, ProviderError> {
        if !(super::MIN_AGENT_THREAD_RETENTION_DAYS..=super::MAX_AGENT_THREAD_RETENTION_DAYS)
            .contains(&retention_days)
        {
            return Err(query("agent thread renewal retention is outside policy"));
        }
        if client_nonce.is_empty() || client_nonce.len() > 128 {
            return Err(query("agent thread renewal nonce is invalid"));
        }
        let observed_at = observed_at
            .with_nanosecond(0)
            .ok_or_else(|| query("agent thread renewal observation is invalid"))?;
        let requested_expiry = observed_at
            .checked_add_signed(Duration::days(i64::from(retention_days)))
            .ok_or_else(|| query("agent thread renewal expiry overflowed"))?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        let nonce = client_nonce.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let existing = sqlx::query(
                        "SELECT renewal_id, thread_id, owner_principal_id, agent_id,
                                conversation_id, project_id, review_ready, retention_days,
                                previous_expires_at, expires_at, completed_at
                           FROM agent_thread_renewal
                          WHERE tenant_id = $1 AND region = $2
                            AND owner_principal_id = $3 AND client_nonce = $4",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&owner)
                    .bind(&nonce)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error("read agent thread renewal retry"))?;
                    if let Some(row) = existing.as_ref() {
                        let renewal = renewal_from_row(row)?;
                        if renewal.thread_id != thread_id
                            || renewal.retention_days != retention_days
                        {
                            return Ok(BeginAgentThreadRenewalOutcome::Conflict);
                        }
                        let thread = by_id(connection, &tenant, &region, thread_id)
                            .await?
                            .ok_or_else(|| {
                                PgError::Query("renewed agent thread disappeared".into())
                            })?;
                        return Ok(BeginAgentThreadRenewalOutcome::Replayed { renewal, thread });
                    }

                    let thread = by_id(connection, &tenant, &region, thread_id).await?;
                    let Some(thread) = thread else {
                        return Ok(BeginAgentThreadRenewalOutcome::NotFound);
                    };
                    if thread.owner_principal_id != owner {
                        return Ok(BeginAgentThreadRenewalOutcome::NotFound);
                    }
                    let current_expiry = DateTime::parse_from_rfc3339(&thread.expires_at)
                        .map_err(|_| {
                            PgError::Query("stored agent thread expiry is invalid".into())
                        })?
                        .with_timezone(&Utc);
                    if thread.state != AgentThreadState::Ready
                        || !thread.is_published()
                        || current_expiry <= observed_at
                    {
                        return Ok(BeginAgentThreadRenewalOutcome::Unavailable);
                    }
                    if requested_expiry <= current_expiry {
                        return Ok(BeginAgentThreadRenewalOutcome::AlreadyRetained(thread));
                    }

                    let agent_id = Uuid::parse_str(&thread.agent_id)
                        .map_err(|_| PgError::Query("stored agent id is invalid".into()))?;
                    let project_id = thread
                        .project_id
                        .as_deref()
                        .map(Uuid::parse_str)
                        .transpose()
                        .map_err(|_| PgError::Query("stored project id is invalid".into()))?;
                    sqlx::query(
                        "INSERT INTO agent_thread_renewal (
                           tenant_id, region, renewal_id, thread_id, owner_principal_id,
                           agent_id, conversation_id, project_id, review_ready, retention_days,
                           client_nonce, previous_expires_at, expires_at, created_at, updated_at
                         ) VALUES (
                           $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14
                         )",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(renewal_id)
                    .bind(thread_id)
                    .bind(&owner)
                    .bind(agent_id)
                    .bind(&thread.conversation_id)
                    .bind(project_id)
                    .bind(thread.review_ready_at.is_some())
                    .bind(retention_days)
                    .bind(&nonce)
                    .bind(current_expiry)
                    .bind(requested_expiry)
                    .bind(observed_at)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("begin agent thread renewal"))?;

                    let row = sqlx::query(
                        "UPDATE agent_thread
                            SET expires_at = $5, retention_days = $7,
                                updated_at = GREATEST(updated_at, $6)
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                            AND owner_principal_id = $4 AND state = 'ready'
                            AND expires_at > $6 AND expires_at < $5
                        RETURNING tenant_id, region, thread_id, owner_principal_id, agent_id,
                          conversation_id, workspace_id, workspace_generation, name, project_id,
                          repository_slug, base_ref, base_oid, workspace_branch,
                          workspace_guidance_digest, workspace_guidance_request_digest,
                          workspace_guidance_key_ref, workspace_guidance_nonce,
                          workspace_guidance_ciphertext, environment_provider,
                          environment_config_path, environment_config_digest, retention_days,
                          state, storage_locator, publication_completed_at, review_ready_at,
                          failure_reason, created_at, expires_at, updated_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(thread_id)
                    .bind(&owner)
                    .bind(requested_expiry)
                    .bind(observed_at)
                    .bind(retention_days)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error("extend agent thread expiry"))?;
                    let thread = row
                        .as_ref()
                        .map(super::row_to_thread)
                        .transpose()?
                        .ok_or_else(|| {
                            PgError::Query("agent thread renewal lost its row".into())
                        })?;
                    Ok(BeginAgentThreadRenewalOutcome::Begun {
                        renewal: AgentThreadRenewal {
                            renewal_id,
                            thread_id,
                            owner_principal_id: owner,
                            agent_id,
                            conversation_id: thread.conversation_id.clone(),
                            project_id,
                            review_ready: thread.review_ready_at.is_some(),
                            retention_days,
                            previous_expires_at: timestamp(current_expiry),
                            expires_at: timestamp(requested_expiry),
                            completed_at: None,
                        },
                        thread,
                    })
                })
            })
            .await
    }

    pub async fn claim_pending_renewals(
        &self,
        tenant: &str,
        stale_before: DateTime<Utc>,
        claimed_at: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<AgentThreadRenewal>, ProviderError> {
        if !(1..=MAX_RENEWAL_BATCH).contains(&limit) || stale_before > claimed_at {
            return Err(query("agent thread renewal recovery boundary is invalid"));
        }
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "WITH pending AS (
                           SELECT renewal_id
                             FROM agent_thread_renewal
                            WHERE tenant_id = $1 AND region = $2
                              AND completed_at IS NULL AND updated_at <= $3
                            ORDER BY updated_at, renewal_id
                            LIMIT $5
                            FOR UPDATE SKIP LOCKED
                         )
                         UPDATE agent_thread_renewal renewal
                            SET updated_at = $4
                           FROM pending
                          WHERE renewal.tenant_id = $1 AND renewal.region = $2
                            AND renewal.renewal_id = pending.renewal_id
                        RETURNING renewal.renewal_id, renewal.thread_id,
                          renewal.owner_principal_id, renewal.agent_id,
                          renewal.conversation_id, renewal.project_id, renewal.review_ready,
                          renewal.retention_days, renewal.previous_expires_at,
                          renewal.expires_at, renewal.completed_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(stale_before)
                    .bind(claimed_at)
                    .bind(i64::from(limit))
                    .fetch_all(&mut *connection)
                    .await
                    .map_err(query_error("claim pending agent thread renewals"))?;
                    rows.iter().map(renewal_from_row).collect()
                })
            })
            .await
    }

    pub async fn complete_renewal(
        &self,
        tenant: &str,
        renewal_id: Uuid,
        completed_at: DateTime<Utc>,
    ) -> Result<bool, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let changed = sqlx::query(
                        "UPDATE agent_thread_renewal
                            SET completed_at = $4, updated_at = $4
                          WHERE tenant_id = $1 AND region = $2 AND renewal_id = $3
                            AND completed_at IS NULL",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(renewal_id)
                    .bind(completed_at)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("complete agent thread renewal"))?
                    .rows_affected();
                    Ok(changed == 1)
                })
            })
            .await
    }
}

fn renewal_from_row(row: &sqlx::postgres::PgRow) -> Result<AgentThreadRenewal, PgError> {
    Ok(AgentThreadRenewal {
        renewal_id: row.try_get("renewal_id").map_err(row_error("renewal_id"))?,
        thread_id: row.try_get("thread_id").map_err(row_error("thread_id"))?,
        owner_principal_id: row
            .try_get("owner_principal_id")
            .map_err(row_error("owner_principal_id"))?,
        agent_id: row.try_get("agent_id").map_err(row_error("agent_id"))?,
        conversation_id: row
            .try_get("conversation_id")
            .map_err(row_error("conversation_id"))?,
        project_id: row.try_get("project_id").map_err(row_error("project_id"))?,
        review_ready: row
            .try_get("review_ready")
            .map_err(row_error("review_ready"))?,
        retention_days: row
            .try_get("retention_days")
            .map_err(row_error("retention_days"))?,
        previous_expires_at: timestamp(
            row.try_get("previous_expires_at")
                .map_err(row_error("previous_expires_at"))?,
        ),
        expires_at: timestamp(row.try_get("expires_at").map_err(row_error("expires_at"))?),
        completed_at: row
            .try_get::<Option<DateTime<Utc>>, _>("completed_at")
            .map_err(row_error("completed_at"))?
            .map(timestamp),
    })
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn query(message: &str) -> ProviderError {
    PgError::Query(message.into()).into()
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{operation}: {error}"))
}

fn row_error(column: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("decode agent thread renewal `{column}`: {error}"))
}
