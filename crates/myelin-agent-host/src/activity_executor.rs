use std::sync::Arc;

use myelin_agent::{ToolCall, ToolDef, ToolName, ToolResult, ToolSchema};
use myelin_agent_model::{LunaClient, ModelClient, ModelError};
use myelin_agent_service::{catalogue_cursor, PlatformToolCatalogue, ToolExecError, ToolExecutor};
use myelin_events::{OutboxStore, UlidMinter};
use myelin_flow::WfJournal;
use myelin_storage::reserve_settle::{CostLedger, ReservationState, RunId as CostRunId};
use myelin_storage::{PgOutboxBacking, SubstrateProvider};
use myelin_tenancy::ArtifactRef;

use crate::{
    AgentHost, HostedAgentRunExecutor, HostedAgentWorkflowInput, RunSubstrateWiring, ToolCatalogue,
    Tools,
};

struct HostedToolBrokerUnavailable;

impl ToolExecutor for HostedToolBrokerUnavailable {
    fn execute(&self, definition: &ToolDef, _call: &ToolCall) -> Result<ToolResult, ToolExecError> {
        Err(ToolExecError::Failed(format!(
            "hosted tool broker is not connected for `{}`; the call was not executed",
            definition.canonical_name()
        )))
    }
}

pub trait HostedModelFactory: Send + Sync {
    fn client(&self) -> Result<Box<dyn ModelClient + Send + Sync>, ModelError>;
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
    models: Arc<dyn HostedModelFactory>,
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
            runtime,
        )));
        Self {
            host,
            provider,
            outbox,
            models,
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
}

impl HostedAgentRunExecutor for AgentHostActivityExecutor {
    fn execute(
        &self,
        input: &HostedAgentWorkflowInput,
        activity_key: &str,
        now_secs: i64,
    ) -> Result<ArtifactRef, String> {
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
                return Ok(Self::run_ref(input));
            }
            if existing.state == ReservationState::Settled {
                return Err(
                    "settled hosted run has a different governed budget; replay refused".into(),
                );
            }
        }
        let (catalogue, advertised) = Self::selected_tools(&input.selected_tools)?;
        let model = self.models.client().map_err(|error| error.to_string())?;
        let mut wiring = RunSubstrateWiring {
            ledger: &mut ledger,
            outbox: &self.outbox,
            id_minter: Arc::new(UlidMinter::new()),
            journal: WfJournal::new(),
        };
        self.host
            .run(
                &input.llm_task(now_secs),
                &mut wiring,
                model,
                Tools {
                    catalogue: &catalogue,
                    executor: &HostedToolBrokerUnavailable,
                    advertised: &advertised,
                },
            )
            .map_err(|error| error.to_string())?;
        Ok(Self::run_ref(input))
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
