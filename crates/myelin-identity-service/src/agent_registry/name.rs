use super::{
    agent_from_joined_row, agent_from_row, agent_name_exists, agent_ref, parse_agent_id,
    query_error, validate_agent_name, AgentRegistration, AgentRegistryError, PgAgentRegistry,
};
use chrono::{DateTime, SecondsFormat, Utc};
use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole as EventDataRole, EmitContext, EventDraft,
    EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalKind, PrincipalStatus, IDENTITY_AGENT_NAME_UPDATED};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentNameRequest {
    pub agent_id: String,
    pub expected_name: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentNameOutcome {
    pub agent: AgentRegistration,
    pub changed: bool,
}

impl PgAgentRegistry {
    /// Renames an owned agent without changing its runtime, policy, tools, or live runs.
    pub async fn update_name(
        &self,
        actor: &Principal,
        request: AgentNameRequest,
    ) -> Result<AgentNameOutcome, AgentRegistryError> {
        if actor.kind != PrincipalKind::Human || actor.status != PrincipalStatus::Active {
            return Err(AgentRegistryError::BadInput(
                "agent name updates require an active Human owner".into(),
            ));
        }
        validate_agent_name(&request.expected_name)?;
        validate_agent_name(&request.name)?;
        self.require_local_region(actor)?;
        let agent_id = parse_agent_id(&request.agent_id)?;
        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        let owner = actor.principal_id.0.clone();
        let event_actor = actor.clone();
        let event_id = EventId(self.event_ids.mint().0);
        let now = Utc::now();
        let requested_name = request.name.clone();

        let outcome = self
            .provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    lock_agent_name(conn, &tenant, &region, &request.name).await?;
                    let row = sqlx::query(
                        "SELECT a.tenant_id, a.agent_id, a.name, a.runtime_ref, a.created_by, \
                                a.tools, a.grants, a.workspace_guidance_revision, \
                                a.workspace_guidance_digest, a.workspace_guidance_key_ref, \
                                a.workspace_guidance_nonce, a.workspace_guidance_ciphertext, \
                                a.workspace_guidance_updated_at, a.created_at, p.kind, p.status \
                           FROM identity_agent a \
                           JOIN principal p ON p.tenant_id = a.tenant_id AND p.region = a.region \
                            AND p.principal_id = 'agent:' || a.agent_id::text \
                          WHERE a.tenant_id = $1 AND a.region = $2 AND a.agent_id = $3 \
                            AND a.created_by = $4 \
                          FOR UPDATE OF p, a",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(agent_id)
                    .bind(&owner)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(query_error("lock agent name"))?;
                    let Some(row) = row else {
                        return Ok(NameTx::NotFound);
                    };
                    let current = agent_from_joined_row(&row)?;
                    if current.status == PrincipalStatus::Disabled {
                        return Ok(NameTx::Retired);
                    }
                    if current.name == request.name {
                        return Ok(NameTx::Complete(Box::new(AgentNameOutcome {
                            agent: current,
                            changed: false,
                        })));
                    }
                    if current.name != request.expected_name {
                        return Ok(NameTx::Conflict);
                    }
                    if agent_name_exists(conn, &tenant, &region, &request.name).await? {
                        return Ok(NameTx::NameConflict);
                    }
                    let row = sqlx::query(
                        "UPDATE identity_agent SET name = $4 \
                          WHERE tenant_id = $1 AND region = $2 AND agent_id = $3 \
                          RETURNING tenant_id, agent_id, name, runtime_ref, created_by, tools, grants, \
                            workspace_guidance_revision, workspace_guidance_digest, \
                            workspace_guidance_key_ref, workspace_guidance_nonce, \
                            workspace_guidance_ciphertext, workspace_guidance_updated_at, created_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(agent_id)
                    .bind(&request.name)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(query_error("update agent name"))?;
                    let agent = agent_from_row(&row, current.status)?;
                    let event = name_updated_event(&event_actor, &agent, event_id, now);
                    myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
                        conn,
                        &event.aggregate.0,
                        &event,
                    )
                    .await?;
                    Ok(NameTx::Complete(Box::new(AgentNameOutcome {
                        agent,
                        changed: true,
                    })))
                })
            })
            .await
            .map_err(|error| AgentRegistryError::Storage(error.to_string()))?;

        match outcome {
            NameTx::Complete(outcome) => Ok(*outcome),
            NameTx::NotFound => Err(AgentRegistryError::NotFound),
            NameTx::Conflict => Err(AgentRegistryError::Conflict(
                "the agent name changed; refresh before replacing it".into(),
            )),
            NameTx::NameConflict => Err(AgentRegistryError::Conflict(format!(
                "an agent named `{}` already exists in this organization",
                requested_name
            ))),
            NameTx::Retired => Err(AgentRegistryError::Conflict(
                "a retired agent cannot be reconfigured".into(),
            )),
        }
    }
}

enum NameTx {
    Complete(Box<AgentNameOutcome>),
    NotFound,
    Conflict,
    NameConflict,
    Retired,
}

async fn lock_agent_name(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    name: &str,
) -> Result<(), myelin_storage::PgError> {
    let identity = format!("agent-name:{region}:{name}");
    sqlx::query(
        "SELECT pg_advisory_xact_lock(\
            hashtextextended(length($2)::text || ':' || $2 || ':' || $1, 0))",
    )
    .bind(identity)
    .bind(tenant)
    .execute(&mut *conn)
    .await
    .map_err(query_error("lock agent name identity"))?;
    Ok(())
}

fn name_updated_event(
    actor: &Principal,
    agent: &AgentRegistration,
    event_id: EventId,
    now: DateTime<Utc>,
) -> myelin_events::EventEnvelope {
    let timestamp = Timestamp(now.to_rfc3339_opts(SecondsFormat::Micros, true));
    derive_envelope(
        EventDraft {
            type_: EventType(IDENTITY_AGENT_NAME_UPDATED.into()),
            subject: agent_ref(&actor.tenant.0, &agent.id),
            aggregate: AggregateKey(format!("identity:agent:{}", agent.id)),
            payload: serde_json::json!({ "agent_id": agent.id }),
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

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId};
    use myelin_tenancy::{Region, TenantId};

    #[test]
    fn rename_events_are_metadata_only() {
        let actor = Principal::new(
            TenantId("acme".into()),
            Region("fr-par".into()),
            PrincipalId("ada".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let agent = AgentRegistration {
            id: "11111111-1111-4111-8111-111111111111".into(),
            principal_id: "agent:11111111-1111-4111-8111-111111111111".into(),
            name: "Potentially personal name".into(),
            runtime_ref: "external:mcp".into(),
            created_by: "ada".into(),
            tools: vec!["chat.post.v1".into()],
            grants: vec!["chat.messages.write".into()],
            workspace_guidance_revision: 0,
            workspace_guidance: None,
            workspace_guidance_updated_at: None,
            status: PrincipalStatus::Active,
            created_at: "2026-08-31T00:00:00Z".into(),
        };
        let event = name_updated_event(
            &actor,
            &agent,
            EventId("01M1D1D1D1D1D1D1D1D1D1D1D1".into()),
            Utc::now(),
        );
        assert_eq!(event.payload, serde_json::json!({ "agent_id": agent.id }));
        assert!(!event.contains_personal_data);
        assert!(!event.payload.to_string().contains(&agent.name));
    }
}
