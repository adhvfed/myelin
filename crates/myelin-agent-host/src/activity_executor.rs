use std::collections::BTreeSet;
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
    approval_contract_from_effect_key, AuditPhase, GateAuditMinter, GovernanceAudit,
    GovernanceAuditRecord, GovernanceAuditTarget, OutboxGovernanceAudit,
};
use myelin_notif::agent_effect_approval_targets;
use myelin_notif::pg_inbox::PgInboxStore;
use myelin_storage::hitl_gate_durable::{
    DurableHitlGateBacking, GateDecideError, GateRecord, GateState,
};
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

#[derive(Debug)]
struct HostedActivityAttemptFailure {
    code: &'static str,
    detail: String,
}

impl HostedActivityAttemptFailure {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    fn from_model(error: ModelError) -> Self {
        let error = error.runtime_step_error();
        Self::new(error.code(), error.to_string())
    }

    fn from_host(error: AgentHostError) -> Self {
        Self::new(error.code(), error.to_string())
    }
}

fn report_activity_failure(
    input: &HostedAgentWorkflowInput,
    operation: &str,
    attempt: Option<u32>,
    failure: &HostedActivityAttemptFailure,
) {
    let attempt = attempt
        .map(|attempt| format!(" attempt={attempt}"))
        .unwrap_or_default();
    eprintln!(
        "hosted-agent-worker: {operation} failed for tenant {} run {}: code={}{}",
        input.tenant.0, input.run_id, failure.code, attempt,
    );
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
        delegation_caveats: &[String],
    ) -> Result<(ToolCatalogue, Vec<ToolSchema>), String> {
        let platform = PlatformToolCatalogue::platform().map_err(|error| error.to_string())?;
        let capability_ceiling = delegation_capability_ceiling(delegation_caveats);
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
            if capability_ceiling.as_ref().is_some_and(|ceiling| {
                definition
                    .required_caps
                    .iter()
                    .any(|capability| !ceiling.contains(capability.as_str()))
            }) {
                continue;
            }
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

    fn ensure_gate_belongs_to_run(
        input: &HostedAgentWorkflowInput,
        gate: &GateRecord,
    ) -> Result<(), String> {
        if gate.run_id != input.run_id || gate.requested_by != input.agent.principal_id.0 {
            return Err("hosted approval gate is not bound to this run".into());
        }
        Ok(())
    }

    fn load_gate(&self, scope: &TenantScope, gate_id: &str) -> Result<GateRecord, String> {
        self.drive(self.gates.fetch(scope, gate_id))
            .map_err(|error| format!("load hosted approval gate: {error}"))?
            .ok_or_else(|| "hosted approval gate disappeared".to_string())
    }

    fn audit_terminal_gate(
        &self,
        input: &HostedAgentWorkflowInput,
        scope: &TenantScope,
        gate: &GateRecord,
        reason: HostedAgentStopReason,
    ) -> Result<(), String> {
        let contract = approval_contract_from_effect_key(&gate.effect_id).ok_or_else(|| {
            "terminal approval gate has no registered approval contract".to_string()
        })?;
        let audited_at_unix = gate.decided_at_unix.unwrap_or(gate.expires_at_unix);
        let audited_at = chrono::DateTime::from_timestamp(audited_at_unix, 0)
            .ok_or_else(|| "hosted approval decision timestamp is invalid".to_string())?
            .to_rfc3339();
        let (phase, actor, jti) = match reason {
            HostedAgentStopReason::Rejected => {
                let decided_by = gate
                    .decided_by
                    .clone()
                    .ok_or_else(|| "rejected hosted approval has no deciding human".to_string())?;
                let actor = if input.trigger_actor.principal_id.0 == decided_by
                    && input.trigger_actor.kind == PrincipalKind::Human
                {
                    input.trigger_actor.clone()
                } else {
                    Principal::new(
                        input.tenant.clone(),
                        input.region.clone(),
                        PrincipalId(decided_by),
                        PrincipalKind::Human,
                        DataRole::Controller,
                        PrincipalStatus::Active,
                    )
                };
                (
                    AuditPhase::Rejected,
                    actor,
                    format!("human-decision:{}", gate.gate_id),
                )
            }
            HostedAgentStopReason::Expired => (
                AuditPhase::Expired,
                Principal::new(
                    input.tenant.clone(),
                    input.region.clone(),
                    PrincipalId("service:mcp-hitl-expiry".into()),
                    PrincipalKind::Service,
                    DataRole::Controller,
                    PrincipalStatus::Active,
                ),
                "system:hitl-expiry".into(),
            ),
        };
        OutboxGovernanceAudit::new(
            self.outbox.clone(),
            Arc::new(GateAuditMinter::new(
                input.tenant.as_str(),
                &gate.gate_id,
                phase,
            )),
        )
        .record(GovernanceAuditRecord {
            scope,
            actor: &actor,
            run_id: &RunId(gate.run_id.clone()),
            target: GovernanceAuditTarget::Gate(&gate.gate_id),
            tool: contract.tool(),
            jti: &jti,
            phase,
            outcome: None,
            now: &Timestamp(audited_at),
        })
        .map_err(|error| format!("audit terminal hosted approval: {error}"))
    }

    fn complete_gate_notifications(
        &self,
        input: &HostedAgentWorkflowInput,
        gate: &GateRecord,
    ) -> Result<(), String> {
        for target in agent_effect_approval_targets(&input.tenant, &input.region, &gate) {
            self.drive(
                self.inbox
                    .complete_if_present(&target.scope, &target.item_id),
            )
            .map_err(|error| format!("complete terminal approval inbox item: {error:?}"))?;
        }
        Ok(())
    }

    fn reconcile_stopped_gate(
        &self,
        input: &HostedAgentWorkflowInput,
        gate_id: &str,
        requested_reason: HostedAgentStopReason,
    ) -> Result<HostedAgentStopReason, String> {
        let scope = TenantScope::from_verified_token(&input.agent, input.region.clone());
        let (gate, reason) = match requested_reason {
            HostedAgentStopReason::Rejected => (
                self.load_gate(&scope, gate_id)?,
                HostedAgentStopReason::Rejected,
            ),
            HostedAgentStopReason::Expired => {
                let now_secs = self.deadline_clock.now_unix_secs()?;
                match self
                    .drive(self.gates.expire_if_due(&scope, gate_id, now_secs))
                    .map_err(|error| format!("expire hosted approval gate: {error}"))?
                {
                    Ok(outcome) => (outcome.record, HostedAgentStopReason::Expired),
                    Err(GateDecideError::AlreadyDecided(GateState::Rejected)) => (
                        self.load_gate(&scope, gate_id)?,
                        HostedAgentStopReason::Rejected,
                    ),
                    Err(error) => return Err(format!("expire hosted approval gate: {error}")),
                }
            }
        };
        Self::ensure_gate_belongs_to_run(input, &gate)?;
        let expected_state = match reason {
            HostedAgentStopReason::Rejected => GateState::Rejected,
            HostedAgentStopReason::Expired => GateState::Expired,
        };
        if gate.state != expected_state {
            return Err(format!(
                "hosted approval is {} but its stop reason is {}",
                gate.state.as_str(),
                reason.as_str(),
            ));
        }
        self.audit_terminal_gate(input, &scope, &gate, reason)?;
        self.complete_gate_notifications(input, &gate)?;
        Ok(reason)
    }

    fn execute_attempt(
        &self,
        input: &HostedAgentWorkflowInput,
        activity_key: &str,
        attempt: u32,
    ) -> Result<HostedAgentActivityOutcome, HostedActivityAttemptFailure> {
        if !activity_key.starts_with(&format!("{}/", input.run_id)) {
            return Err(HostedActivityAttemptFailure::new(
                "activity_contract_invalid",
                "hosted activity key belongs to a different run",
            ));
        }
        let mut ledger = CostLedger::with_pg(self.provider.clone());
        if let Some(existing) = ledger
            .reservation_of(&input.tenant, &CostRunId::new(input.run_id.clone()))
            .map_err(|error| {
                HostedActivityAttemptFailure::new(
                    "cost_storage_unavailable",
                    format!("load hosted run cost reservation: {error}"),
                )
            })?
        {
            if existing.state == ReservationState::Settled
                && existing.reserved.0 == input.budget_minor_units
            {
                return Ok(HostedAgentActivityOutcome::Completed(Self::run_ref(input)));
            }
            if existing.state == ReservationState::Settled {
                return Err(HostedActivityAttemptFailure::new(
                    "governed_budget_mismatch",
                    "settled hosted run has a different governed budget; replay refused",
                ));
            }
        }
        let (catalogue, advertised) =
            Self::selected_tools(&input.selected_tools, &input.delegation_caveats).map_err(
                |error| HostedActivityAttemptFailure::new("tool_contract_unavailable", error),
            )?;
        let model = self
            .models
            .client()
            .map_err(HostedActivityAttemptFailure::from_model)?;
        let deadline_now_secs = self.deadline_clock.now_unix_secs().map_err(|error| {
            HostedActivityAttemptFailure::new("deadline_clock_unavailable", error)
        })?;
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
                let gate = self.load_gate(&scope, &gate_id).map_err(|error| {
                    HostedActivityAttemptFailure::new("approval_state_unavailable", error)
                })?;
                Self::ensure_gate_belongs_to_run(input, &gate).map_err(|error| {
                    HostedActivityAttemptFailure::new("approval_state_invalid", error)
                })?;
                if gate.state != GateState::Waiting {
                    return Err(HostedActivityAttemptFailure::new(
                        "approval_state_invalid",
                        "hosted approval gate is not bound to this waiting run",
                    ));
                }
                Ok(HostedAgentActivityOutcome::ApprovalRequired {
                    gate_id,
                    expires_at_unix: gate.expires_at_unix,
                })
            }
            Err(error) => Err(HostedActivityAttemptFailure::from_host(error)),
        }
    }

    fn stop_attempt(
        &self,
        input: &HostedAgentWorkflowInput,
        activity_key: &str,
        gate_id: &str,
        reason: HostedAgentStopReason,
    ) -> Result<ArtifactRef, HostedActivityAttemptFailure> {
        if !activity_key.starts_with(&format!("{}/", input.run_id)) {
            return Err(HostedActivityAttemptFailure::new(
                "activity_contract_invalid",
                "hosted stop activity key belongs to a different run",
            ));
        }
        let reason = self
            .reconcile_stopped_gate(input, gate_id, reason)
            .map_err(|error| {
                HostedActivityAttemptFailure::new("approval_reconciliation_failed", error)
            })?;
        let mut ledger = CostLedger::with_pg(self.provider.clone());
        ledger
            .settle(&input.tenant, &CostRunId::new(input.run_id.clone()), &[])
            .map_err(|error| {
                HostedActivityAttemptFailure::new(
                    "cost_storage_unavailable",
                    format!("settle stopped hosted run: {error}"),
                )
            })?;
        Ok(Self::stopped_ref(input, gate_id, reason))
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
        match self.execute_attempt(input, activity_key, attempt) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                report_activity_failure(input, "activity attempt", Some(attempt), &error);
                Err(error.detail)
            }
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
        match self.stop_attempt(input, activity_key, gate_id, reason) {
            Ok(run_ref) => Ok(run_ref),
            Err(error) => {
                report_activity_failure(input, "stop activity", None, &error);
                Err(error.detail)
            }
        }
    }
}

fn delegation_capability_ceiling(caveats: &[String]) -> Option<BTreeSet<&str>> {
    let capabilities = caveats
        .iter()
        .map(String::as_str)
        .filter(|caveat| {
            !caveat.starts_with("run:")
                && !caveat.starts_with("tenant:")
                && !caveat.starts_with("delegated:")
                && caveat
                    .strip_prefix("repo:")
                    .is_none_or(|repository| repository.contains('#'))
        })
        .collect::<BTreeSet<_>>();
    (!capabilities.is_empty()).then_some(capabilities)
}

#[cfg(test)]
mod tests {
    use myelin_agent::{ToolName, ToolSurface};

    use super::*;

    #[test]
    fn governed_tool_cursors_become_canonical_model_tools() {
        let (catalogue, schemas) = AgentHostActivityExecutor::selected_tools(
            &["ci.read_run.v1".into(), "issues.create.v1".into()],
            &[],
        )
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
        let error = AgentHostActivityExecutor::selected_tools(&["ci.retired.v1".into()], &[])
            .expect_err("a removed tool cannot silently become a different tool");
        assert!(error.contains("no longer available"));
    }

    #[test]
    fn capability_caveats_hide_tools_outside_the_automation_scope() {
        let (catalogue, schemas) = AgentHostActivityExecutor::selected_tools(
            &["ci.read_run.v1".into(), "issues.create.v1".into()],
            &["issue.create".into(), "repo:core".into()],
        )
        .expect("the selected tools still exist");

        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name.0, "issues.create");
        assert!(catalogue
            .resolve(&ToolName("issues.create".into()))
            .is_some());
        assert!(catalogue.resolve(&ToolName("ci.read_run".into())).is_none());
    }

    #[test]
    fn model_factory_failures_become_safe_operator_diagnostics() {
        let failure = HostedActivityAttemptFailure::from_model(ModelError::Http {
            status: 503,
            body: "provider response containing a secret".into(),
        });

        assert_eq!(failure.code, "runtime_rejected");
        assert_eq!(
            failure.detail,
            "the agent runtime rejected the step (HTTP 503)",
        );
        assert!(!failure.detail.contains("provider response"));
        assert!(!failure.detail.contains("secret"));
    }
}
