mod model;
mod schema;

pub use model::{
    AgentTriggerApprovalDecision, AgentTriggerApprovalOutcome, AgentTriggerCapacityScope,
    AgentTriggerClaimRequest, AgentTriggerEvaluationDiagnostic, AgentTriggerEvaluationErrorCode,
    AgentTriggerFiringState, AgentTriggerLifecycleAction, AgentTriggerLifecycleOutcome,
    AgentTriggerRunOutcome, AgentTriggerStartRequest, ChangeAgentTriggerApprovalOutcome,
    ChangeAgentTriggerLifecycleOutcome, ClaimedAgentTriggerFiring,
    CreateAgentTriggerBindingOutcome, DurableAgentTriggerBinding, DurableAgentTriggerFiring,
    NewAgentTriggerBinding, ReserveAgentTriggerFiringOutcome, ReservedAgentTriggerFiring,
    StartAgentTriggerFiringOutcome, StartedAgentTriggerRun, TerminalizeAgentTriggerClaimOutcome,
    MAX_ACTIVE_AGENT_TRIGGERS_PER_EVENT, MAX_ACTIVE_AGENT_TRIGGERS_PER_OWNER_EVENT,
    MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS, MIN_AGENT_TRIGGER_BUDGET_MINOR_UNITS,
};
pub use schema::{
    agent_trigger_durable_migrations, agent_trigger_evaluation_diagnostic_migrations,
    agent_trigger_terminal_reason_migrations, AGENT_TRIGGER_APPROVAL_MIGRATION,
    AGENT_TRIGGER_BUDGET_MIGRATION, AGENT_TRIGGER_CLAIM_MIGRATION,
    AGENT_TRIGGER_EVALUATION_DIAGNOSTIC_MIGRATION, AGENT_TRIGGER_MIGRATION,
    AGENT_TRIGGER_RLS_POLICY, AGENT_TRIGGER_RUN_MIGRATION, AGENT_TRIGGER_TERMINAL_REASON_MIGRATION,
};

use sqlx::types::chrono::{DateTime, Utc};
use sqlx::types::Uuid;
use sqlx::Row;

use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};
use crate::reserve_settle::RunId as CostRunId;
use crate::reserve_settle_durable::DurableCostLedger;

const FAILED_RUN_TERMINAL_REASON: &str =
    "agent run failed; retry it or inspect the hosted-agent service diagnostics";
const TERMINATED_RUN_TERMINAL_REASON: &str = "agent run was terminated";
const NONDETERMINISTIC_RUN_TERMINAL_REASON: &str =
    "agent run stopped because durable replay diverged";

#[derive(Clone)]
pub struct DurableAgentTriggerBacking {
    provider: SubstrateProvider,
    capacity: AgentTriggerCapacity,
}

#[derive(Clone, Copy)]
struct AgentTriggerCapacity {
    per_owner_event: u32,
    per_event: u32,
}

impl DurableAgentTriggerBacking {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self {
            provider,
            capacity: AgentTriggerCapacity {
                per_owner_event: MAX_ACTIVE_AGENT_TRIGGERS_PER_OWNER_EVENT,
                per_event: MAX_ACTIVE_AGENT_TRIGGERS_PER_EVENT,
            },
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_capacity_for_test(
        provider: SubstrateProvider,
        per_owner_event: u32,
        per_event: u32,
    ) -> Self {
        assert!(per_owner_event > 0);
        assert!(per_owner_event <= per_event);
        Self {
            provider,
            capacity: AgentTriggerCapacity {
                per_owner_event,
                per_event,
            },
        }
    }

    pub async fn create(
        &self,
        tenant: &str,
        proposal: NewAgentTriggerBinding,
    ) -> Result<CreateAgentTriggerBindingOutcome, ProviderError> {
        if !(MIN_AGENT_TRIGGER_BUDGET_MINOR_UNITS..=MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS)
            .contains(&proposal.budget_minor_units)
        {
            return Err(
                PgError::Query("trigger budget is outside its durable bound".into()).into(),
            );
        }
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let capacity = self.capacity;
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
                            AND a.created_by = $4 \
                          FOR UPDATE OF p",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(proposal.run_as_agent_id)
                    .bind(&proposal.owner_principal_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("verify trigger run-as agent"))?;
                    if agent_status.as_deref() != Some(active.as_str()) {
                        return Ok(CreateAgentTriggerBindingOutcome::AgentUnavailable);
                    }

                    lock_agent_trigger_capacity(conn, &tenant, &region, &proposal.event_type)
                        .await?;
                    if let Some(binding) = binding_for_nonce_on_connection(
                        conn,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.client_nonce,
                    )
                    .await?
                    {
                        return Ok(if binding_matches(&binding, &proposal) {
                            CreateAgentTriggerBindingOutcome::Replayed(binding)
                        } else {
                            CreateAgentTriggerBindingOutcome::Conflict
                        });
                    }
                    if let Some(scope) = active_trigger_capacity_scope_on_connection(
                        conn,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.event_type,
                        capacity,
                    )
                    .await?
                    {
                        return Ok(CreateAgentTriggerBindingOutcome::CapacityReached(scope));
                    }

                    let max_firings = i64::try_from(proposal.max_firings)
                        .map_err(|_| PgError::Query("trigger max_firings exceeds i64".into()))?;
                    let budget_minor_units = i64::try_from(proposal.budget_minor_units)
                        .map_err(|_| PgError::Query("trigger budget exceeds i64".into()))?;
                    let max_causal_depth = i32::try_from(proposal.max_causal_depth)
                        .map_err(|_| PgError::Query("trigger max_causal_depth exceeds i32".into()))?;
                    let created = sqlx::query(
                        "INSERT INTO agent_trigger_binding (\
                           tenant_id, region, binding_id, owner_principal_id, run_as_agent_id, \
                           client_nonce, event_type, matcher, task, delegation_caveats, \
                           budget_minor_units, max_firings, max_causal_depth, \
                           require_no_personal_data, require_human_approval, created_at\
                         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16) \
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
                    .bind(budget_minor_units)
                    .bind(max_firings)
                    .bind(max_causal_depth)
                    .bind(proposal.require_no_personal_data)
                    .bind(proposal.require_human_approval)
                    .bind(proposal.created_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("create agent trigger binding"))?
                    .rows_affected();
                    let binding = binding_for_nonce_on_connection(
                        conn,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.client_nonce,
                    )
                    .await?
                    .ok_or_else(|| {
                        PgError::Query("created agent trigger binding is unavailable".into())
                    })?;
                    if !binding_matches(&binding, &proposal) {
                        return Ok(CreateAgentTriggerBindingOutcome::Conflict);
                    }
                    Ok(if created == 1 {
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
        // Consumers may ask for one row beyond their own fanout bound so they can
        // distinguish an exact-cap batch from an overflow without unbounded reads.
        let limit =
            i64::from(limit.clamp(1, MAX_ACTIVE_AGENT_TRIGGERS_PER_EVENT.saturating_add(1)));
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
                                b.delegation_caveats, b.budget_minor_units, \
                                b.max_firings, b.firings_used, \
                                b.max_causal_depth, b.require_no_personal_data, \
                                b.require_human_approval, b.state, b.created_at, \
                                b.last_evaluation_error_code, \
                                b.last_evaluation_error_detail, \
                                b.last_evaluation_error_event_id, b.last_evaluation_error_at \
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

    pub async fn record_evaluation_error(
        &self,
        tenant: &str,
        binding_id: Uuid,
        event_id: &str,
        code: AgentTriggerEvaluationErrorCode,
        detail: &str,
        event_recorded_at: DateTime<Utc>,
    ) -> Result<bool, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let event_id = event_id.to_string();
        let code = code.token().to_string();
        let detail = detail.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let updated = sqlx::query(
                        "UPDATE agent_trigger_binding \
                            SET last_evaluation_error_code = $5, \
                                last_evaluation_error_detail = $6, \
                                last_evaluation_error_event_id = $4, \
                                last_evaluation_error_at = $7 \
                          WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 \
                            AND (last_evaluation_error_at IS NULL \
                              OR (last_evaluation_error_at, last_evaluation_error_event_id) \
                                 <= ($7, $4))",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(binding_id)
                    .bind(&event_id)
                    .bind(&code)
                    .bind(&detail)
                    .bind(event_recorded_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("record agent trigger evaluation error"))?;
                    Ok(updated.rows_affected() == 1)
                })
            })
            .await
    }

    pub async fn list_for_owner(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<DurableAgentTriggerBinding>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner_principal_id = owner_principal_id.to_string();
        let limit = i64::from(limit.clamp(1, 1_000));
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT binding_id, owner_principal_id, run_as_agent_id, client_nonce, \
                                event_type, matcher, task, delegation_caveats, \
                                budget_minor_units, max_firings, \
                                firings_used, max_causal_depth, require_no_personal_data, \
                                require_human_approval, state, created_at, \
                                last_evaluation_error_code, last_evaluation_error_detail, \
                                last_evaluation_error_event_id, last_evaluation_error_at \
                           FROM agent_trigger_binding \
                          WHERE tenant_id = $1 AND region = $2 AND owner_principal_id = $3 \
                            AND ($4::uuid IS NULL OR binding_id > $4::uuid) \
                          ORDER BY binding_id LIMIT $5",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&owner_principal_id)
                    .bind(after)
                    .bind(limit)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(query_error("list agent trigger bindings for owner"))?;
                    rows.iter().map(binding_from_row).collect()
                })
            })
            .await
    }

    pub async fn get_for_owner(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        binding_id: Uuid,
    ) -> Result<Option<DurableAgentTriggerBinding>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner_principal_id = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT binding_id, owner_principal_id, run_as_agent_id, client_nonce, \
                                event_type, matcher, task, delegation_caveats, \
                                budget_minor_units, max_firings, firings_used, max_causal_depth, \
                                require_no_personal_data, require_human_approval, state, created_at, \
                                last_evaluation_error_code, last_evaluation_error_detail, \
                                last_evaluation_error_event_id, last_evaluation_error_at \
                           FROM agent_trigger_binding \
                          WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 \
                            AND owner_principal_id = $4",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(binding_id)
                    .bind(&owner_principal_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("get agent trigger binding for owner"))?;
                    row.as_ref().map(binding_from_row).transpose()
                })
            })
            .await
    }

    pub async fn change_lifecycle(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        binding_id: Uuid,
        action: AgentTriggerLifecycleAction,
    ) -> Result<ChangeAgentTriggerLifecycleOutcome, ProviderError> {
        let tenant = tenant.to_string();
        let owner_principal_id = owner_principal_id.to_string();
        let backing = self.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let outcome = backing
                        .change_lifecycle_on_conn(
                            conn,
                            &tenant,
                            &owner_principal_id,
                            binding_id,
                            action,
                        )
                        .await?;
                    if matches!(
                        outcome,
                        ChangeAgentTriggerLifecycleOutcome::Complete(ref lifecycle)
                            if !lifecycle.canceled_run_ids.is_empty()
                    ) {
                        return Err(PgError::Query(
                            "disabling an automation with started runs requires coordinated lifecycle cleanup"
                                .into(),
                        ));
                    }
                    Ok(outcome)
                })
            })
            .await
    }

    /// Changes one owned binding and fences all of its unfinished firings on the caller's
    /// transaction. Run IDs are returned so the control plane can terminate the corresponding
    /// workflows, gates, reservations, and approval projections before that transaction commits.
    pub async fn change_lifecycle_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        owner_principal_id: &str,
        binding_id: Uuid,
        action: AgentTriggerLifecycleAction,
    ) -> Result<ChangeAgentTriggerLifecycleOutcome, PgError> {
        let region = self.provider.config().region.as_str();
        let row = sqlx::query(
            "SELECT binding_id, owner_principal_id, run_as_agent_id, client_nonce, \
                    event_type, matcher, task, delegation_caveats, \
                    budget_minor_units, max_firings, firings_used, max_causal_depth, \
                    require_no_personal_data, require_human_approval, state, created_at, \
                    last_evaluation_error_code, last_evaluation_error_detail, \
                    last_evaluation_error_event_id, last_evaluation_error_at \
               FROM agent_trigger_binding \
              WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 \
                AND owner_principal_id = $4 FOR UPDATE",
        )
        .bind(tenant)
        .bind(region)
        .bind(binding_id)
        .bind(owner_principal_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(query_error("lock agent trigger lifecycle"))?;
        let Some(row) = row else {
            return Ok(ChangeAgentTriggerLifecycleOutcome::NotFound);
        };
        let mut binding = binding_from_row(&row)?;
        let Some(changed) = lifecycle_transition(&binding.state, action) else {
            return Ok(ChangeAgentTriggerLifecycleOutcome::InvalidTransition);
        };
        let target = action.target();
        if changed {
            if action == AgentTriggerLifecycleAction::Resume
                && binding.firings_used < binding.max_firings
            {
                lock_agent_trigger_capacity(conn, tenant, region, &binding.event_type).await?;
                if let Some(scope) = active_trigger_capacity_scope_on_connection(
                    conn,
                    tenant,
                    region,
                    &binding.owner_principal_id,
                    &binding.event_type,
                    self.capacity,
                )
                .await?
                {
                    return Ok(ChangeAgentTriggerLifecycleOutcome::CapacityReached(scope));
                }
            }
            sqlx::query(
                "UPDATE agent_trigger_binding SET state = $5 \
                  WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 \
                    AND owner_principal_id = $4",
            )
            .bind(tenant)
            .bind(region)
            .bind(binding_id)
            .bind(owner_principal_id)
            .bind(target)
            .execute(&mut *conn)
            .await
            .map_err(query_error("change agent trigger lifecycle"))?;
            binding.state = target.to_string();
        }
        let canceled = if action == AgentTriggerLifecycleAction::Disable {
            sqlx::query_scalar::<_, String>(
                "SELECT run.run_id FROM agent_trigger_firing firing \
                   JOIN workflow_run run ON run.tenant_id = firing.tenant_id \
                    AND run.region = firing.region AND run.run_id = firing.run_id::text \
                  WHERE firing.tenant_id = $1 AND firing.region = $2 \
                    AND firing.binding_id = $3 AND firing.state = 'started' \
                  ORDER BY firing.event_id FOR UPDATE OF firing, run",
            )
            .bind(tenant)
            .bind(region)
            .bind(binding_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(query_error("lock started automation workflows"))?;
            sqlx::query(
                "UPDATE agent_trigger_firing firing SET state = 'terminal' \
                  WHERE firing.tenant_id = $1 AND firing.region = $2 \
                    AND firing.binding_id = $3 AND firing.state = 'started' \
                    AND EXISTS (SELECT 1 FROM workflow_run run \
                         WHERE run.tenant_id = firing.tenant_id \
                           AND run.region = firing.region \
                           AND run.run_id = firing.run_id::text \
                           AND run.state IN ('completed','failed','terminated','nondeterministic'))",
            )
            .bind(tenant)
            .bind(region)
            .bind(binding_id)
            .execute(&mut *conn)
            .await
            .map_err(query_error(
                "project completed workflows before disabling automation",
            ))?;
            sqlx::query_scalar::<_, Option<Uuid>>(
                "UPDATE agent_trigger_firing \
                    SET state = 'terminal', terminal_reason = \
                          COALESCE(terminal_reason, 'automation disabled by owner'), \
                        claim_owner = NULL, claim_until = NULL \
                  WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 \
                    AND state IN ('queued','awaiting_approval','claimed','started') \
              RETURNING run_id",
            )
            .bind(tenant)
            .bind(region)
            .bind(binding_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(query_error("cancel disabled agent trigger firings"))?
        } else {
            Vec::new()
        };
        let canceled_firings = canceled.len() as u64;
        let canceled_run_ids = canceled
            .into_iter()
            .flatten()
            .map(|run_id| run_id.to_string())
            .collect();
        Ok(ChangeAgentTriggerLifecycleOutcome::Complete(Box::new(
            AgentTriggerLifecycleOutcome {
                binding,
                changed,
                canceled_firings,
                canceled_run_ids,
            },
        )))
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

    pub async fn list_firings_for_owner(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        binding_id: Uuid,
        before_event_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<DurableAgentTriggerFiring>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner_principal_id = owner_principal_id.to_string();
        let before_event_id = before_event_id.map(str::to_string);
        let limit = i64::from(limit.clamp(1, 1_001));
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT f.binding_id, f.event_id, f.event_type, f.state, f.run_id, \
                                f.terminal_reason, \
                                f.approval_decision, f.approval_decided_by, \
                                f.approval_decided_at, f.created_at, \
                                run.state AS workflow_state \
                           FROM agent_trigger_firing f \
                           JOIN agent_trigger_binding b ON b.tenant_id = f.tenant_id \
                            AND b.region = f.region AND b.binding_id = f.binding_id \
                           LEFT JOIN workflow_run run ON run.tenant_id = f.tenant_id \
                            AND run.region = f.region AND run.run_id = f.run_id::text \
                          WHERE f.tenant_id = $1 AND f.region = $2 \
                            AND f.binding_id = $3 AND b.owner_principal_id = $4 \
                            AND ($5::text IS NULL OR (f.created_at, f.event_id) < (\
                                SELECT cursor.created_at, cursor.event_id \
                                  FROM agent_trigger_firing cursor \
                                 WHERE cursor.tenant_id = $1 AND cursor.region = $2 \
                                   AND cursor.binding_id = $3 AND cursor.event_id = $5)) \
                          ORDER BY f.created_at DESC, f.event_id DESC LIMIT $6",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(binding_id)
                    .bind(&owner_principal_id)
                    .bind(&before_event_id)
                    .bind(limit)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(query_error("list agent trigger firings for owner"))?;
                    rows.iter().map(firing_from_row).collect()
                })
            })
            .await
    }

    pub async fn change_firing_approval(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        binding_id: Uuid,
        event_id: &str,
        decision: AgentTriggerApprovalDecision,
    ) -> Result<ChangeAgentTriggerApprovalOutcome, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner_principal_id = owner_principal_id.to_string();
        let event_id = event_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT f.binding_id, f.event_id, f.event_type, f.state, f.run_id, \
                                f.terminal_reason, \
                                f.approval_decision, f.approval_decided_by, \
                                f.approval_decided_at, f.created_at, \
                                NULL::text AS workflow_state \
                           FROM agent_trigger_firing f \
                           JOIN agent_trigger_binding b ON b.tenant_id = f.tenant_id \
                            AND b.region = f.region AND b.binding_id = f.binding_id \
                          WHERE f.tenant_id = $1 AND f.region = $2 AND f.binding_id = $3 \
                            AND f.event_id = $4 AND b.owner_principal_id = $5 \
                          FOR UPDATE OF f, b",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(binding_id)
                    .bind(&event_id)
                    .bind(&owner_principal_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("lock governed firing approval"))?;
                    let Some(row) = row else {
                        return Ok(ChangeAgentTriggerApprovalOutcome::NotFound);
                    };
                    let mut firing = firing_from_row(&row)?;
                    if firing.approval_decision == Some(decision) {
                        return Ok(ChangeAgentTriggerApprovalOutcome::Complete(Box::new(
                            AgentTriggerApprovalOutcome {
                                firing,
                                changed: false,
                            },
                        )));
                    }
                    if firing.state != AgentTriggerFiringState::AwaitingApproval
                        || firing.approval_decision.is_some()
                    {
                        return Ok(ChangeAgentTriggerApprovalOutcome::InvalidTransition);
                    }

                    let target_state = match decision {
                        AgentTriggerApprovalDecision::Approve => AgentTriggerFiringState::Queued,
                        AgentTriggerApprovalDecision::Reject => AgentTriggerFiringState::Terminal,
                    };
                    let decided_at = sqlx::query_scalar::<_, DateTime<Utc>>(
                        "UPDATE agent_trigger_firing \
                            SET state = $6, approval_decision = $7, \
                                approval_decided_by = $5, approval_decided_at = clock_timestamp() \
                          WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 \
                            AND event_id = $4 \
                      RETURNING approval_decided_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(binding_id)
                    .bind(&event_id)
                    .bind(&owner_principal_id)
                    .bind(target_state.token())
                    .bind(decision.token())
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(query_error("decide governed firing approval"))?;
                    firing.state = target_state;
                    firing.approval_decision = Some(decision);
                    firing.approval_decided_by = Some(owner_principal_id);
                    firing.approval_decided_at = Some(decided_at.to_rfc3339());
                    Ok(ChangeAgentTriggerApprovalOutcome::Complete(Box::new(
                        AgentTriggerApprovalOutcome {
                            firing,
                            changed: true,
                        },
                    )))
                })
            })
            .await
    }

    /// Projects terminal workflow state back onto its governed firing.
    ///
    /// This is intentionally replayable. The workflow commit remains the source of truth; if the
    /// worker dies between that commit and this projection, the next reconciliation pass closes
    /// the same firing without re-running agent work.
    pub async fn reconcile_terminal_firings(
        &self,
        tenant: &str,
        limit: u32,
    ) -> Result<u64, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let limit = i64::from(limit.clamp(1, 1_001));
        let ledger = DurableCostLedger::new(self.provider.clone());
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let terminal_run_ids = sqlx::query_scalar::<_, String>(
                        "WITH terminal AS (\
                            SELECT firing.binding_id, firing.event_id, firing.run_id, \
                                   run.state AS workflow_state \
                              FROM agent_trigger_firing firing \
                              JOIN workflow_run run ON run.tenant_id = firing.tenant_id \
                               AND run.region = firing.region \
                               AND run.run_id = firing.run_id::text \
                             WHERE firing.tenant_id = $1 AND firing.region = $2 \
                               AND firing.state = 'started' \
                               AND run.state IN ('completed','failed','terminated','nondeterministic') \
                             ORDER BY firing.created_at, firing.binding_id, firing.event_id \
                             FOR UPDATE OF firing SKIP LOCKED LIMIT $3\
                         ) \
                         UPDATE agent_trigger_firing firing SET state = 'terminal', \
                           terminal_reason = CASE terminal.workflow_state \
                               WHEN 'completed' THEN NULL \
                               WHEN 'failed' THEN $4 \
                               WHEN 'terminated' THEN $5 \
                               WHEN 'nondeterministic' THEN $6 \
                           END \
                           FROM terminal \
                          WHERE firing.tenant_id = $1 AND firing.region = $2 \
                            AND firing.binding_id = terminal.binding_id \
                            AND firing.event_id = terminal.event_id \
                            AND firing.state = 'started' \
                      RETURNING firing.run_id::text",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(limit)
                    .bind(FAILED_RUN_TERMINAL_REASON)
                    .bind(TERMINATED_RUN_TERMINAL_REASON)
                    .bind(NONDETERMINISTIC_RUN_TERMINAL_REASON)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(query_error("reconcile terminal governed agent firings"))?;
                    for run_id in &terminal_run_ids {
                        ledger
                            .stop_if_present_in_tx(
                                conn,
                                &myelin_tenancy::TenantId(tenant.clone()),
                                &CostRunId::new(run_id),
                            )
                            .await
                            .map_err(|error| {
                                PgError::Query(format!(
                                    "settle terminal governed agent firing: {error}"
                                ))
                            })?;
                    }
                    Ok(terminal_run_ids.len() as u64)
                })
            })
            .await
    }

    pub async fn claim_next_firing(
        &self,
        tenant: &str,
        request: AgentTriggerClaimRequest,
    ) -> Result<Option<ClaimedAgentTriggerFiring>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let active = serde_json::to_string(&myelin_identity::PrincipalStatus::Active)
                        .expect("principal status serializes");
                    let human = serde_json::to_string(&myelin_identity::PrincipalKind::Human)
                        .expect("principal kind serializes");
                    let row = sqlx::query(
                        "WITH candidate AS (\
                            SELECT f.binding_id, f.event_id, b.owner_principal_id, \
                                   b.run_as_agent_id, b.task, b.delegation_caveats, \
                                   b.budget_minor_units, \
                                   agent_row.runtime_ref \
                              FROM agent_trigger_firing f \
                              JOIN agent_trigger_binding b ON b.tenant_id = f.tenant_id \
                               AND b.region = f.region AND b.binding_id = f.binding_id \
                              JOIN identity_agent agent_row ON agent_row.tenant_id = b.tenant_id \
                               AND agent_row.region = b.region \
                               AND agent_row.agent_id = b.run_as_agent_id \
                              JOIN principal owner ON owner.tenant_id = b.tenant_id \
                               AND owner.region = b.region \
                               AND owner.principal_id = b.owner_principal_id \
                              JOIN principal agent ON agent.tenant_id = b.tenant_id \
                               AND agent.region = b.region \
                               AND agent.principal_id = 'agent:' || b.run_as_agent_id::text \
                             WHERE f.tenant_id = $1 AND f.region = $2 \
                               AND agent_row.runtime_ref = $3 AND b.state = 'active' \
                               AND owner.kind = $4 AND owner.status = $5 AND agent.status = $5 \
                               AND (f.state = 'queued' \
                                 OR (f.state = 'claimed' AND f.claim_until <= clock_timestamp())) \
                             ORDER BY f.created_at, f.binding_id, f.event_id \
                             FOR UPDATE OF f, b SKIP LOCKED LIMIT 1\
                         ) \
                         UPDATE agent_trigger_firing f \
                            SET state = 'claimed', claim_owner = $6, \
                                claim_until = clock_timestamp() + ($7 * INTERVAL '1 second'), \
                                claim_attempts = claim_attempts + 1 \
                           FROM candidate \
                          WHERE f.tenant_id = $1 AND f.region = $2 \
                            AND f.binding_id = candidate.binding_id \
                            AND f.event_id = candidate.event_id \
                      RETURNING f.binding_id, f.event_id, f.event_type, f.event_envelope, \
                                candidate.owner_principal_id, candidate.run_as_agent_id, \
                                candidate.runtime_ref, candidate.task, candidate.delegation_caveats, \
                                candidate.budget_minor_units, \
                                f.claim_owner, f.claim_until, f.claim_attempts",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&request.runtime_ref)
                    .bind(&human)
                    .bind(&active)
                    .bind(&request.worker_id)
                    .bind(i64::from(request.lease_seconds))
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("claim next governed agent firing"))?;
                    row.as_ref().map(claimed_firing_from_row).transpose()
                })
            })
            .await
    }

    /// Permanently isolates one malformed or otherwise non-retryable claimed firing.
    ///
    /// The exact claim owner is the fence: a late worker cannot terminalize a claim that another
    /// process has already reclaimed. Transient failures should leave the lease intact instead so
    /// the ordinary expiry path can retry them.
    pub async fn terminalize_claim(
        &self,
        tenant: &str,
        claim: &ClaimedAgentTriggerFiring,
        reason: &str,
    ) -> Result<TerminalizeAgentTriggerClaimOutcome, ProviderError> {
        let binding_id = Uuid::parse_str(&claim.binding_id)
            .map_err(|_| PgError::Query("claimed trigger binding_id is not a UUID".into()))?;
        let reason = reason.trim();
        if reason.is_empty() || reason.len() > 1024 {
            return Err(PgError::Query(
                "trigger terminal reason must contain 1..=1024 bytes".into(),
            )
            .into());
        }
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let event_id = claim.event_id.clone();
        let claim_owner = claim.claim_owner.clone();
        let reason = reason.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let changed = sqlx::query(
                        "UPDATE agent_trigger_firing \
                            SET state = 'terminal', terminal_reason = $6, \
                                claim_owner = NULL, claim_until = NULL \
                          WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 \
                            AND event_id = $4 AND state = 'claimed' AND claim_owner = $5",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(binding_id)
                    .bind(&event_id)
                    .bind(&claim_owner)
                    .bind(&reason)
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("terminalize invalid governed agent firing"))?
                    .rows_affected()
                        == 1;
                    Ok(if changed {
                        TerminalizeAgentTriggerClaimOutcome::Terminalized
                    } else {
                        TerminalizeAgentTriggerClaimOutcome::ClaimUnavailable
                    })
                })
            })
            .await
    }

    /// Promotes one live firing claim to a run on the caller's transaction.
    ///
    /// The caller must insert the durable workflow on the same transaction. If
    /// either write fails, both are rolled back, so `started` always means that
    /// a run-of-record exists and a reclaimed lease can never create a second
    /// workflow.
    pub async fn start_claimed_firing_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        request: &AgentTriggerStartRequest,
    ) -> Result<StartAgentTriggerFiringOutcome, PgError> {
        let region = self.provider.config().region.as_str();
        let updated = sqlx::query_scalar::<_, Uuid>(
            "UPDATE agent_trigger_firing \
                SET state = 'started', run_id = $6, claim_owner = NULL, claim_until = NULL \
              WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 AND event_id = $4 \
                AND state = 'claimed' AND claim_owner = $5 \
                AND claim_until > clock_timestamp() \
          RETURNING run_id",
        )
        .bind(tenant)
        .bind(region)
        .bind(request.binding_id)
        .bind(&request.event_id)
        .bind(&request.claim_owner)
        .bind(request.run_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(query_error("promote governed agent firing to started"))?;
        if updated.is_some() {
            return Ok(StartAgentTriggerFiringOutcome::Started);
        }

        let existing = sqlx::query_as::<_, (String, Option<Uuid>)>(
            "SELECT state, run_id FROM agent_trigger_firing \
              WHERE tenant_id = $1 AND region = $2 AND binding_id = $3 AND event_id = $4",
        )
        .bind(tenant)
        .bind(region)
        .bind(request.binding_id)
        .bind(&request.event_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(query_error(
            "inspect unavailable governed agent firing claim",
        ))?;
        Ok(match existing {
            Some((state, Some(run_id)))
                if state == AgentTriggerFiringState::Started.token()
                    && run_id == request.run_id =>
            {
                StartAgentTriggerFiringOutcome::AlreadyStarted
            }
            _ => StartAgentTriggerFiringOutcome::ClaimUnavailable,
        })
    }

    pub async fn started_for_run(
        &self,
        tenant: &str,
        run_id: &str,
    ) -> Result<Option<StartedAgentTriggerRun>, ProviderError> {
        let run_id = Uuid::parse_str(run_id)
            .map_err(|_| PgError::Query("governed agent run_id is not a UUID".into()))?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let active = serde_json::to_string(&myelin_identity::PrincipalStatus::Active)
                        .expect("principal status serializes");
                    let human = serde_json::to_string(&myelin_identity::PrincipalKind::Human)
                        .expect("principal kind serializes");
                    let row = sqlx::query(
                        "SELECT f.binding_id, f.event_id, f.event_type, f.event_envelope, f.run_id, \
                                b.owner_principal_id, b.run_as_agent_id, b.task, \
                                b.delegation_caveats, b.budget_minor_units, a.runtime_ref, a.tools \
                           FROM agent_trigger_firing f \
                           JOIN agent_trigger_binding b ON b.tenant_id = f.tenant_id \
                            AND b.region = f.region AND b.binding_id = f.binding_id \
                           JOIN identity_agent a ON a.tenant_id = b.tenant_id \
                            AND a.region = b.region AND a.agent_id = b.run_as_agent_id \
                           JOIN principal owner ON owner.tenant_id = b.tenant_id \
                            AND owner.region = b.region AND owner.principal_id = b.owner_principal_id \
                           JOIN principal agent ON agent.tenant_id = b.tenant_id \
                            AND agent.region = b.region \
                            AND agent.principal_id = 'agent:' || b.run_as_agent_id::text \
                          WHERE f.tenant_id = $1 AND f.region = $2 AND f.run_id = $3 \
                            AND f.state = 'started' AND owner.kind = $4 \
                            AND owner.status = $5 AND agent.status = $5",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(run_id)
                    .bind(&human)
                    .bind(&active)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("load started governed agent run"))?;
                    row.as_ref().map(started_run_from_row).transpose()
                })
            })
            .await
    }
}

async fn lock_agent_trigger_capacity(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    event_type: &str,
) -> Result<(), PgError> {
    let capacity_key =
        format!("agent-trigger-capacity\u{1f}{tenant}\u{1f}{region}\u{1f}{event_type}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(capacity_key)
        .execute(&mut *conn)
        .await
        .map_err(query_error("lock agent trigger capacity"))?;
    Ok(())
}

async fn binding_for_nonce_on_connection(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    owner_principal_id: &str,
    client_nonce: &str,
) -> Result<Option<DurableAgentTriggerBinding>, PgError> {
    let row = sqlx::query(
        "SELECT binding_id, owner_principal_id, run_as_agent_id, client_nonce, \
                event_type, matcher, task, delegation_caveats, \
                budget_minor_units, max_firings, firings_used, max_causal_depth, \
                require_no_personal_data, require_human_approval, state, created_at, \
                last_evaluation_error_code, last_evaluation_error_detail, \
                last_evaluation_error_event_id, last_evaluation_error_at \
           FROM agent_trigger_binding \
          WHERE tenant_id = $1 AND region = $2 \
            AND owner_principal_id = $3 AND client_nonce = $4",
    )
    .bind(tenant)
    .bind(region)
    .bind(owner_principal_id)
    .bind(client_nonce)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("load agent trigger binding by nonce"))?;
    row.as_ref().map(binding_from_row).transpose()
}

async fn active_trigger_capacity_scope_on_connection(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    owner_principal_id: &str,
    event_type: &str,
    capacity: AgentTriggerCapacity,
) -> Result<Option<AgentTriggerCapacityScope>, PgError> {
    let (owner_count, event_count) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(*) FILTER (WHERE owner_principal_id = $4), count(*) \
           FROM agent_trigger_binding \
          WHERE tenant_id = $1 AND region = $2 AND event_type = $3 \
            AND state = 'active' AND firings_used < max_firings",
    )
    .bind(tenant)
    .bind(region)
    .bind(event_type)
    .bind(owner_principal_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(query_error("count active agent trigger capacity"))?;
    let owner_count = u64::try_from(owner_count)
        .map_err(|_| PgError::Query("negative owner trigger capacity count".into()))?;
    let event_count = u64::try_from(event_count)
        .map_err(|_| PgError::Query("negative event trigger capacity count".into()))?;
    Ok(capacity_scope(owner_count, event_count, capacity))
}

fn capacity_scope(
    owner_count: u64,
    event_count: u64,
    capacity: AgentTriggerCapacity,
) -> Option<AgentTriggerCapacityScope> {
    if owner_count >= u64::from(capacity.per_owner_event) {
        Some(AgentTriggerCapacityScope::OwnerEvent)
    } else if event_count >= u64::from(capacity.per_event) {
        Some(AgentTriggerCapacityScope::TenantEvent)
    } else {
        None
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
        && binding.budget_minor_units == proposal.budget_minor_units
        && binding.max_firings == proposal.max_firings
        && binding.max_causal_depth == proposal.max_causal_depth
        && binding.require_no_personal_data == proposal.require_no_personal_data
        && binding.require_human_approval == proposal.require_human_approval
}

fn lifecycle_transition(current: &str, action: AgentTriggerLifecycleAction) -> Option<bool> {
    match (current, action) {
        ("active", AgentTriggerLifecycleAction::Pause)
        | ("paused", AgentTriggerLifecycleAction::Resume)
        | ("active" | "paused", AgentTriggerLifecycleAction::Disable) => Some(true),
        ("paused", AgentTriggerLifecycleAction::Pause)
        | ("active", AgentTriggerLifecycleAction::Resume)
        | ("disabled", AgentTriggerLifecycleAction::Disable) => Some(false),
        _ => None,
    }
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
        budget_minor_units: positive_u64(row, "budget_minor_units")?,
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
        last_evaluation_error: evaluation_diagnostic_from_row(row)?,
    })
}

fn evaluation_diagnostic_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<Option<AgentTriggerEvaluationDiagnostic>, PgError> {
    let code = row
        .try_get::<Option<String>, _>("last_evaluation_error_code")
        .map_err(row_error("last_evaluation_error_code"))?;
    let detail = row
        .try_get::<Option<String>, _>("last_evaluation_error_detail")
        .map_err(row_error("last_evaluation_error_detail"))?;
    let event_id = row
        .try_get::<Option<String>, _>("last_evaluation_error_event_id")
        .map_err(row_error("last_evaluation_error_event_id"))?;
    let event_recorded_at = row
        .try_get::<Option<DateTime<Utc>>, _>("last_evaluation_error_at")
        .map_err(row_error("last_evaluation_error_at"))?;
    match (code, detail, event_id, event_recorded_at) {
        (None, None, None, None) => Ok(None),
        (Some(code), Some(detail), Some(event_id), Some(event_recorded_at)) => {
            Ok(Some(AgentTriggerEvaluationDiagnostic {
                code: AgentTriggerEvaluationErrorCode::parse(&code)?,
                detail,
                event_id,
                event_recorded_at: event_recorded_at.to_rfc3339(),
            }))
        }
        _ => Err(PgError::Query(
            "agent trigger has an incomplete evaluation diagnostic".into(),
        )),
    }
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

fn firing_from_row(row: &sqlx::postgres::PgRow) -> Result<DurableAgentTriggerFiring, PgError> {
    let approval_decision = row
        .try_get::<Option<String>, _>("approval_decision")
        .map_err(row_error("approval_decision"))?
        .as_deref()
        .map(AgentTriggerApprovalDecision::parse)
        .transpose()?;
    Ok(DurableAgentTriggerFiring {
        binding_id: row
            .try_get::<Uuid, _>("binding_id")
            .map_err(row_error("binding_id"))?
            .to_string(),
        event_id: row.try_get("event_id").map_err(row_error("event_id"))?,
        event_type: row.try_get("event_type").map_err(row_error("event_type"))?,
        state: AgentTriggerFiringState::parse(
            &row.try_get::<String, _>("state")
                .map_err(row_error("state"))?,
        )?,
        run_id: row
            .try_get::<Option<Uuid>, _>("run_id")
            .map_err(row_error("run_id"))?
            .map(|id| id.to_string()),
        outcome: AgentTriggerRunOutcome::from_workflow_state(
            row.try_get::<Option<String>, _>("workflow_state")
                .map_err(row_error("workflow_state"))?
                .as_deref(),
        )?,
        terminal_reason: row
            .try_get("terminal_reason")
            .map_err(row_error("terminal_reason"))?,
        approval_decision,
        approval_decided_by: row
            .try_get("approval_decided_by")
            .map_err(row_error("approval_decided_by"))?,
        approval_decided_at: row
            .try_get::<Option<DateTime<Utc>>, _>("approval_decided_at")
            .map_err(row_error("approval_decided_at"))?
            .map(|at| at.to_rfc3339()),
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .map_err(row_error("created_at"))?
            .to_rfc3339(),
    })
}

fn claimed_firing_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<ClaimedAgentTriggerFiring, PgError> {
    let attempts = row
        .try_get::<i32, _>("claim_attempts")
        .map_err(row_error("claim_attempts"))?;
    Ok(ClaimedAgentTriggerFiring {
        binding_id: row
            .try_get::<Uuid, _>("binding_id")
            .map_err(row_error("binding_id"))?
            .to_string(),
        event_id: row.try_get("event_id").map_err(row_error("event_id"))?,
        event_type: row.try_get("event_type").map_err(row_error("event_type"))?,
        event_envelope: row
            .try_get("event_envelope")
            .map_err(row_error("event_envelope"))?,
        owner_principal_id: row
            .try_get("owner_principal_id")
            .map_err(row_error("owner_principal_id"))?,
        run_as_agent_id: row
            .try_get::<Uuid, _>("run_as_agent_id")
            .map_err(row_error("run_as_agent_id"))?
            .to_string(),
        runtime_ref: row
            .try_get("runtime_ref")
            .map_err(row_error("runtime_ref"))?,
        task: row.try_get("task").map_err(row_error("task"))?,
        delegation_caveats: row
            .try_get("delegation_caveats")
            .map_err(row_error("delegation_caveats"))?,
        budget_minor_units: positive_u64(row, "budget_minor_units")?,
        claim_owner: row
            .try_get("claim_owner")
            .map_err(row_error("claim_owner"))?,
        claim_until: row
            .try_get::<DateTime<Utc>, _>("claim_until")
            .map_err(row_error("claim_until"))?
            .to_rfc3339(),
        claim_attempts: u32::try_from(attempts)
            .map_err(|_| PgError::Query("negative durable trigger claim_attempts".into()))?,
    })
}

fn started_run_from_row(row: &sqlx::postgres::PgRow) -> Result<StartedAgentTriggerRun, PgError> {
    Ok(StartedAgentTriggerRun {
        binding_id: row
            .try_get::<Uuid, _>("binding_id")
            .map_err(row_error("binding_id"))?
            .to_string(),
        event_id: row.try_get("event_id").map_err(row_error("event_id"))?,
        event_type: row.try_get("event_type").map_err(row_error("event_type"))?,
        event_envelope: row
            .try_get("event_envelope")
            .map_err(row_error("event_envelope"))?,
        owner_principal_id: row
            .try_get("owner_principal_id")
            .map_err(row_error("owner_principal_id"))?,
        run_as_agent_id: row
            .try_get::<Uuid, _>("run_as_agent_id")
            .map_err(row_error("run_as_agent_id"))?
            .to_string(),
        runtime_ref: row
            .try_get("runtime_ref")
            .map_err(row_error("runtime_ref"))?,
        selected_tools: row.try_get("tools").map_err(row_error("tools"))?,
        task: row.try_get("task").map_err(row_error("task"))?,
        delegation_caveats: row
            .try_get("delegation_caveats")
            .map_err(row_error("delegation_caveats"))?,
        budget_minor_units: positive_u64(row, "budget_minor_units")?,
        run_id: row
            .try_get::<Uuid, _>("run_id")
            .map_err(row_error("run_id"))?
            .to_string(),
    })
}

fn positive_u64(row: &sqlx::postgres::PgRow, column: &'static str) -> Result<u64, PgError> {
    let value = row.try_get::<i64, _>(column).map_err(row_error(column))?;
    u64::try_from(value).map_err(|_| PgError::Query(format!("negative agent trigger `{column}`")))
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{operation}: {error}"))
}

fn row_error(column: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("decode agent trigger `{column}`: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_capacity_reports_the_constraint_an_owner_can_act_on_first() {
        let capacity = AgentTriggerCapacity {
            per_owner_event: 2,
            per_event: 3,
        };

        assert_eq!(capacity_scope(1, 2, capacity), None);
        assert_eq!(
            capacity_scope(2, 2, capacity),
            Some(AgentTriggerCapacityScope::OwnerEvent)
        );
        assert_eq!(
            capacity_scope(1, 3, capacity),
            Some(AgentTriggerCapacityScope::TenantEvent)
        );
        assert_eq!(
            capacity_scope(2, 3, capacity),
            Some(AgentTriggerCapacityScope::OwnerEvent)
        );
    }

    #[test]
    fn trigger_lifecycle_has_idempotent_targets_and_irreversible_disablement() {
        assert_eq!(
            lifecycle_transition("active", AgentTriggerLifecycleAction::Pause),
            Some(true)
        );
        assert_eq!(
            lifecycle_transition("paused", AgentTriggerLifecycleAction::Pause),
            Some(false)
        );
        assert_eq!(
            lifecycle_transition("paused", AgentTriggerLifecycleAction::Resume),
            Some(true)
        );
        assert_eq!(
            lifecycle_transition("active", AgentTriggerLifecycleAction::Resume),
            Some(false)
        );
        assert_eq!(
            lifecycle_transition("active", AgentTriggerLifecycleAction::Disable),
            Some(true)
        );
        assert_eq!(
            lifecycle_transition("paused", AgentTriggerLifecycleAction::Disable),
            Some(true)
        );
        assert_eq!(
            lifecycle_transition("disabled", AgentTriggerLifecycleAction::Disable),
            Some(false)
        );
        assert_eq!(
            lifecycle_transition("disabled", AgentTriggerLifecycleAction::Resume),
            None,
            "a retired automation cannot be revived accidentally"
        );
    }
}
