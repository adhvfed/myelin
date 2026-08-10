use std::sync::Arc;

pub use crate::hosted_run_contract::{
    agent_run_definition_hash, AGENT_RUN_WORKFLOW, AGENT_RUN_WORKFLOW_VERSION,
};
use myelin_events::{EventEnvelope, HandlerTx, UlidMinter};
use myelin_flow::{ExecutorError, PgFlowExecutor, RunBudget, RunId, StartSpec};
use myelin_identity_service::HOSTED_LUNA_RUNTIME;
use myelin_storage::{
    with_tenant_tx_error, AgentTriggerStartRequest, ClaimedAgentTriggerFiring,
    DurableAgentTriggerBacking, PgError, StartAgentTriggerFiringOutcome, SubstrateProvider,
    MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS, MIN_AGENT_TRIGGER_BUDGET_MINOR_UNITS,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::types::Uuid;

pub const HOSTED_AGENT_RUNTIME: &str = HOSTED_LUNA_RUNTIME;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerRunStart {
    Started,
    AlreadyStarted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TriggerRunReceipt {
    pub run_id: String,
    pub outcome: TriggerRunStart,
}

#[derive(Debug)]
pub enum TriggerHandoffError {
    InvalidClaim(String),
    ClaimUnavailable,
    Workflow(ExecutorError),
    Storage(PgError),
}

impl core::fmt::Display for TriggerHandoffError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidClaim(reason) => write!(formatter, "invalid trigger claim: {reason}"),
            Self::ClaimUnavailable => formatter.write_str("trigger claim is no longer live"),
            Self::Workflow(error) => write!(formatter, "agent workflow start failed: {error}"),
            Self::Storage(error) => write!(formatter, "trigger handoff storage failed: {error}"),
        }
    }
}

impl std::error::Error for TriggerHandoffError {}

impl From<PgError> for TriggerHandoffError {
    fn from(error: PgError) -> Self {
        Self::Storage(error)
    }
}

#[derive(Clone)]
pub struct TriggerRunHandoff {
    provider: SubstrateProvider,
    triggers: DurableAgentTriggerBacking,
    runtime: tokio::runtime::Handle,
}

impl TriggerRunHandoff {
    pub fn new(provider: SubstrateProvider, runtime: tokio::runtime::Handle) -> Self {
        Self {
            triggers: DurableAgentTriggerBacking::new(provider.clone()),
            provider,
            runtime,
        }
    }

    pub fn register_workflow(&self, tenant: &TenantId) -> Result<(), TriggerHandoffError> {
        self.executor(tenant)
            .register_definition(
                AGENT_RUN_WORKFLOW,
                AGENT_RUN_WORKFLOW_VERSION,
                &agent_run_definition_hash(),
            )
            .map_err(TriggerHandoffError::Workflow)
    }

    pub async fn start(
        &self,
        tenant: &TenantId,
        claim: &ClaimedAgentTriggerFiring,
    ) -> Result<TriggerRunReceipt, TriggerHandoffError> {
        let event = validate_claim_envelope(
            tenant,
            &Region(self.provider.config().region.clone()),
            claim,
        )?;
        let run_uuid = run_id_for(tenant, claim)?;
        let start_request = AgentTriggerStartRequest::from_claim(claim, run_uuid)
            .map_err(|reason| TriggerHandoffError::InvalidClaim(reason.into()))?;
        if !(MIN_AGENT_TRIGGER_BUDGET_MINOR_UNITS..=MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS)
            .contains(&claim.budget_minor_units)
        {
            return Err(TriggerHandoffError::InvalidClaim(
                "budget_minor_units is outside its durable bound".into(),
            ));
        }
        let budget = i64::try_from(claim.budget_minor_units)
            .expect("the durable trigger budget bound fits i64");
        let start_spec = StartSpec {
            wf_type: AGENT_RUN_WORKFLOW.into(),
            input: vec![event.subject],
            budget: Some(RunBudget {
                minor_units: budget,
            }),
            idem_key: firing_idempotency_key(claim),
        };
        let executor = self.executor(tenant);
        let triggers = self.triggers.clone();
        let tenant_id = tenant.0.clone();
        let region = self.provider.config().region.clone();

        with_tenant_tx_error(self.provider.db_pool(), &tenant.0, &region, move |conn| {
            Box::pin(async move {
                let outcome = triggers
                    .start_claimed_firing_on_conn(conn, &tenant_id, &start_request)
                    .await?;
                match outcome {
                    StartAgentTriggerFiringOutcome::ClaimUnavailable => {
                        Err(TriggerHandoffError::ClaimUnavailable)
                    }
                    StartAgentTriggerFiringOutcome::AlreadyStarted => Ok(TriggerRunReceipt {
                        run_id: run_uuid.to_string(),
                        outcome: TriggerRunStart::AlreadyStarted,
                    }),
                    StartAgentTriggerFiringOutcome::Started => {
                        let started = {
                            let mut tx = HandlerTx::with_connection(conn);
                            executor.start_with_id_on_conn(
                                &mut tx,
                                start_spec,
                                Some(RunId(run_uuid.to_string())),
                            )
                        }
                        .map_err(TriggerHandoffError::Workflow)?;
                        if started.0 != run_uuid.to_string() {
                            return Err(TriggerHandoffError::InvalidClaim(
                                "firing idempotency key resolved to a different workflow run"
                                    .into(),
                            ));
                        }
                        Ok(TriggerRunReceipt {
                            run_id: started.0,
                            outcome: TriggerRunStart::Started,
                        })
                    }
                }
            })
        })
        .await
    }

    fn executor(&self, tenant: &TenantId) -> PgFlowExecutor {
        PgFlowExecutor::new(
            self.provider.db_pool().clone(),
            self.runtime.clone(),
            Arc::new(UlidMinter::new()),
            tenant.clone(),
            Region(self.provider.config().region.clone()),
        )
    }
}

fn firing_idempotency_key(claim: &ClaimedAgentTriggerFiring) -> String {
    format!("agent-trigger:{}:{}", claim.binding_id, claim.event_id)
}

fn run_id_for(
    tenant: &TenantId,
    claim: &ClaimedAgentTriggerFiring,
) -> Result<Uuid, TriggerHandoffError> {
    Uuid::parse_str(&claim.binding_id)
        .map_err(|_| TriggerHandoffError::InvalidClaim("binding_id is not a UUID".into()))?;
    let digest = blake3::hash(
        format!(
            "myelin:agent-trigger-run:v1\0{}\0{}\0{}",
            tenant.0, claim.binding_id, claim.event_id
        )
        .as_bytes(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(Uuid::from_bytes(bytes))
}

fn validate_claim_envelope(
    tenant: &TenantId,
    region: &Region,
    claim: &ClaimedAgentTriggerFiring,
) -> Result<EventEnvelope, TriggerHandoffError> {
    if claim.runtime_ref != HOSTED_AGENT_RUNTIME {
        return Err(TriggerHandoffError::InvalidClaim(format!(
            "runtime `{}` cannot be handed to the hosted worker",
            claim.runtime_ref
        )));
    }
    let event: EventEnvelope = serde_json::from_value(claim.event_envelope.clone())
        .map_err(|error| TriggerHandoffError::InvalidClaim(format!("invalid envelope: {error}")))?;
    if event.tenant != *tenant
        || event.region != *region
        || event.event_id.0 != claim.event_id
        || event.type_.0 != claim.event_type
    {
        return Err(TriggerHandoffError::InvalidClaim(
            "envelope identity does not match its firing record".into(),
        ));
    }
    let subject = myelin_refs::parse_scoped(&event.subject.0)
        .map_err(|error| TriggerHandoffError::InvalidClaim(error.to_string()))?;
    if subject.tenant != *tenant {
        return Err(TriggerHandoffError::InvalidClaim(
            "event subject belongs to another tenant".into(),
        ));
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::ArtifactRef;

    fn claim() -> ClaimedAgentTriggerFiring {
        let tenant = TenantId("acme".into());
        let event = EventEnvelope {
            event_id: EventId("event-1".into()),
            type_: EventType("ci.run.failed".into()),
            schema_ver: 1,
            tenant: tenant.clone(),
            region: Region("eu-north".into()),
            actor: Actor(Principal::stub(
                PrincipalId("ci-controlplane".into()),
                PrincipalKind::Service,
                tenant,
            )),
            subject: ArtifactRef("myelin://acme/ci/run/run-1".into()),
            aggregate: AggregateKey("ci-run:run-1".into()),
            causation_id: None,
            correlation_id: CorrelationId("root-1".into()),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-08-10T00:00:00Z".into()),
            recorded_at: Timestamp("2026-08-10T00:00:01Z".into()),
            payload: serde_json::json!({}),
        };
        ClaimedAgentTriggerFiring {
            binding_id: "10000000-0000-4000-8000-000000000001".into(),
            event_id: "event-1".into(),
            event_type: "ci.run.failed".into(),
            event_envelope: serde_json::to_value(event).unwrap(),
            owner_principal_id: "founder".into(),
            run_as_agent_id: "20000000-0000-4000-8000-000000000002".into(),
            runtime_ref: HOSTED_AGENT_RUNTIME.into(),
            task: "Fix the failure.".into(),
            delegation_caveats: vec!["repo:core".into()],
            budget_minor_units: 250_000,
            claim_owner: "host-1".into(),
            claim_until: "2026-08-10T00:00:30Z".into(),
            claim_attempts: 1,
        }
    }

    #[test]
    fn one_firing_always_names_one_workflow() {
        let first = run_id_for(&TenantId("acme".into()), &claim()).unwrap();
        let second = run_id_for(&TenantId("acme".into()), &claim()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.get_version_num(), 8, "the ID is an RFC custom UUID");

        let mut another = claim();
        another.event_id = "event-2".into();
        assert_ne!(
            first,
            run_id_for(&TenantId("acme".into()), &another).unwrap()
        );
    }

    #[test]
    fn the_immutable_event_is_the_workflow_input_authority() {
        let claim = claim();
        let event =
            validate_claim_envelope(&TenantId("acme".into()), &Region("eu-north".into()), &claim)
                .unwrap();
        assert_eq!(event.subject.0, "myelin://acme/ci/run/run-1");

        let mut crossed = claim;
        crossed.event_envelope["tenant"] = serde_json::json!("other");
        assert!(validate_claim_envelope(
            &TenantId("acme".into()),
            &Region("eu-north".into()),
            &crossed,
        )
        .is_err());
    }
}
