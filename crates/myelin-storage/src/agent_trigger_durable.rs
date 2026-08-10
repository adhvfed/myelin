mod model;
mod schema;

pub use model::{
    AgentTriggerFiringState, CreateAgentTriggerBindingOutcome, DurableAgentTriggerBinding,
    NewAgentTriggerBinding, ReserveAgentTriggerFiringOutcome, ReservedAgentTriggerFiring,
};
pub use schema::{
    agent_trigger_durable_migrations, AGENT_TRIGGER_MIGRATION, AGENT_TRIGGER_RLS_POLICY,
};

use sqlx::types::chrono::{DateTime, Utc};
use sqlx::types::Uuid;
use sqlx::Row;

use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};

#[derive(Clone)]
pub struct DurableAgentTriggerBacking {
    provider: SubstrateProvider,
}

impl DurableAgentTriggerBacking {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self { provider }
    }

    pub async fn create(
        &self,
        tenant: &str,
        proposal: NewAgentTriggerBinding,
    ) -> Result<CreateAgentTriggerBindingOutcome, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let human = serde_json::to_string(&myelin_identity::PrincipalKind::Human)
                        .expect("principal kind serializes");
                    let active = serde_json::to_string(&myelin_identity::PrincipalStatus::Active)
                        .expect("principal status serializes");
                    let owner = sqlx::query_as::<_, (String, String)>(
                        "SELECT kind, status FROM principal \
                          WHERE tenant_id = $1 AND region = $2 AND principal_id = $3 \
                          FOR UPDATE",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&proposal.owner_principal_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("verify trigger owner"))?;
                    if !owner.is_some_and(|(kind, status)| kind == human && status == active) {
                        return Ok(CreateAgentTriggerBindingOutcome::OwnerUnavailable);
                    }

                    let agent_status = sqlx::query_scalar::<_, String>(
                        "SELECT p.status FROM identity_agent a \
                           JOIN principal p ON p.tenant_id = a.tenant_id \
                            AND p.region = a.region \
                            AND p.principal_id = 'agent:' || a.agent_id::text \
                          WHERE a.tenant_id = $1 AND a.region = $2 AND a.agent_id = $3 \
                          FOR UPDATE OF p",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(proposal.run_as_agent_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("verify trigger run-as agent"))?;
                    if agent_status.as_deref() != Some(active.as_str()) {
                        return Ok(CreateAgentTriggerBindingOutcome::AgentUnavailable);
                    }

                    let max_firings = i64::try_from(proposal.max_firings)
                        .map_err(|_| PgError::Query("trigger max_firings exceeds i64".into()))?;
                    let max_causal_depth = i32::try_from(proposal.max_causal_depth)
                        .map_err(|_| PgError::Query("trigger max_causal_depth exceeds i32".into()))?;
                    let created = sqlx::query(
                        "INSERT INTO agent_trigger_binding (\
                           tenant_id, region, binding_id, owner_principal_id, run_as_agent_id, \
                           client_nonce, event_type, matcher, task, delegation_caveats, max_firings, \
                           max_causal_depth, require_no_personal_data, require_human_approval, created_at\
                         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) \
                         ON CONFLICT (tenant_id, region, owner_principal_id, client_nonce) DO NOTHING",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(proposal.binding_id)
                    .bind(&proposal.owner_principal_id)
                    .bind(proposal.run_as_agent_id)
                    .bind(&proposal.client_nonce)
                    .bind(&proposal.event_type)
                    .bind(&proposal.matcher)
                    .bind(&proposal.task)
                    .bind(&proposal.delegation_caveats)
                    .bind(max_firings)
                    .bind(max_causal_depth)
                    .bind(proposal.require_no_personal_data)
                    .bind(proposal.require_human_approval)
                    .bind(proposal.created_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("create agent trigger binding"))?
                    .rows_affected()
                        == 1;
                    let row = sqlx::query(
                        "SELECT binding_id, owner_principal_id, run_as_agent_id, client_nonce, \
                                event_type, matcher, task, delegation_caveats, max_firings, \
                                firings_used, max_causal_depth, require_no_personal_data, \
                                require_human_approval, state, created_at \
                           FROM agent_trigger_binding \
                          WHERE tenant_id = $1 AND region = $2 \
                            AND owner_principal_id = $3 AND client_nonce = $4",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&proposal.owner_principal_id)
                    .bind(&proposal.client_nonce)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(query_error("load agent trigger binding"))?;
                    let binding = binding_from_row(&row)?;
                    if !binding_matches(&binding, &proposal) {
                        return Ok(CreateAgentTriggerBindingOutcome::Conflict);
                    }
                    Ok(if created {
                        CreateAgentTriggerBindingOutcome::Created(binding)
                    } else {
                        CreateAgentTriggerBindingOutcome::Replayed(binding)
                    })
                })
            })
            .await
    }

    pub async fn active_for_event(
        &self,
        tenant: &str,
        event_type: &str,
        limit: u32,
    ) -> Result<Vec<DurableAgentTriggerBinding>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let event_type = event_type.to_string();
        let limit = i64::from(limit.clamp(1, 1_000));
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let human = serde_json::to_string(&myelin_identity::PrincipalKind::Human)
                        .expect("principal kind serializes");
                    let active = serde_json::to_string(&myelin_identity::PrincipalStatus::Active)
                        .expect("principal status serializes");
                    let rows = sqlx::query(
                        "SELECT b.binding_id, b.owner_principal_id, b.run_as_agent_id, \
                                b.client_nonce, b.event_type, b.matcher, b.task, \
                                b.delegation_caveats, b.max_firings, b.firings_used, \
                                b.max_causal_depth, b.require_no_personal_data, \
                                b.require_human_approval, b.state, b.created_at \
                           FROM agent_trigger_binding b \
                           JOIN principal owner ON owner.tenant_id = b.tenant_id \
                            AND owner.region = b.region \
                            AND owner.principal_id = b.owner_principal_id \
                           JOIN principal agent ON agent.tenant_id = b.tenant_id \
                            AND agent.region = b.region \
                            AND agent.principal_id = 'agent:' || b.run_as_agent_id::text \
                          WHERE b.tenant_id = $1 AND b.region = $2 AND b.event_type = $3 \
                            AND b.state = 'active' AND b.firings_used < b.max_firings \
                            AND owner.kind = $4 AND owner.status = $5 AND agent.status = $5 \
                          ORDER BY b.binding_id LIMIT $6",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&event_type)
                    .bind(&human)
                    .bind(&active)
                    .bind(limit)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(query_error("list active agent triggers for event"))?;
                    rows.iter().map(binding_from_row).collect()
                })
            })
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn reserve_firing(
        &self,
        tenant: &str,
        binding_id: Uuid,
        event_id: &str,
        event_type: &str,
        event_envelope: serde_json::Value,
        causal_depth: u32,
        contains_personal_data: bool,
        created_at: DateTime<Utc>,
    ) -> Result<ReserveAgentTriggerFiringOutcome, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let event_id = event_id.to_string();
        let event_type = event_type.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT b.event_type, b.max_firings, b.firings_used, b.max_causal_depth, \
                                b.require_no_personal_data, b.require_human_approval, b.state, \
                                owner.kind AS owner_kind, owner.status AS owner_status, \
                                agent.status AS agent_status \
                           FROM agent_trigger_binding b \
                           JOIN principal owner ON owner.tenant_id = b.tenant_id \
                            AND owner.region = b.region \
                            AND owner.principal_id = b.owner_principal_id \
                           JOIN principal agent ON agent.tenant_id = b.tenant_id \
                            AND agent.region = b.region \
                            AND agent.principal_id = 'agent:' || b.run_as_agent_id::text \
                          WHERE b.tenant_id = $1 AND b.region = $2 AND b.binding_id = $3 \
                          FOR UPDATE OF b, owner, agent",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(binding_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("lock agent trigger binding"))?;
                    let Some(row) = row else {
                        return Ok(ReserveAgentTriggerFiringOutcome::BindingUnavailable);
                    };

                    if let Some(existing) = load_firing(conn, &tenant, &region, binding_id, &event_id).await? {
                        return Ok(ReserveAgentTriggerFiringOutcome::AlreadyReserved(existing));
                    }

                    let active = serde_json::to_string(&myelin_identity::PrincipalStatus::Active)
                        .expect("principal status serializes");
                    let human = serde_json::to_string(&myelin_identity::PrincipalKind::Human)
                        .expect("principal kind serializes");
                    if row.try_get::<String, _>("state").map_err(row_error("state"))? != "active"
                        || row
                            .try_get::<String, _>("owner_kind")
                            .map_err(row_error("owner_kind"))?
                            != human
                        || row
                            .try_get::<String, _>("owner_status")
                            .map_err(row_error("owner_status"))?
                            != active
                        || row
                            .try_get::<String, _>("agent_status")
                            .map_err(row_error("agent_status"))?
                            != active
                    {
                        return Ok(ReserveAgentTriggerFiringOutcome::BindingUnavailable);
                    }
                    if row
                        .try_get::<String, _>("event_type")
                        .map_err(row_error("event_type"))?
                        != event_type
                    {
                        return Ok(ReserveAgentTriggerFiringOutcome::EventTypeMismatch);
                    }
                    let max_depth = row
                        .try_get::<i32, _>("max_causal_depth")
                        .map_err(row_error("max_causal_depth"))?;
                    let max_depth = u32::try_from(max_depth).map_err(|_| {
                        PgError::Query("negative durable trigger max_causal_depth".into())
                    })?;
                    let no_pii = row
                        .try_get::<bool, _>("require_no_personal_data")
                        .map_err(row_error("require_no_personal_data"))?;
                    if causal_depth > max_depth
                        || (no_pii && contains_personal_data)
                    {
                        return Ok(ReserveAgentTriggerFiringOutcome::GateRefused);
                    }
                    let max_firings = row
                        .try_get::<i64, _>("max_firings")
                        .map_err(row_error("max_firings"))?;
                    let used = row
                        .try_get::<i64, _>("firings_used")
                        .map_err(row_error("firings_used"))?;
                    if used >= max_firings {
                        return Ok(ReserveAgentTriggerFiringOutcome::BudgetExhausted);
                    }
                    let firing_state = if row
                        .try_get::<bool, _>("require_human_approval")
                        .map_err(row_error("require_human_approval"))?
                    {
                        AgentTriggerFiringState::AwaitingApproval
                    } else {
                        AgentTriggerFiringState::Queued
                    };
                    sqlx::query(
                        "INSERT INTO agent_trigger_firing (\
                           tenant_id, region, binding_id, event_id, event_type, event_envelope, state, created_at\
                         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(binding_id)
                    .bind(&event_id)
                    .bind(&event_type)
                    .bind(&event_envelope)
                    .bind(firing_state.token())
                    .bind(created_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("reserve agent trigger firing"))?;
                    sqlx::query(
                        "UPDATE agent_trigger_binding SET firings_used = firings_used + 1 \
                          WHERE tenant_id = $1 AND region = $2 AND binding_id = $3",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(binding_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("charge agent trigger firing budget"))?;
                    Ok(ReserveAgentTriggerFiringOutcome::Reserved(
                        ReservedAgentTriggerFiring {
                            binding_id: binding_id.to_string(),
                            event_id,
                            event_type,
                            state: firing_state,
                        },
                    ))
                })
            })
            .await
    }
}

fn binding_matches(
    binding: &DurableAgentTriggerBinding,
    proposal: &NewAgentTriggerBinding,
) -> bool {
    binding.run_as_agent_id == proposal.run_as_agent_id.to_string()
        && binding.event_type == proposal.event_type
        && binding.matcher == proposal.matcher
        && binding.task == proposal.task
        && binding.delegation_caveats == proposal.delegation_caveats
        && binding.max_firings == proposal.max_firings
        && binding.max_causal_depth == proposal.max_causal_depth
        && binding.require_no_personal_data == proposal.require_no_personal_data
        && binding.require_human_approval == proposal.require_human_approval
}

fn binding_from_row(row: &sqlx::postgres::PgRow) -> Result<DurableAgentTriggerBinding, PgError> {
    let max_firings = row
        .try_get::<i64, _>("max_firings")
        .map_err(row_error("max_firings"))?;
    let firings_used = row
        .try_get::<i64, _>("firings_used")
        .map_err(row_error("firings_used"))?;
    let max_causal_depth = row
        .try_get::<i32, _>("max_causal_depth")
        .map_err(row_error("max_causal_depth"))?;
    Ok(DurableAgentTriggerBinding {
        binding_id: row
            .try_get::<Uuid, _>("binding_id")
            .map_err(row_error("binding_id"))?
            .to_string(),
        owner_principal_id: row
            .try_get("owner_principal_id")
            .map_err(row_error("owner_principal_id"))?,
        run_as_agent_id: row
            .try_get::<Uuid, _>("run_as_agent_id")
            .map_err(row_error("run_as_agent_id"))?
            .to_string(),
        client_nonce: row
            .try_get("client_nonce")
            .map_err(row_error("client_nonce"))?,
        event_type: row.try_get("event_type").map_err(row_error("event_type"))?,
        matcher: row.try_get("matcher").map_err(row_error("matcher"))?,
        task: row.try_get("task").map_err(row_error("task"))?,
        delegation_caveats: row
            .try_get("delegation_caveats")
            .map_err(row_error("delegation_caveats"))?,
        max_firings: u64::try_from(max_firings)
            .map_err(|_| PgError::Query("negative durable trigger max_firings".into()))?,
        firings_used: u64::try_from(firings_used)
            .map_err(|_| PgError::Query("negative durable trigger firings_used".into()))?,
        max_causal_depth: u32::try_from(max_causal_depth)
            .map_err(|_| PgError::Query("negative durable trigger max_causal_depth".into()))?,
        require_no_personal_data: row
            .try_get("require_no_personal_data")
            .map_err(row_error("require_no_personal_data"))?,
        require_human_approval: row
            .try_get("require_human_approval")
            .map_err(row_error("require_human_approval"))?,
        state: row.try_get("state").map_err(row_error("state"))?,
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .map_err(row_error("created_at"))?
            .to_rfc3339(),
    })
}

async fn load_firing(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    binding_id: Uuid,
    event_id: &str,
) -> Result<Option<ReservedAgentTriggerFiring>, PgError> {
    let row = sqlx::query(
        "SELECT binding_id, event_id, event_type, state FROM agent_trigger_firing \
          WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 AND event_id = $4",
    )
    .bind(tenant)
    .bind(region)
    .bind(binding_id)
    .bind(event_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("load agent trigger firing"))?;
    row.map(|row| {
        let state = AgentTriggerFiringState::parse(
            &row.try_get::<String, _>("state")
                .map_err(row_error("state"))?,
        )?;
        Ok(ReservedAgentTriggerFiring {
            binding_id: row
                .try_get::<Uuid, _>("binding_id")
                .map_err(row_error("binding_id"))?
                .to_string(),
            event_id: row.try_get("event_id").map_err(row_error("event_id"))?,
            event_type: row.try_get("event_type").map_err(row_error("event_type"))?,
            state,
        })
    })
    .transpose()
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{operation}: {error}"))
}

fn row_error(column: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("decode agent trigger `{column}`: {error}"))
}
