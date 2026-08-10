use std::future::Future;
use std::pin::Pin;

use myelin_agent_service::hosted_run_contract::{AGENT_RUN_WORKFLOW, AGENT_RUN_WORKFLOW_VERSION};
use myelin_events::EventEnvelope;
use myelin_flow::{
    DelegationCaveats, PgClaimedDriveInput, PgInputResolveError, PgWorkflowInputResolver,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::HOSTED_LUNA_RUNTIME;
use myelin_storage::{
    DurableAgentTriggerBacking, DurablePrincipalBacking, DurablePrincipalRow, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::LlmRunTask;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostedAgentWorkflowInput {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub agent_id: String,
    pub agent: Principal,
    pub trigger_actor: Principal,
    pub task: String,
    pub delegation_caveats: Vec<String>,
    pub selected_tools: Vec<String>,
    pub event: EventEnvelope,
}

impl HostedAgentWorkflowInput {
    pub fn llm_task(&self, now_secs: i64) -> LlmRunTask {
        LlmRunTask::new(
            self.tenant.clone(),
            self.agent.clone(),
            self.agent.principal_id.0.clone(),
            self.run_id.clone(),
            "You are a hosted software-development agent. You are labelled as an agent. Use only \
             the tools and authority delegated for this run. Prefer the smallest safe change.",
            format!(
                "{}\n\nTrigger: {} on {}\nEvent payload: {}",
                self.task, self.event.type_.0, self.event.subject.0, self.event.payload
            ),
        )
        .with_delegation(
            self.trigger_actor.clone(),
            DelegationCaveats(self.delegation_caveats.clone()),
        )
        .with_now_secs(now_secs)
    }
}

#[derive(Clone)]
pub struct HostedAgentInputResolver {
    triggers: DurableAgentTriggerBacking,
    principals: DurablePrincipalBacking,
}

impl HostedAgentInputResolver {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self {
            triggers: DurableAgentTriggerBacking::new(provider.clone()),
            principals: DurablePrincipalBacking::new(provider),
        }
    }

    async fn resolve_input(
        &self,
        input: PgClaimedDriveInput,
    ) -> Result<Vec<u8>, PgInputResolveError> {
        if input.wf_type != AGENT_RUN_WORKFLOW || input.wf_version != AGENT_RUN_WORKFLOW_VERSION {
            return Err(PgInputResolveError::Permanent(
                "hosted resolver received a different workflow definition".into(),
            ));
        }
        Uuid::parse_str(&input.run_id).map_err(|_| {
            PgInputResolveError::Permanent("agent workflow run_id is not a UUID".into())
        })?;
        let started = self
            .triggers
            .started_for_run(&input.tenant.0, &input.run_id)
            .await
            .map_err(|error| PgInputResolveError::Retry(error.to_string()))?
            .ok_or_else(|| {
                PgInputResolveError::Permanent(
                    "agent workflow has no live governed firing authority".into(),
                )
            })?;
        if started.runtime_ref != HOSTED_LUNA_RUNTIME || started.run_id != input.run_id {
            return Err(PgInputResolveError::Permanent(
                "agent workflow resolved to a different runtime or run".into(),
            ));
        }
        let event: EventEnvelope = serde_json::from_value(started.event_envelope)
            .map_err(|error| PgInputResolveError::Permanent(format!("invalid event: {error}")))?;
        if event.tenant != input.tenant
            || event.region != input.region
            || event.event_id.0 != started.event_id
            || event.type_.0 != started.event_type
            || input.input.as_slice() != [event.subject.clone()]
        {
            return Err(PgInputResolveError::Permanent(
                "agent workflow input does not match its immutable trigger event".into(),
            ));
        }

        let agent_principal_id = format!("agent:{}", started.run_as_agent_id);
        let trigger_actor = self
            .load_principal(&input.tenant, &input.region, &started.owner_principal_id)
            .await?;
        let agent = self
            .load_principal(&input.tenant, &input.region, &agent_principal_id)
            .await?;
        if trigger_actor.kind != PrincipalKind::Human {
            return Err(PgInputResolveError::Permanent(
                "governed firing owner is not a human".into(),
            ));
        }
        match &agent.kind {
            PrincipalKind::Agent {
                runtime_ref,
                on_behalf_of: Some(owner),
            } if runtime_ref.0 == HOSTED_LUNA_RUNTIME && owner == &trigger_actor.principal_id => {}
            _ => {
                return Err(PgInputResolveError::Permanent(
                    "run-as principal is not the owner's hosted agent".into(),
                ))
            }
        }

        serde_json::to_vec(&HostedAgentWorkflowInput {
            tenant: input.tenant,
            region: input.region,
            run_id: input.run_id,
            agent_id: started.run_as_agent_id,
            agent,
            trigger_actor,
            task: started.task,
            delegation_caveats: started.delegation_caveats,
            selected_tools: started.selected_tools,
            event,
        })
        .map_err(|error| {
            PgInputResolveError::Permanent(format!("encode hosted run input: {error}"))
        })
    }

    async fn load_principal(
        &self,
        tenant: &TenantId,
        region: &Region,
        principal_id: &str,
    ) -> Result<Principal, PgInputResolveError> {
        let row = self
            .principals
            .get_principal(&tenant.0, principal_id)
            .await
            .map_err(|error| PgInputResolveError::Retry(error.to_string()))?
            .ok_or_else(|| {
                PgInputResolveError::Permanent(format!(
                    "governed principal `{principal_id}` no longer exists"
                ))
            })?;
        principal_from_row(tenant, region, row)
    }
}

impl PgWorkflowInputResolver for HostedAgentInputResolver {
    fn resolve(
        &self,
        input: PgClaimedDriveInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PgInputResolveError>> + Send + '_>> {
        Box::pin(self.resolve_input(input))
    }
}

fn principal_from_row(
    tenant: &TenantId,
    region: &Region,
    row: DurablePrincipalRow,
) -> Result<Principal, PgInputResolveError> {
    let kind = serde_json::from_str(&row.kind)
        .map_err(|_| PgInputResolveError::Permanent("governed principal kind is corrupt".into()))?;
    let data_role: DataRole = serde_json::from_str(&row.data_role).map_err(|_| {
        PgInputResolveError::Permanent("governed principal data role is corrupt".into())
    })?;
    let status: PrincipalStatus = serde_json::from_str(&row.status).map_err(|_| {
        PgInputResolveError::Permanent("governed principal status is corrupt".into())
    })?;
    if status != PrincipalStatus::Active {
        return Err(PgInputResolveError::Permanent(format!(
            "governed principal `{}` is not active",
            row.principal_id
        )));
    }
    Ok(Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId(row.principal_id),
        kind,
        data_role,
        status,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole as EventDataRole, EventId, EventType,
        Timestamp, Visibility,
    };
    use myelin_identity::RuntimeRef;
    use myelin_tenancy::ArtifactRef;

    #[test]
    fn resolved_work_becomes_a_clear_agent_prompt_without_losing_delegation() {
        let tenant = TenantId("acme".into());
        let region = Region("fr-par".into());
        let founder = Principal::new(
            tenant.clone(),
            region.clone(),
            PrincipalId("founder".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let agent = Principal::new(
            tenant.clone(),
            region.clone(),
            PrincipalId("agent:a1".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef(HOSTED_LUNA_RUNTIME.into()),
                on_behalf_of: Some(founder.principal_id.clone()),
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let event = EventEnvelope {
            event_id: EventId("event-1".into()),
            type_: EventType("ci.run.failed".into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(Principal::stub(
                PrincipalId("ci".into()),
                PrincipalKind::Service,
                tenant.clone(),
            )),
            subject: ArtifactRef("myelin://acme/ci/run/r1".into()),
            aggregate: AggregateKey("ci:r1".into()),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: EventDataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-08-10T00:00:00Z".into()),
            recorded_at: Timestamp("2026-08-10T00:00:01Z".into()),
            payload: serde_json::json!({"source_ref": "refs/heads/main"}),
        };
        let resolved = HostedAgentWorkflowInput {
            tenant,
            region,
            run_id: "run-1".into(),
            agent_id: "a1".into(),
            agent,
            trigger_actor: founder.clone(),
            task: "Prepare the smallest safe fix.".into(),
            delegation_caveats: vec!["repo:core".into()],
            selected_tools: vec!["git.open_pr.v1".into()],
            event,
        };
        let task = resolved.llm_task(42);
        assert_eq!(task.trigger_actor, founder);
        assert_eq!(task.delegation_caveats.0, ["repo:core"]);
        assert!(task.prompt.contains("Prepare the smallest safe fix."));
        assert!(task.prompt.contains("ci.run.failed"));
        assert!(task.prompt.contains("refs/heads/main"));
    }
}
