use super::lifecycle::terminate_unfinished_runs;
use super::{
    agent_from_joined_row, agent_from_row, agent_ref, canonicalize, parse_agent_id, query_error,
    AgentRegistration, AgentRegistryError, PgAgentRegistry, MAX_AGENT_TOOLS,
};
use chrono::{DateTime, SecondsFormat, Utc};
use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole as EventDataRole, EmitContext, EventDraft,
    EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalKind, PrincipalStatus, IDENTITY_AGENT_TOOLS_UPDATED};
use myelin_storage::{
    replace_agent_policy_bundle_on_conn, DurableDelegationPolicyRevisions,
    DurableDelegationPolicyVersions,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolPaletteRequest {
    pub agent_id: String,
    pub expected_tools: Vec<String>,
    pub tools: Vec<String>,
    pub grants: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolPaletteOutcome {
    pub agent: AgentRegistration,
    pub changed: bool,
    pub terminated_runs: u64,
    pub policy_versions: DurableDelegationPolicyVersions,
    pub policy_revisions: DurableDelegationPolicyRevisions,
}

impl PgAgentRegistry {
    /// Replace an owned agent's exact tool and grant set, invalidating old policy snapshots and
    /// terminating unfinished runs in the same transaction as the registration update.
    pub async fn update_tool_palette(
        &self,
        actor: &Principal,
        mut request: AgentToolPaletteRequest,
    ) -> Result<AgentToolPaletteOutcome, AgentRegistryError> {
        if actor.kind != PrincipalKind::Human || actor.status != PrincipalStatus::Active {
            return Err(AgentRegistryError::BadInput(
                "agent tool updates require an active Human owner".into(),
            ));
        }
        canonicalize(&mut request.expected_tools);
        canonicalize(&mut request.tools);
        canonicalize(&mut request.grants);
        if request.tools.is_empty()
            || request.tools.len() > MAX_AGENT_TOOLS
            || request.grants.is_empty()
        {
            return Err(AgentRegistryError::BadInput(
                "agent tool updates require a bounded non-empty tool and grant set".into(),
            ));
        }
        self.require_local_region(actor)?;
        let agent_id = parse_agent_id(&request.agent_id)?;
        let tenant = actor.tenant.0.clone();
        let region = actor.region.0.clone();
        let owner = actor.principal_id.0.clone();
        let event_actor = actor.clone();
        let event_id = EventId(self.event_ids.mint().0);
        let now = Utc::now();
        let outcome = self
            .provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
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
                    .map_err(query_error("lock agent tool palette"))?;
                    let Some(row) = row else {
                        return Ok(ToolPaletteTx::NotFound);
                    };
                    let current = agent_from_joined_row(&row)?;
                    if current.status == PrincipalStatus::Disabled {
                        return Ok(ToolPaletteTx::Retired);
                    }
                    let exact_retry = current.tools == request.tools && current.grants == request.grants;
                    if !exact_retry && tool_names(&current.tools).as_deref() != Some(&request.expected_tools) {
                        return Ok(ToolPaletteTx::Conflict);
                    }
                    let policies = replace_agent_policy_bundle_on_conn(
                        conn,
                        &tenant,
                        &region,
                        &current.principal_id,
                        &owner,
                        request.grants.clone(),
                    )
                    .await?;
                    let (policy_versions, policy_revisions) = match policies {
                        Ok(cursors) => cursors,
                        Err(error) => return Ok(ToolPaletteTx::Policy(error.to_string())),
                    };
                    if exact_retry {
                        return Ok(ToolPaletteTx::Complete(Box::new(AgentToolPaletteOutcome {
                            agent: current,
                            changed: false,
                            terminated_runs: 0,
                            policy_versions,
                            policy_revisions,
                        })));
                    }
                    let terminated_runs =
                        terminate_unfinished_runs(conn, &tenant, &region, agent_id).await?;
                    let row = sqlx::query(
                        "UPDATE identity_agent SET tools = $4, grants = $5 \
                          WHERE tenant_id = $1 AND region = $2 AND agent_id = $3 \
                          RETURNING tenant_id, agent_id, name, runtime_ref, created_by, tools, grants, \
                            workspace_guidance_revision, workspace_guidance_digest, \
                            workspace_guidance_key_ref, workspace_guidance_nonce, \
                            workspace_guidance_ciphertext, workspace_guidance_updated_at, created_at",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(agent_id)
                    .bind(&request.tools)
                    .bind(&request.grants)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(query_error("update agent tool palette"))?;
                    let agent = agent_from_row(&row, current.status)?;
                    let event = tool_palette_event(
                        &event_actor,
                        &agent,
                        &current.tools,
                        terminated_runs,
                        event_id,
                        now,
                    );
                    myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
                        conn,
                        &event.aggregate.0,
                        &event,
                    )
                    .await?;
                    Ok(ToolPaletteTx::Complete(Box::new(AgentToolPaletteOutcome {
                        agent,
                        changed: true,
                        terminated_runs,
                        policy_versions,
                        policy_revisions,
                    })))
                })
            })
            .await
            .map_err(|error| AgentRegistryError::Storage(error.to_string()))?;
        match outcome {
            ToolPaletteTx::Complete(outcome) => Ok(*outcome),
            ToolPaletteTx::NotFound => Err(AgentRegistryError::NotFound),
            ToolPaletteTx::Conflict => Err(AgentRegistryError::Conflict(
                "the agent tool palette changed; refresh before replacing it".into(),
            )),
            ToolPaletteTx::Retired => Err(AgentRegistryError::Conflict(
                "a retired agent cannot be reconfigured".into(),
            )),
            ToolPaletteTx::Policy(message) => Err(AgentRegistryError::Policy(message)),
        }
    }
}

fn tool_names(cursors: &[String]) -> Option<Vec<String>> {
    let mut names = Vec::with_capacity(cursors.len());
    for cursor in cursors {
        let (name, version) = cursor.rsplit_once(".v")?;
        if name.is_empty()
            || version
                .parse::<u16>()
                .ok()
                .filter(|value| *value > 0)
                .is_none()
        {
            return None;
        }
        names.push(name.to_string());
    }
    canonicalize(&mut names);
    Some(names)
}

enum ToolPaletteTx {
    Complete(Box<AgentToolPaletteOutcome>),
    NotFound,
    Conflict,
    Retired,
    Policy(String),
}

fn tool_palette_event(
    actor: &Principal,
    agent: &AgentRegistration,
    previous_tools: &[String],
    terminated_runs: u64,
    event_id: EventId,
    now: DateTime<Utc>,
) -> myelin_events::EventEnvelope {
    let timestamp = Timestamp(now.to_rfc3339_opts(SecondsFormat::Micros, true));
    derive_envelope(
        EventDraft {
            type_: EventType(IDENTITY_AGENT_TOOLS_UPDATED.into()),
            subject: agent_ref(&actor.tenant.0, &agent.id),
            aggregate: AggregateKey(format!("identity:agent:{}", agent.id)),
            payload: serde_json::json!({
                "agent_id": agent.id,
                "previous_tools": previous_tools,
                "tools": agent.tools,
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

#[cfg(test)]
mod tests {
    use super::tool_names;

    #[test]
    fn optimistic_names_survive_catalogue_version_advances_without_accepting_bad_cursors() {
        assert_eq!(
            tool_names(&["workspace.read_file.v2".into(), "chat.post.v1".into()]),
            Some(vec!["chat.post".into(), "workspace.read_file".into()]),
        );
        assert_eq!(tool_names(&["workspace.read_file".into()]), None);
        assert_eq!(tool_names(&["workspace.read_file.v0".into()]), None);
        assert_eq!(tool_names(&["workspace.read_file.vbogus".into()]), None);
    }
}
