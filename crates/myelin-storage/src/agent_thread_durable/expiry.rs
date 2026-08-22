use chrono::{DateTime, Duration, Utc};
use sqlx::types::Uuid;
use sqlx::Row;

use super::DurableAgentThreadBacking;
use crate::{PgError, ProviderError};

pub const AGENT_THREAD_EXPIRY_GRACE_SECONDS: i64 = 30;
const MAX_EXPIRY_BATCH: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThreadExpiry {
    pub thread_id: Uuid,
    pub conversation_id: String,
    pub workspace_id: Uuid,
    pub workspace_generation: u32,
    pub storage_locator: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentThreadExpiryCompletion {
    Deleted,
    AlreadyDeleted,
    Changed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentThreadExpiryFailure {
    WorkspaceCleanupFailed,
}

impl AgentThreadExpiryFailure {
    fn token(self) -> &'static str {
        match self {
            Self::WorkspaceCleanupFailed => "workspace_cleanup_failed",
        }
    }
}

impl DurableAgentThreadBacking {
    /// Makes every due surface inaccessible in one transaction and returns the
    /// newly claimed cleanup work. Files remain intact until a later pass has
    /// allowed live SSH gateways to observe the state transition.
    pub async fn start_due_expirations(
        &self,
        tenant: &str,
        observed_at: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<AgentThreadExpiry>, ProviderError> {
        validate_limit(limit)?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "WITH due AS (
                           SELECT thread_id
                             FROM agent_thread
                            WHERE tenant_id = $1 AND region = $2
                              AND state IN ('provisioning', 'ready', 'failed')
                              AND expires_at <= $3
                            ORDER BY expires_at, thread_id
                            LIMIT $4
                            FOR UPDATE SKIP LOCKED
                         )
                         UPDATE agent_thread thread
                            SET state = 'expiring', failure_reason = NULL, updated_at = $3
                           FROM due
                          WHERE thread.tenant_id = $1 AND thread.region = $2
                            AND thread.thread_id = due.thread_id
                        RETURNING thread.thread_id, thread.conversation_id, thread.workspace_id,
                                  thread.workspace_generation, thread.storage_locator",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(observed_at)
                    .bind(i64::from(limit))
                    .fetch_all(&mut *connection)
                    .await
                    .map_err(query_error("claim due private thread expirations"))?;
                    let work = rows
                        .iter()
                        .map(expiry_from_row)
                        .collect::<Result<Vec<_>, _>>()?;
                    if work.is_empty() {
                        return Ok(work);
                    }
                    let thread_ids = work.iter().map(|item| item.thread_id).collect::<Vec<_>>();
                    let conversation_ids = work
                        .iter()
                        .map(|item| item.conversation_id.clone())
                        .collect::<Vec<_>>();

                    sqlx::query(
                        "UPDATE agent_thread_ssh_grant
                            SET revoked_at = COALESCE(revoked_at, $4)
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = ANY($3)",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&thread_ids)
                    .bind(observed_at)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("revoke expiring private workspace SSH grants"))?;

                    sqlx::query(
                        "INSERT INTO run_token_teardown (tenant_id, region, jti)
                         SELECT run.tenant_id, run.region, run.token_jti
                           FROM external_agent_run run
                           JOIN agent_thread_run binding
                             ON binding.tenant_id = run.tenant_id
                            AND binding.region = run.region
                            AND binding.run_id = run.run_id
                          WHERE binding.tenant_id = $1 AND binding.region = $2
                            AND binding.thread_id = ANY($3)
                            AND run.state IN ('provisioning', 'ready')
                         ON CONFLICT DO NOTHING",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&thread_ids)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("revoke expiring private thread run identities"))?;

                    sqlx::query(
                        "UPDATE external_agent_run run
                            SET state = 'terminal'
                           FROM agent_thread_run binding
                          WHERE binding.tenant_id = $1 AND binding.region = $2
                            AND binding.thread_id = ANY($3)
                            AND binding.tenant_id = run.tenant_id
                            AND binding.region = run.region
                            AND binding.run_id = run.run_id
                            AND run.state IN ('provisioning', 'ready')",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&thread_ids)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("terminate expiring private thread runs"))?;

                    sqlx::query(
                        "UPDATE chat_conversation
                            SET archived = true
                          WHERE tenant_id = $1 AND region = $2
                            AND conversation_id = ANY($3)",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&conversation_ids)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("archive expiring private conversations"))?;

                    sqlx::query(
                        "DELETE FROM rebac_tuple
                          WHERE tenant_id = $1 AND region = $2
                            AND object_id = ANY(
                              SELECT 'channel:' || conversation_id
                                FROM unnest($3::text[]) AS conversation_id
                            )
                            AND relation IN ('member', 'watcher')",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&conversation_ids)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error(
                        "remove expiring private conversation authority",
                    ))?;

                    Ok(work)
                })
            })
            .await
    }

    pub async fn expirations_ready_for_cleanup(
        &self,
        tenant: &str,
        observed_at: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<AgentThreadExpiry>, ProviderError> {
        validate_limit(limit)?;
        let ready_before = observed_at
            .checked_sub_signed(Duration::seconds(AGENT_THREAD_EXPIRY_GRACE_SECONDS))
            .ok_or_else(|| query("private thread expiry grace underflowed"))?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT thread_id, conversation_id, workspace_id,
                                workspace_generation, storage_locator
                           FROM agent_thread
                          WHERE tenant_id = $1 AND region = $2 AND state = 'expiring'
                            AND updated_at <= $3
                          ORDER BY updated_at, thread_id
                          LIMIT $4",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(ready_before)
                    .bind(i64::from(limit))
                    .fetch_all(&mut *connection)
                    .await
                    .map_err(query_error(
                        "list private thread expirations ready for cleanup",
                    ))?;
                    rows.iter().map(expiry_from_row).collect()
                })
            })
            .await
    }

    pub async fn complete_expiration(
        &self,
        tenant: &str,
        work: &AgentThreadExpiry,
        completed_at: DateTime<Utc>,
    ) -> Result<AgentThreadExpiryCompletion, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let work = work.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let changed = sqlx::query(
                        "UPDATE agent_thread
                            SET state = 'deleted', storage_locator = NULL, failure_reason = NULL,
                                updated_at = $6
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                            AND workspace_id = $4 AND workspace_generation = $5
                            AND state = 'expiring'",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(work.thread_id)
                    .bind(work.workspace_id)
                    .bind(i32::try_from(work.workspace_generation).map_err(|_| {
                        PgError::Query("private thread expiry generation overflowed".into())
                    })?)
                    .bind(completed_at)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("complete private thread expiration"))?
                    .rows_affected();
                    if changed == 1 {
                        return Ok(AgentThreadExpiryCompletion::Deleted);
                    }
                    let state = sqlx::query_scalar::<_, String>(
                        "SELECT state FROM agent_thread
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                            AND workspace_id = $4 AND workspace_generation = $5",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(work.thread_id)
                    .bind(work.workspace_id)
                    .bind(i32::try_from(work.workspace_generation).map_err(|_| {
                        PgError::Query("private thread expiry generation overflowed".into())
                    })?)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error("check completed private thread expiration"))?;
                    Ok(if state.as_deref() == Some("deleted") {
                        AgentThreadExpiryCompletion::AlreadyDeleted
                    } else {
                        AgentThreadExpiryCompletion::Changed
                    })
                })
            })
            .await
    }

    pub async fn record_expiration_failure(
        &self,
        tenant: &str,
        work: &AgentThreadExpiry,
        failure: AgentThreadExpiryFailure,
        observed_at: DateTime<Utc>,
    ) -> Result<bool, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let work = work.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let changed = sqlx::query(
                        "UPDATE agent_thread
                            SET failure_reason = $6, updated_at = $7
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                            AND workspace_id = $4 AND workspace_generation = $5
                            AND state = 'expiring'",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(work.thread_id)
                    .bind(work.workspace_id)
                    .bind(i32::try_from(work.workspace_generation).map_err(|_| {
                        PgError::Query("private thread expiry generation overflowed".into())
                    })?)
                    .bind(failure.token())
                    .bind(observed_at)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("record private thread expiration failure"))?
                    .rows_affected();
                    Ok(changed == 1)
                })
            })
            .await
    }
}

fn validate_limit(limit: u32) -> Result<(), ProviderError> {
    if (1..=MAX_EXPIRY_BATCH).contains(&limit) {
        Ok(())
    } else {
        Err(query(
            "private thread expiry batch must be between 1 and 100",
        ))
    }
}

fn expiry_from_row(row: &sqlx::postgres::PgRow) -> Result<AgentThreadExpiry, PgError> {
    let generation = row
        .try_get::<i32, _>("workspace_generation")
        .map_err(row_error("workspace_generation"))?;
    Ok(AgentThreadExpiry {
        thread_id: row.try_get("thread_id").map_err(row_error("thread_id"))?,
        conversation_id: row
            .try_get("conversation_id")
            .map_err(row_error("conversation_id"))?,
        workspace_id: row
            .try_get("workspace_id")
            .map_err(row_error("workspace_id"))?,
        workspace_generation: u32::try_from(generation).map_err(|_| {
            PgError::Query("private thread expiry has an invalid generation".into())
        })?,
        storage_locator: row
            .try_get("storage_locator")
            .map_err(row_error("storage_locator"))?,
    })
}

fn query(message: &str) -> ProviderError {
    PgError::Query(message.into()).into()
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{operation}: {error}"))
}

fn row_error(column: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("decode private thread expiry `{column}`: {error}"))
}
