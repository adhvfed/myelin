use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use myelin_agent_service::hosted_run_contract::{
    agent_run_definition_hash, gate_ref_token, hosted_agent_decision_ref,
    legacy_agent_run_definition_hash, HostedAgentDecision, AGENT_RUN_WORKFLOW,
    AGENT_RUN_WORKFLOW_VERSION, HOSTED_AGENT_APPROVAL_SIGNAL, LEGACY_AGENT_RUN_WORKFLOW_VERSION,
};
use myelin_events::EventEnvelope;
use myelin_flow::{
    ActivityError, DelegationCaveats, PgClaimedDriveInput, PgFlowWorker, PgInputResolveError,
    PgResolvedDriveInput, PgWorkerError, PgWorkflowInputResolver, RetryPolicy, WaitOutcome,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::HOSTED_LUNA_RUNTIME;
use myelin_storage::{
    hitl_gate_durable::gate_id_from_ref_token, DurableAgentTriggerBacking, DurablePrincipalBacking,
    DurablePrincipalRow, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::LlmRunTask;

const MAX_APPROVALS_PER_HOSTED_RUN: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostedAgentActivityOutcome {
    Completed(myelin_refs::ArtifactRef),
    ApprovalRequired {
        gate_id: String,
        expires_at_unix: i64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostedAgentStopReason {
    Rejected,
    Expired,
}

impl HostedAgentStopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

pub trait HostedAgentRunExecutor: Send + Sync {
    /// Executes an activity with deterministic workflow time as replay metadata.
    ///
    /// Implementations must obtain live time independently for credential and deadline checks.
    fn execute(
        &self,
        input: &HostedAgentWorkflowInput,
        activity_key: &str,
        attempt: u32,
        workflow_time_secs: i64,
    ) -> Result<HostedAgentActivityOutcome, ActivityError>;

    /// Stops an activity without treating deterministic workflow time as wall time.
    fn stop(
        &self,
        input: &HostedAgentWorkflowInput,
        activity_key: &str,
        workflow_time_secs: i64,
        gate_id: &str,
        reason: HostedAgentStopReason,
    ) -> Result<myelin_refs::ArtifactRef, ActivityError>;
}

pub fn register_hosted_agent_workflow(
    worker: &mut PgFlowWorker,
    resolver: HostedAgentInputResolver,
    executor: Arc<dyn HostedAgentRunExecutor>,
) -> Result<(), PgWorkerError> {
    let legacy_executor = executor.clone();
    worker.register_definition_with_input_resolver(
        AGENT_RUN_WORKFLOW,
        LEGACY_AGENT_RUN_WORKFLOW_VERSION,
        &legacy_agent_run_definition_hash(),
        resolver.clone(),
        move |resolved, ctx| {
            let input = decode_hosted_agent_workflow_input(resolved)?;
            let now_secs = workflow_now_secs(ctx)?;
            ctx.activity(RetryPolicy { max_attempts: 1 }, |activity_key, attempt| {
                match legacy_executor.execute(&input, activity_key, attempt, now_secs) {
                    Ok(HostedAgentActivityOutcome::Completed(run_ref)) => Ok(vec![run_ref]),
                    Ok(HostedAgentActivityOutcome::ApprovalRequired { gate_id, .. }) => {
                        Err(ActivityError::permanent(format!(
                            "hosted agent requires approval at `{gate_id}`"
                        )))
                    }
                    Err(error) => Err(error),
                }
            })
            .map_err(|error| format!("legacy hosted agent activity failed: {error:?}"))
        },
    )?;
    worker.register_definition_with_input_resolver(
        AGENT_RUN_WORKFLOW,
        AGENT_RUN_WORKFLOW_VERSION,
        &agent_run_definition_hash(),
        resolver,
        move |resolved, ctx| {
            let input = decode_hosted_agent_workflow_input(resolved)?;
            for _ in 0..MAX_APPROVALS_PER_HOSTED_RUN {
                let now_secs = workflow_now_secs(ctx)?;
                let activity_output = ctx
                    .activity(RetryPolicy::default_policy(), |activity_key, attempt| {
                        executor
                            .execute(&input, activity_key, attempt, now_secs)
                            .map(|outcome| vec![encode_activity_outcome(&input, outcome)])
                    })
                    .map_err(|error| format!("hosted agent activity failed: {error:?}"))?;
                match decode_activity_outcome(&input, &activity_output)? {
                    HostedAgentActivityOutcome::Completed(run_ref) => return Ok(vec![run_ref]),
                    HostedAgentActivityOutcome::ApprovalRequired {
                        gate_id,
                        expires_at_unix,
                    } => {
                        match ctx
                            .wait_for_signal_exact_until(
                                HOSTED_AGENT_APPROVAL_SIGNAL,
                                &gate_id,
                                Some(expires_at_unix),
                            )
                            .map_err(|error| {
                                format!("hosted agent approval wait failed: {error:?}")
                            })?
                        {
                            WaitOutcome::Parked => return Ok(Vec::new()),
                            WaitOutcome::Signalled { payload, .. } => {
                                let Some(reason) = decode_decision(&input, &gate_id, &payload)? else {
                                    continue;
                                };
                                return ctx
                                    .activity(
                                        RetryPolicy::default_policy(),
                                        |activity_key, _attempt| {
                                            executor
                                                .stop(
                                                    &input,
                                                    activity_key,
                                                    now_secs,
                                                    &gate_id,
                                                    reason,
                                                )
                                                .map(|run_ref| vec![run_ref])
                                        },
                                    )
                                    .map_err(|error| {
                                        format!("stop rejected hosted agent run: {error:?}")
                                    });
                            }
                            WaitOutcome::TimedOut => return ctx
                                .activity(
                                    RetryPolicy::default_policy(),
                                    |activity_key, _attempt| {
                                        executor
                                            .stop(
                                                &input,
                                                activity_key,
                                                expires_at_unix,
                                                &gate_id,
                                                HostedAgentStopReason::Expired,
                                            )
                                            .map(|run_ref| vec![run_ref])
                                    },
                                )
                                .map_err(|error| {
                                    format!("expire hosted agent approval: {error:?}")
                                }),
                        }
                    }
                }
            }
            Err(format!(
                "hosted agent exceeded its bounded {MAX_APPROVALS_PER_HOSTED_RUN} approval decisions"
            ))
        },
    )
}

fn workflow_now_secs(ctx: &mut myelin_flow::WfCtx) -> Result<i64, String> {
    chrono::DateTime::parse_from_rfc3339(&ctx.now())
        .map_err(|error| format!("hosted workflow clock is invalid: {error}"))
        .map(|now| now.timestamp())
}

fn run_ref(input: &HostedAgentWorkflowInput) -> myelin_refs::ArtifactRef {
    hosted_agent_run_ref(&input.tenant, &input.run_id)
}

fn hosted_agent_run_ref(tenant: &TenantId, run_id: &str) -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef(format!("myelin://{}/agent/run/{}", tenant.0, run_id))
}

fn gate_ref(
    input: &HostedAgentWorkflowInput,
    gate_id: &str,
    expires_at_unix: i64,
) -> myelin_refs::ArtifactRef {
    myelin_refs::ArtifactRef(format!(
        "{}:hitl-gate:{}:expires:{expires_at_unix}",
        run_ref(input).0,
        gate_ref_token(gate_id)
    ))
}

fn encode_activity_outcome(
    input: &HostedAgentWorkflowInput,
    outcome: HostedAgentActivityOutcome,
) -> myelin_refs::ArtifactRef {
    match outcome {
        HostedAgentActivityOutcome::Completed(run_ref) => run_ref,
        HostedAgentActivityOutcome::ApprovalRequired {
            gate_id,
            expires_at_unix,
        } => gate_ref(input, &gate_id, expires_at_unix),
    }
}

fn decode_activity_outcome(
    input: &HostedAgentWorkflowInput,
    output: &[myelin_refs::ArtifactRef],
) -> Result<HostedAgentActivityOutcome, String> {
    let [artifact] = output else {
        return Err("hosted agent activity must return exactly one artifact".into());
    };
    if artifact == &run_ref(input) {
        return Ok(HostedAgentActivityOutcome::Completed(artifact.clone()));
    }
    let prefix = format!("{}:hitl-gate:", run_ref(input).0);
    let encoded_with_expiry = artifact
        .0
        .strip_prefix(&prefix)
        .ok_or_else(|| "hosted agent activity returned an unbound artifact".to_string())?;
    let (encoded, expires_at_unix) = encoded_with_expiry
        .split_once(":expires:")
        .ok_or_else(|| "hosted agent gate artifact has no expiry".to_string())?;
    let expires_at_unix = expires_at_unix
        .parse::<i64>()
        .ok()
        .filter(|deadline| *deadline > 0)
        .ok_or_else(|| "hosted agent gate artifact has an invalid expiry".to_string())?;
    let gate_id = gate_id_from_ref_token(encoded)
        .ok_or_else(|| "hosted agent activity returned an invalid gate ID".to_string())?;
    Ok(HostedAgentActivityOutcome::ApprovalRequired {
        gate_id,
        expires_at_unix,
    })
}

fn decode_decision(
    input: &HostedAgentWorkflowInput,
    gate_id: &str,
    payload: &[myelin_refs::ArtifactRef],
) -> Result<Option<HostedAgentStopReason>, String> {
    let [artifact] = payload else {
        return Err(
            "hosted agent approval signal must contain exactly one decision artifact".into(),
        );
    };
    if artifact
        == &hosted_agent_decision_ref(
            &input.tenant,
            &input.run_id,
            gate_id,
            HostedAgentDecision::Approved,
        )
    {
        return Ok(None);
    }
    if artifact
        == &hosted_agent_decision_ref(
            &input.tenant,
            &input.run_id,
            gate_id,
            HostedAgentDecision::Rejected,
        )
    {
        return Ok(Some(HostedAgentStopReason::Rejected));
    }
    if artifact
        == &hosted_agent_decision_ref(
            &input.tenant,
            &input.run_id,
            gate_id,
            HostedAgentDecision::Expired,
        )
    {
        return Ok(Some(HostedAgentStopReason::Expired));
    }
    Err("hosted agent approval signal is not bound to this gate and run".into())
}

fn decode_hosted_agent_workflow_input(
    resolved: &PgResolvedDriveInput,
) -> Result<HostedAgentWorkflowInput, String> {
    let input: HostedAgentWorkflowInput = serde_json::from_slice(&resolved.material)
        .map_err(|error| format!("decode governed hosted-agent input: {error}"))?;
    if input.tenant != resolved.claimed.tenant
        || input.region != resolved.claimed.region
        || input.run_id != resolved.claimed.run_id
    {
        return Err("resolved hosted-agent identity differs from its claimed workflow".into());
    }
    Ok(input)
}

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
    pub budget_minor_units: u64,
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
        .with_reservation_budget(myelin_storage::MicroUsd(self.budget_minor_units))
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
        if input.wf_type != AGENT_RUN_WORKFLOW
            || !matches!(
                input.wf_version,
                LEGACY_AGENT_RUN_WORKFLOW_VERSION | AGENT_RUN_WORKFLOW_VERSION
            )
        {
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
        let workflow_budget = input
            .budget
            .as_ref()
            .and_then(|budget| budget.get("minor_units"))
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                PgInputResolveError::Permanent(
                    "hosted agent workflow has no positive integer run budget".into(),
                )
            })?;
        if workflow_budget != started.budget_minor_units {
            return Err(PgInputResolveError::Permanent(
                "agent workflow budget does not match its governed trigger".into(),
            ));
        }
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
            budget_minor_units: started.budget_minor_units,
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

    fn workflow_input() -> HostedAgentWorkflowInput {
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
        HostedAgentWorkflowInput {
            tenant,
            region,
            run_id: "run-1".into(),
            agent_id: "a1".into(),
            agent,
            trigger_actor: founder.clone(),
            task: "Prepare the smallest safe fix.".into(),
            delegation_caveats: vec!["repo:core".into()],
            selected_tools: vec!["git.open_pr.v1".into()],
            budget_minor_units: 250_000,
            event,
        }
    }

    #[test]
    fn resolved_work_becomes_a_clear_agent_prompt_without_losing_delegation() {
        let resolved = workflow_input();
        let founder = resolved.trigger_actor.clone();
        let task = resolved.llm_task(42);
        assert_eq!(task.trigger_actor, founder);
        assert_eq!(task.delegation_caveats.0, ["repo:core"]);
        assert_eq!(task.estimate, myelin_storage::MicroUsd(250_000));
        assert!(task.prompt.contains("Prepare the smallest safe fix."));
        assert!(task.prompt.contains("ci.run.failed"));
        assert!(task.prompt.contains("refs/heads/main"));
    }

    #[test]
    fn an_approval_activity_returns_one_canonical_exact_gate_artifact() {
        let input = workflow_input();
        let outcome = HostedAgentActivityOutcome::ApprovalRequired {
            gate_id: "gate:merge-7".into(),
            expires_at_unix: 1_800_000_000,
        };
        let artifact = encode_activity_outcome(&input, outcome.clone());

        myelin_refs::parse_scoped(&artifact.0)
            .expect("an activity marker must cross the ArtifactRef boundary");
        assert_eq!(
            decode_activity_outcome(&input, &[artifact]).expect("the marker is replayable"),
            outcome
        );
    }

    #[test]
    fn a_human_decision_is_bound_to_one_run_and_one_gate() {
        let input = workflow_input();
        let approved = hosted_agent_decision_ref(
            &input.tenant,
            &input.run_id,
            "gate:merge-7",
            HostedAgentDecision::Approved,
        );
        let rejected = hosted_agent_decision_ref(
            &input.tenant,
            &input.run_id,
            "gate:merge-7",
            HostedAgentDecision::Rejected,
        );

        assert_eq!(
            decode_decision(&input, "gate:merge-7", &[approved]),
            Ok(None)
        );
        assert_eq!(
            decode_decision(&input, "gate:merge-7", &[rejected]),
            Ok(Some(HostedAgentStopReason::Rejected))
        );
        assert!(decode_decision(
            &input,
            "gate:another-effect",
            &[hosted_agent_decision_ref(
                &input.tenant,
                &input.run_id,
                "gate:merge-7",
                HostedAgentDecision::Approved,
            )],
        )
        .is_err());
    }
}
