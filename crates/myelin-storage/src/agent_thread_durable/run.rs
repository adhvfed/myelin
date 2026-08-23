use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::types::Uuid;
use sqlx::Row;

use super::DurableAgentThreadBacking;
use crate::{PgError, ProviderError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThreadRunBinding {
    pub run_id: String,
    pub thread_id: String,
    pub conversation_id: String,
    pub workspace_id: String,
    pub workspace_generation: u32,
    pub workspace_expires_at: String,
    pub workspace_storage_locator: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindAgentThreadRunOutcome {
    Bound(AgentThreadRunBinding),
    Replayed(AgentThreadRunBinding),
    NotFound,
    Conflict,
}

impl DurableAgentThreadBacking {
    pub async fn bind_run(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
        run_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<BindAgentThreadRunOutcome, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let inserted = sqlx::query(
                        "INSERT INTO agent_thread_run (
                           tenant_id, region, run_id, thread_id, conversation_id,
                           workspace_id, workspace_generation, bound_at
                         )
                         SELECT thread.tenant_id, thread.region, run.run_id, thread.thread_id,
                                thread.conversation_id, thread.workspace_id,
                                thread.workspace_generation, $6
                           FROM agent_thread thread
                           JOIN external_agent_run run
                             ON run.tenant_id = thread.tenant_id
                            AND run.region = thread.region
                            AND run.agent_id = thread.agent_id
                          WHERE thread.tenant_id = $1 AND thread.region = $2
                            AND thread.thread_id = $3 AND run.run_id = $4
                            AND thread.owner_principal_id = $5
                            AND run.trigger_actor_id = $5
                            AND thread.state = 'ready' AND thread.expires_at > $6
                            AND run.state = 'ready' AND run.expires_at > $6
                            AND run.expires_at <= thread.expires_at
                         ON CONFLICT (tenant_id, region, run_id) DO NOTHING
                         RETURNING run_id, thread_id, conversation_id, workspace_id,
                                   workspace_generation",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(thread_id)
                    .bind(run_id)
                    .bind(&owner)
                    .bind(now)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error("bind external agent run to private thread"))?;
                    if let Some(row) = inserted {
                        let (expires_at, storage_locator) =
                            thread_workspace_details(&mut *connection, &tenant, &region, thread_id)
                                .await?;
                        return binding_from_row(&row, expires_at, storage_locator)
                            .map(BindAgentThreadRunOutcome::Bound);
                    }

                    if let Some(binding) = binding_for_intent(
                        &mut *connection,
                        &tenant,
                        &region,
                        &owner,
                        thread_id,
                        run_id,
                        now,
                    )
                    .await?
                    {
                        return Ok(BindAgentThreadRunOutcome::Replayed(binding));
                    }
                    Ok(
                        if run_binding_belongs_to_owner(
                            &mut *connection,
                            &tenant,
                            &region,
                            &owner,
                            run_id,
                        )
                        .await?
                        {
                            BindAgentThreadRunOutcome::Conflict
                        } else {
                            BindAgentThreadRunOutcome::NotFound
                        },
                    )
                })
            })
            .await
    }

    pub async fn live_binding_for_run(
        &self,
        tenant: &str,
        run_id: Uuid,
        agent_id: Uuid,
        token_jti: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<AgentThreadRunBinding>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let token_jti = token_jti.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT binding.run_id, binding.thread_id, binding.conversation_id,
                                binding.workspace_id, binding.workspace_generation,
                                thread.expires_at AS workspace_expires_at,
                                thread.storage_locator AS workspace_storage_locator
                           FROM agent_thread_run binding
                           JOIN agent_thread thread
                             ON thread.tenant_id = binding.tenant_id
                            AND thread.region = binding.region
                            AND thread.thread_id = binding.thread_id
                           JOIN external_agent_run run
                             ON run.tenant_id = binding.tenant_id
                            AND run.region = binding.region
                            AND run.run_id = binding.run_id
                          WHERE binding.tenant_id = $1 AND binding.region = $2
                            AND binding.run_id = $3 AND run.agent_id = $4
                            AND run.token_jti = $5
                            AND run.state = 'ready' AND run.expires_at > $6
                            AND thread.state = 'ready' AND thread.expires_at > $6
                            AND binding.conversation_id = thread.conversation_id
                            AND binding.workspace_id = thread.workspace_id
                            AND binding.workspace_generation = thread.workspace_generation",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(run_id)
                    .bind(agent_id)
                    .bind(&token_jti)
                    .bind(now)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error("resolve live private thread run binding"))?;
                    row.as_ref().map(binding_from_joined_row).transpose()
                })
            })
            .await
    }
}

async fn binding_for_intent(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    owner: &str,
    thread_id: Uuid,
    run_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<AgentThreadRunBinding>, PgError> {
    let row = sqlx::query(
        "SELECT binding.run_id, binding.thread_id, binding.conversation_id,
                binding.workspace_id, binding.workspace_generation,
                thread.expires_at AS workspace_expires_at,
                thread.storage_locator AS workspace_storage_locator
           FROM agent_thread_run binding
           JOIN agent_thread thread
             ON thread.tenant_id = binding.tenant_id
            AND thread.region = binding.region
            AND thread.thread_id = binding.thread_id
           JOIN external_agent_run run
             ON run.tenant_id = binding.tenant_id
            AND run.region = binding.region
            AND run.run_id = binding.run_id
          WHERE binding.tenant_id = $1 AND binding.region = $2
            AND binding.thread_id = $3 AND binding.run_id = $4
            AND thread.owner_principal_id = $5 AND run.trigger_actor_id = $5
            AND run.agent_id = thread.agent_id
            AND thread.state = 'ready' AND thread.expires_at > $6
            AND run.state = 'ready' AND run.expires_at > $6
            AND run.expires_at <= thread.expires_at
            AND binding.conversation_id = thread.conversation_id
            AND binding.workspace_id = thread.workspace_id
            AND binding.workspace_generation = thread.workspace_generation",
    )
    .bind(tenant)
    .bind(region)
    .bind(thread_id)
    .bind(run_id)
    .bind(owner)
    .bind(now)
    .fetch_optional(connection)
    .await
    .map_err(query_error("replay private thread run binding"))?;
    row.as_ref().map(binding_from_joined_row).transpose()
}

async fn run_binding_belongs_to_owner(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    owner: &str,
    run_id: Uuid,
) -> Result<bool, PgError> {
    sqlx::query_scalar(
        "SELECT EXISTS (
           SELECT 1 FROM agent_thread_run binding
           JOIN external_agent_run run
             ON run.tenant_id = binding.tenant_id
            AND run.region = binding.region
            AND run.run_id = binding.run_id
          WHERE binding.tenant_id = $1 AND binding.region = $2
            AND binding.run_id = $3 AND run.trigger_actor_id = $4)",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(owner)
    .fetch_one(connection)
    .await
    .map_err(query_error("check private thread run retry ownership"))
}

async fn thread_workspace_details(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    thread_id: Uuid,
) -> Result<(DateTime<Utc>, String), PgError> {
    let row = sqlx::query(
        "SELECT expires_at, storage_locator FROM agent_thread
          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(thread_id)
    .fetch_one(connection)
    .await
    .map_err(query_error("read private thread workspace details"))?;
    Ok((
        row.try_get("expires_at")
            .map_err(row_error("workspace_expires_at"))?,
        row.try_get("storage_locator")
            .map_err(row_error("workspace_storage_locator"))?,
    ))
}

fn binding_from_joined_row(row: &sqlx::postgres::PgRow) -> Result<AgentThreadRunBinding, PgError> {
    binding_from_row(
        row,
        row.try_get("workspace_expires_at")
            .map_err(row_error("workspace_expires_at"))?,
        row.try_get("workspace_storage_locator")
            .map_err(row_error("workspace_storage_locator"))?,
    )
}

fn binding_from_row(
    row: &sqlx::postgres::PgRow,
    workspace_expires_at: DateTime<Utc>,
    workspace_storage_locator: String,
) -> Result<AgentThreadRunBinding, PgError> {
    let generation = row
        .try_get::<i32, _>("workspace_generation")
        .map_err(row_error("workspace_generation"))?;
    Ok(AgentThreadRunBinding {
        run_id: row
            .try_get::<Uuid, _>("run_id")
            .map_err(row_error("run_id"))?
            .to_string(),
        thread_id: row
            .try_get::<Uuid, _>("thread_id")
            .map_err(row_error("thread_id"))?
            .to_string(),
        conversation_id: row
            .try_get("conversation_id")
            .map_err(row_error("conversation_id"))?,
        workspace_id: row
            .try_get::<Uuid, _>("workspace_id")
            .map_err(row_error("workspace_id"))?
            .to_string(),
        workspace_generation: u32::try_from(generation)
            .map_err(|_| PgError::Query("private thread run has an invalid generation".into()))?,
        workspace_expires_at: workspace_expires_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        workspace_storage_locator,
    })
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{operation}: {error}"))
}

fn row_error(column: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("decode private thread run `{column}`: {error}"))
}
