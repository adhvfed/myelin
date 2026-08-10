use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use myelin_agent::{ToolCall, ToolDef, ToolName, ToolResult, ToolSchema};
use myelin_agent_model::{LunaClient, ModelClient, ModelError};
use myelin_agent_service::{
    catalogue_cursor, hosted_run_contract::gate_ref_token, PlatformToolCatalogue, ToolExecError,
    ToolExecutionContext, ToolExecutor,
};
use myelin_events::{OutboxStore, Timestamp, UlidMinter};
use myelin_flow::WfJournal;
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RunId};
use myelin_mcp::{
    git_merge_repo_from_effect_key, AuditPhase, GateAuditMinter, GovernanceAudit,
    GovernanceAuditRecord, OutboxGovernanceAudit,
};
use myelin_notif::agent_effect_approval_targets;
use myelin_notif::pg_inbox::PgInboxStore;
use myelin_storage::hitl_gate_durable::{DurableHitlGateBacking, GateState};
use myelin_storage::reserve_settle::{CostLedger, ReservationState, RunId as CostRunId};
use myelin_storage::{PgOutboxBacking, SubstrateProvider, TenantScope};
use myelin_tenancy::ArtifactRef;

use crate::{
    AgentHost, AgentHostError, HostedAgentActivityOutcome, HostedAgentRunExecutor,
    HostedAgentStopReason, HostedAgentWorkflowInput, RunSubstrateWiring, SkeletonError,
    ToolCatalogue, Tools,
};

struct HostedToolBrokerUnavailable;

impl ToolExecutor for HostedToolBrokerUnavailable {
    fn execute(
        &self,
        _context: &ToolExecutionContext<'_>,
        definition: &ToolDef,
        _call: &ToolCall,
    ) -> Result<ToolResult, ToolExecError> {
        Err(ToolExecError::Failed(format!(
            "hosted tool broker is not connected for `{}`; the call was not executed",
            definition.canonical_name()
        )))
    }
}

pub trait HostedModelFactory: Send + Sync {
    fn client(&self) -> Result<Box<dyn ModelClient + Send + Sync>, ModelError>;
}

/// Supplies live time for security decisions made outside deterministic workflow replay.
pub trait DeadlineClock: Send + Sync {
    fn now_unix_secs(&self) -> Result<i64, String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemDeadlineClock;

impl DeadlineClock for SystemDeadlineClock {
    fn now_unix_secs(&self) -> Result<i64, String> {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "system clock is before the Unix epoch".to_string())?
            .as_secs();
        i64::try_from(seconds).map_err(|_| "system clock exceeds the supported range".to_string())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LunaModelFactory;

impl HostedModelFactory for LunaModelFactory {
    fn client(&self) -> Result<Box<dyn ModelClient + Send + Sync>, ModelError> {
        LunaClient::from_env().map(|client| Box::new(client) as Box<dyn ModelClient + Send + Sync>)
    }
}

pub struct AgentHostActivityExecutor {
    host: Arc<AgentHost>,
    provider: SubstrateProvider,
    outbox: OutboxStore,
    gates: DurableHitlGateBacking,
    inbox: PgInboxStore,
    runtime: tokio::runtime::Handle,
    models: Arc<dyn HostedModelFactory>,
    tools: Arc<dyn ToolExecutor>,
    deadline_clock: Arc<dyn DeadlineClock>,
}

impl AgentHostActivityExecutor {
    pub fn new(
        host: Arc<AgentHost>,
        provider: SubstrateProvider,
        runtime: tokio::runtime::Handle,
        models: Arc<dyn HostedModelFactory>,
    ) -> Self {
        let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
            provider.db_pool().clone(),
            runtime.clone(),
        )));
        Self {
            host,
            gates: DurableHitlGateBacking::new(provider.clone()),
            inbox: PgInboxStore::new(provider.db_pool().clone()),
            provider,
            outbox,
            runtime,
            models,
            tools: Arc::new(HostedToolBrokerUnavailable),
            deadline_clock: Arc::new(SystemDeadlineClock),
        }
    }

    pub fn with_tool_executor(mut self, tools: Arc<dyn ToolExecutor>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_deadline_clock(mut self, clock: Arc<dyn DeadlineClock>) -> Self {
        self.deadline_clock = clock;
        self
    }

    fn drive<F: std::future::Future>(&self, future: F) -> F::Output {
        match tokio::runtime::Handle::try_current() {
            Ok(_) => tokio::task::block_in_place(|| self.runtime.block_on(future)),
            Err(_) => self.runtime.block_on(future),
        }
    }

    fn selected_tools(
        selected_tools: &[String],
    ) -> Result<(ToolCatalogue, Vec<ToolSchema>), String> {
        let platform = PlatformToolCatalogue::platform().map_err(|error| error.to_string())?;
        let mut definitions = Vec::with_capacity(selected_tools.len());
        let mut schemas = Vec::with_capacity(selected_tools.len());
        for cursor in selected_tools {
            let definition = platform
                .definitions()
                .iter()
                .find(|definition| catalogue_cursor(definition) == *cursor)
                .or_else(|| platform.resolve(cursor))
                .filter(|definition| definition.exposed_over_mcp)
                .ok_or_else(|| {
                    format!("selected hosted-agent tool `{cursor}` is no longer available")
                })?;
            let canonical_name = definition.canonical_name();
            let description = definition
                .mcp_projection()
                .ok()
                .and_then(|projection| {
                    projection
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("Myelin tool `{canonical_name}`"));
            schemas.push(ToolSchema {
                name: ToolName(canonical_name),
                description,
                input_schema: definition.input_schema.clone(),
            });
            definitions.push(definition.clone());
        }
        Ok((ToolCatalogue::new(definitions), schemas))
    }

    fn run_ref(input: &HostedAgentWorkflowInput) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{}/agent/run/{}",
            input.tenant.0, input.run_id
        ))
    }

    fn stopped_ref(
        input: &HostedAgentWorkflowInput,
        gate_id: &str,
        reason: HostedAgentStopReason,
    ) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{}/agent/run/{}:stopped:{}:gate:{}",
            input.tenant.0,
            input.run_id,
            reason.as_str(),
            gate_ref_token(gate_id)
        ))
    }

    fn expire_gate(
        &self,
        input: &HostedAgentWorkflowInput,
        gate_id: &str,
        now_secs: i64,
    ) -> Result<(), String> {
        let scope = TenantScope::from_verified_token(&input.agent, input.region.clone());
        let outcome = self
            .drive(self.gates.expire_if_due(&scope, gate_id, now_secs))
            .map_err(|error| format!("expire hosted approval gate: {error}"))?
            .map_err(|error| format!("expire hosted approval gate: {error}"))?;
        let gate = outcome.record;
        if gate.run_id != input.run_id || gate.requested_by != input.agent.principal_id.0 {
            return Err("expired approval gate is not bound to this hosted run".into());
        }
        let tool = if git_merge_repo_from_effect_key(&gate.effect_id).is_some() {
            "git.merge"
        } else {
            return Err("expired approval gate has no governance audit taxonomy".into());
        };
        let audited_at_unix = gate.decided_at_unix.unwrap_or(gate.expires_at_unix);
        let audited_at = chrono::DateTime::from_timestamp(audited_at_unix, 0)
            .ok_or_else(|| "hosted approval expiry timestamp is invalid".to_string())?
            .to_rfc3339();
        let actor = Principal::new(
            input.tenant.clone(),
            input.region.clone(),
            PrincipalId("service:mcp-hitl-expiry".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        OutboxGovernanceAudit::new(
            self.outbox.clone(),
            Arc::new(GateAuditMinter::new(
                input.tenant.as_str(),
                &gate.gate_id,
                AuditPhase::Expired,
            )),
        )
        .record(GovernanceAuditRecord {
            scope: &scope,
            actor: &actor,
            run_id: &RunId(gate.run_id.clone()),
            gate_id: Some(&gate.gate_id),
            tool,
            jti: "system:hitl-expiry",
            phase: AuditPhase::Expired,
            outcome: None,
            now: &Timestamp(audited_at),
        })
        .map_err(|error| format!("audit hosted approval expiry: {error}"))?;
        for target in agent_effect_approval_targets(&input.tenant, &input.region, &gate) {
            self.drive(
                self.inbox
                    .complete_if_present(&target.scope, &target.item_id),
            )
            .map_err(|error| format!("complete expired approval inbox item: {error:?}"))?;
        }
        Ok(())
    }
}

impl HostedAgentRunExecutor for AgentHostActivityExecutor {
    fn execute(
        &self,
        input: &HostedAgentWorkflowInput,
        activity_key: &str,
        attempt: u32,
        _workflow_time_secs: i64,
    ) -> Result<HostedAgentActivityOutcome, String> {
        if !activity_key.starts_with(&format!("{}/", input.run_id)) {
            return Err("hosted activity key belongs to a different run".into());
        }
        let mut ledger = CostLedger::with_pg(self.provider.clone());
        if let Some(existing) =
            ledger.reservation_of(&input.tenant, &CostRunId::new(input.run_id.clone()))
        {
            if existing.state == ReservationState::Settled
                && existing.reserved.0 == input.budget_minor_units
            {
                return Ok(HostedAgentActivityOutcome::Completed(Self::run_ref(input)));
            }
            if existing.state == ReservationState::Settled {
                return Err(
                    "settled hosted run has a different governed budget; replay refused".into(),
                );
            }
        }
        let (catalogue, advertised) = Self::selected_tools(&input.selected_tools)?;
        let model = self.models.client().map_err(|error| error.to_string())?;
        let deadline_now_secs = self.deadline_clock.now_unix_secs()?;
        let mut wiring = RunSubstrateWiring {
            ledger: &mut ledger,
            outbox: &self.outbox,
            id_minter: Arc::new(UlidMinter::new()),
            journal: WfJournal::new(),
        };
        match self.host.run(
            &input
                .llm_task(deadline_now_secs)
                .with_credential_attempt(format!("{activity_key}/{attempt}")),
            &mut wiring,
            model,
            Tools {
                catalogue: &catalogue,
                executor: self.tools.as_ref(),
                advertised: &advertised,
            },
        ) {
            Ok(_) => Ok(HostedAgentActivityOutcome::Completed(Self::run_ref(input))),
            Err(AgentHostError::Run(SkeletonError::ApprovalRequired { gate_id })) => {
                let scope = TenantScope::from_verified_token(&input.agent, input.region.clone());
                let gate = self
                    .drive(self.gates.fetch(&scope, &gate_id))
                    .map_err(|error| format!("load hosted approval gate: {error}"))?
                    .ok_or_else(|| "hosted approval gate disappeared".to_string())?;
                if gate.run_id != input.run_id
                    || gate.requested_by != input.agent.principal_id.0
                    || gate.state != GateState::Waiting
                {
                    return Err("hosted approval gate is not bound to this waiting run".into());
                }
                Ok(HostedAgentActivityOutcome::ApprovalRequired {
                    gate_id,
                    expires_at_unix: gate.expires_at_unix,
                })
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn stop(
        &self,
        input: &HostedAgentWorkflowInput,
        activity_key: &str,
        _workflow_time_secs: i64,
        gate_id: &str,
        reason: HostedAgentStopReason,
    ) -> Result<ArtifactRef, String> {
        if !activity_key.starts_with(&format!("{}/", input.run_id)) {
            return Err("hosted stop activity key belongs to a different run".into());
        }
        if reason == HostedAgentStopReason::Expired {
            self.expire_gate(input, gate_id, self.deadline_clock.now_unix_secs()?)?;
        }
        let mut ledger = CostLedger::with_pg(self.provider.clone());
        ledger
            .settle(&input.tenant, &CostRunId::new(input.run_id.clone()), &[])
            .map_err(|error| format!("settle stopped hosted run: {error}"))?;
        Ok(Self::stopped_ref(input, gate_id, reason))
    }
}

#[cfg(test)]
mod tests {
    use myelin_agent::{ToolName, ToolSurface};

    use super::*;

    #[test]
    fn governed_tool_cursors_become_canonical_model_tools() {
        let (catalogue, schemas) = AgentHostActivityExecutor::selected_tools(&[
            "ci.read_run.v1".into(),
            "issues.create.v1".into(),
        ])
        .expect("the tools selected at agent creation still exist");

        assert_eq!(
            schemas
                .iter()
                .map(|schema| schema.name.0.as_str())
                .collect::<Vec<_>>(),
            ["ci.read_run", "issues.create"]
        );
        assert!(catalogue.resolve(&ToolName("ci.read_run".into())).is_some());
        assert!(catalogue
            .resolve(&ToolName("issues.create".into()))
            .is_some());
    }

    #[test]
    fn stale_selected_tools_fail_closed_before_a_model_run() {
        let error = AgentHostActivityExecutor::selected_tools(&["ci.retired.v1".into()])
            .expect_err("a removed tool cannot silently become a different tool");
        assert!(error.contains("no longer available"));
    }
}
