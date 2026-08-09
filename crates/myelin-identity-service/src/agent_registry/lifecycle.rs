use chrono::{DateTime, SecondsFormat, Utc};
use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole as EventDataRole, EmitContext, EventDraft,
    EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalKind, PrincipalStatus, IDENTITY_AGENT_STATUS_CHANGED};
use myelin_storage::pgrelay::PgRelay;
use sqlx::Row;
use uuid::Uuid;

use super::{
    agent_from_joined_row, agent_from_row, agent_principal_id, agent_ref, decode_error,
    parse_agent_id, query_error, AgentRegistration, AgentRegistryError, PgAgentRegistry,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentLifecycleAction {
    Suspend,
    Resume,
    Retire,
}

impl AgentLifecycleAction {
    fn target(self) -> PrincipalStatus {
        match self {
            Self::Suspend => PrincipalStatus::Suspended,
            Self::Resume => PrincipalStatus::Active,
            Self::Retire => PrincipalStatus::Disabled,
        }
    }

    fn token(self) -> &'static str {
        match self {
            Self::Suspend => "suspend",
            Self::Resume => "resume",
            Self::Retire => "retire",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentLifecycleRequest {
    pub agent_id: String,
    pub action: AgentLifecycleAction,
    pub client_nonce: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentLifecycleOutcome {
    pub agent: AgentRegistration,
    pub changed: bool,
    pub terminated_runs: u64,
}

impl PgAgentRegistry {
    /**
     * Change an external agent's lifecycle state and revoke all unfinished runs atomically.
     *
     * The principal row is the serialization point shared with run creation. A suspension or
     * retirement can therefore never commit while leaving a provisioning/ready bearer live.
     */
    pub async fn change_status(
        &self,
        actor: &Principal,
        request: AgentLifecycleRequest,
    ) -> Result<AgentLifecycleOutcome, AgentRegistryError> {
        require_active_human(actor)?;
        self.require_local_region(actor)?;
        let agent_id = parse_agent_id(&request.agent_id)?;
        validate_nonce(&request.client_nonce)?;

        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        let actor_id = actor.principal_id.0.clone();
        let event_actor = actor.clone();
        let request_for_tx = request.clone();
        let event_id = EventId(self.event_ids.mint().0);
        let now = Utc::now();
        let outcome = self
            .provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    lock_lifecycle_nonce(
                        conn,
                        &tenant,
                        &region,
                        &actor_id,
                        &request_for_tx.client_nonce,
                    )
                    .await?;
                    if let Some(recorded) = lifecycle_by_nonce(
                        conn,
                        &tenant,
                        &region,
                        &actor_id,
                        &request_for_tx.client_nonce,
                    )
                    .await?
                    {
                        return if recorded.agent.id == request_for_tx.agent_id
                            && recorded.agent.status == request_for_tx.action.target()
                        {
                            Ok(LifecycleTx::Complete(Box::new(recorded)))
                        } else {
                            Ok(LifecycleTx::NonceConflict)
                        };
                    }

                    let Some(mut agent) = lock_agent(conn, &tenant, &region, agent_id).await?
                    else {
                        return Ok(LifecycleTx::NotFound);
                    };
                    let target = request_for_tx.action.target();
                    let Some(changed) = transition_allowed(agent.status, target) else {
                        return Ok(LifecycleTx::RetiredConflict);
                    };
                    let previous = agent.status;
                    let terminated_runs = if target == PrincipalStatus::Active {
                        0
                    } else {
                        terminate_unfinished_runs(conn, &tenant, &region, agent_id).await?
                    };

                    if changed {
                        let encoded = serde_json::to_string(&target)
                            .expect("principal lifecycle status serializes");
                        sqlx::query(
                            "UPDATE principal SET status = $4 \
                              WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&agent.principal_id)
                        .bind(encoded)
                        .execute(&mut *conn)
                        .await
                        .map_err(query_error("change agent principal status"))?;
                        let event = lifecycle_event(
                            &event_actor,
                            &agent,
                            request_for_tx.action,
                            previous,
                            target,
                            terminated_runs,
                            event_id,
                            now,
                        );
                        PgRelay::co_commit_in_tx(conn, &event.aggregate.0, &event).await?;
                    }
                    agent.status = target;

                    sqlx::query(
                        "INSERT INTO agent_lifecycle_command (\
                           tenant_id, region, actor_id, client_nonce, agent_id, requested_status, \
                           changed, terminated_runs, occurred_at\
                         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&actor_id)
                    .bind(&request_for_tx.client_nonce)
                    .bind(agent_id)
                    .bind(status_token(target))
                    .bind(changed)
                    .bind(i64::try_from(terminated_runs).map_err(|_| {
                        myelin_storage::PgError::Query(
                            "terminated agent-run count exceeded i64".into(),
                        )
                    })?)
                    .bind(now)
                    .execute(&mut *conn)
                    .await
                    .map_err(query_error("record agent lifecycle command"))?;

                    Ok(LifecycleTx::Complete(Box::new(AgentLifecycleOutcome {
                        agent,
                        changed,
                        terminated_runs,
                    })))
                })
            })
            .await
            .map_err(|error| AgentRegistryError::Storage(error.to_string()))?;

        match outcome {
            LifecycleTx::Complete(outcome) => Ok(*outcome),
            LifecycleTx::NotFound => Err(AgentRegistryError::NotFound),
            LifecycleTx::NonceConflict => Err(AgentRegistryError::Conflict(
                "that idempotency key was already used for a different agent lifecycle change"
                    .into(),
            )),
            LifecycleTx::RetiredConflict => Err(AgentRegistryError::Conflict(
                "a retired agent cannot be resumed or suspended".into(),
            )),
        }
    }
}

enum LifecycleTx {
    Complete(Box<AgentLifecycleOutcome>),
    NotFound,
    NonceConflict,
    RetiredConflict,
}

fn require_active_human(actor: &Principal) -> Result<(), AgentRegistryError> {
    if actor.kind == PrincipalKind::Human && actor.status == PrincipalStatus::Active {
        Ok(())
    } else {
        Err(AgentRegistryError::BadInput(
            "agent lifecycle changes require an active Human actor".into(),
        ))
    }
}

fn validate_nonce(nonce: &str) -> Result<(), AgentRegistryError> {
    if !nonce.is_empty() && nonce.len() <= 128 && nonce.bytes().all(|byte| byte.is_ascii_graphic())
    {
        Ok(())
    } else {
        Err(AgentRegistryError::BadInput(
            "client nonce must be 1..=128 ASCII-graphic bytes".into(),
        ))
    }
}

fn transition_allowed(observed: PrincipalStatus, target: PrincipalStatus) -> Option<bool> {
    match (observed, target) {
        (PrincipalStatus::Disabled, PrincipalStatus::Active | PrincipalStatus::Suspended) => None,
        (observed, target) => Some(observed != target),
    }
}

async fn lock_lifecycle_nonce(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    actor: &str,
    nonce: &str,
) -> Result<(), myelin_storage::PgError> {
    let identity = format!("agent-lifecycle:{region}:{actor}:{nonce}");
    sqlx::query(
        "SELECT pg_advisory_xact_lock(\
            hashtextextended(length($2)::text || ':' || $2 || ':' || $1, 0))",
    )
    .bind(identity)
    .bind(tenant)
    .execute(&mut *conn)
    .await
    .map_err(query_error("lock agent lifecycle identity"))?;
    Ok(())
}

async fn lifecycle_by_nonce(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    actor: &str,
    nonce: &str,
) -> Result<Option<AgentLifecycleOutcome>, myelin_storage::PgError> {
    let row = sqlx::query(
        "SELECT a.agent_id, a.name, a.runtime_ref, a.created_by, a.tools, a.grants, \
                a.created_at, c.requested_status, c.changed, c.terminated_runs \
           FROM agent_lifecycle_command c \
           JOIN identity_agent a ON a.tenant_id = c.tenant_id AND a.region = c.region \
            AND a.agent_id = c.agent_id \
          WHERE c.tenant_id = $1 AND c.region = $2 AND c.actor_id = $3 \
            AND c.client_nonce = $4",
    )
    .bind(tenant)
    .bind(region)
    .bind(actor)
    .bind(nonce)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("read agent lifecycle idempotency record"))?;
    row.map(|row| {
        let status = parse_status(
            &row.try_get::<String, _>("requested_status")
                .map_err(decode_error("agent lifecycle requested status"))?,
        )?;
        let terminated_runs = row
            .try_get::<i64, _>("terminated_runs")
            .map_err(decode_error("agent lifecycle terminated runs"))?;
        Ok(AgentLifecycleOutcome {
            agent: agent_from_row(&row, status)?,
            changed: row
                .try_get("changed")
                .map_err(decode_error("agent lifecycle changed flag"))?,
            terminated_runs: u64::try_from(terminated_runs).map_err(|_| {
                myelin_storage::PgError::Query(
                    "agent lifecycle terminated-run count is negative".into(),
                )
            })?,
        })
    })
    .transpose()
}

async fn lock_agent(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    agent_id: Uuid,
) -> Result<Option<AgentRegistration>, myelin_storage::PgError> {
    let row = sqlx::query(
        "SELECT a.agent_id, a.name, a.runtime_ref, a.created_by, a.tools, a.grants, \
                a.created_at, p.kind, p.status \
           FROM identity_agent a \
           JOIN principal p ON p.tenant_id = a.tenant_id AND p.region = a.region \
            AND p.principal_id = 'agent:' || a.agent_id::text \
          WHERE a.tenant_id = $1 AND a.region = $2 AND a.agent_id = $3 \
          FOR UPDATE OF p",
    )
    .bind(tenant)
    .bind(region)
    .bind(agent_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(query_error("lock agent for lifecycle change"))?;
    row.as_ref().map(agent_from_joined_row).transpose()
}

async fn terminate_unfinished_runs(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    agent_id: Uuid,
) -> Result<u64, myelin_storage::PgError> {
    sqlx::query(
        "INSERT INTO run_token_teardown (tenant_id, region, jti) \
         SELECT tenant_id, region, token_jti FROM external_agent_run \
          WHERE tenant_id = $1 AND region = $2 AND agent_id = $3 \
            AND state IN ('provisioning', 'ready') \
         ON CONFLICT DO NOTHING",
    )
    .bind(tenant)
    .bind(region)
    .bind(agent_id)
    .execute(&mut *conn)
    .await
    .map_err(query_error("revoke unfinished agent runs"))?;
    let changed = sqlx::query(
        "UPDATE external_agent_run SET state = 'terminal' \
          WHERE tenant_id = $1 AND region = $2 AND agent_id = $3 \
            AND state IN ('provisioning', 'ready')",
    )
    .bind(tenant)
    .bind(region)
    .bind(agent_id)
    .execute(&mut *conn)
    .await
    .map_err(query_error("terminate unfinished agent runs"))?
    .rows_affected();
    Ok(changed)
}

#[allow(clippy::too_many_arguments)]
fn lifecycle_event(
    actor: &Principal,
    agent: &AgentRegistration,
    action: AgentLifecycleAction,
    previous: PrincipalStatus,
    status: PrincipalStatus,
    terminated_runs: u64,
    event_id: EventId,
    now: DateTime<Utc>,
) -> myelin_events::EventEnvelope {
    let timestamp = Timestamp(now.to_rfc3339_opts(SecondsFormat::Micros, true));
    derive_envelope(
        EventDraft {
            type_: EventType(IDENTITY_AGENT_STATUS_CHANGED.into()),
            subject: agent_ref(&actor.tenant.0, &agent.id),
            aggregate: AggregateKey(format!("identity:agent:{}", agent.id)),
            payload: serde_json::json!({
                "agent_id": agent.id,
                "principal_id": agent_principal_id(
                    Uuid::parse_str(&agent.id).expect("stored agent id is a UUID")
                ),
                "action": action.token(),
                "previous_status": status_token(previous),
                "status": status_token(status),
                "terminated_runs": terminated_runs,
            }),
            data_role: EventDataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: actor.tenant.clone(),
            region: actor.region.clone(),
            actor: Actor(actor.clone()),
            schema_ver: 1,
            occurred_at: timestamp.clone(),
            recorded_at: timestamp,
            caused_by: None,
        },
        None,
    )
}

fn status_token(status: PrincipalStatus) -> &'static str {
    match status {
        PrincipalStatus::Active => "Active",
        PrincipalStatus::Suspended => "Suspended",
        PrincipalStatus::Disabled => "Disabled",
    }
}

fn parse_status(value: &str) -> Result<PrincipalStatus, myelin_storage::PgError> {
    match value {
        "Active" => Ok(PrincipalStatus::Active),
        "Suspended" => Ok(PrincipalStatus::Suspended),
        "Disabled" => Ok(PrincipalStatus::Disabled),
        _ => Err(myelin_storage::PgError::Query(
            "agent lifecycle command has an invalid status".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retirement_is_absorbing_while_suspension_is_reversible() {
        assert_eq!(
            transition_allowed(PrincipalStatus::Active, PrincipalStatus::Suspended),
            Some(true)
        );
        assert_eq!(
            transition_allowed(PrincipalStatus::Suspended, PrincipalStatus::Active),
            Some(true)
        );
        assert_eq!(
            transition_allowed(PrincipalStatus::Active, PrincipalStatus::Disabled),
            Some(true)
        );
        assert_eq!(
            transition_allowed(PrincipalStatus::Disabled, PrincipalStatus::Disabled),
            Some(false)
        );
        assert_eq!(
            transition_allowed(PrincipalStatus::Disabled, PrincipalStatus::Active),
            None
        );
        assert_eq!(
            transition_allowed(PrincipalStatus::Disabled, PrincipalStatus::Suspended),
            None
        );
    }
}
