use std::sync::Arc;
use std::time::Duration;

use myelin_agent_host::{
    register_hosted_agent_workflow, AgentHost, AgentHostActivityExecutor, EdgeMcpToolExecutor,
    HostedAgentInputResolver, HostedAgentRunExecutor, HostedModelFactory, LunaModelFactory,
};
use myelin_agent_model::{
    ModelClient, ModelError, ModelReply, ModelRequest, ModelResponse, ModelTurn, ToolCallRequest,
    Usage,
};
use myelin_agent_service::trigger_handoff::TriggerRunHandoff;
use myelin_config::Mode;
use myelin_events::{Actor, UlidMinter};
use myelin_flow::{PgFlowWorker, PgWorkerScope, PARTITION_COUNT};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    all_durable_migrations, seal_key_from_env, AgentTriggerClaimRequest,
    DurableAgentTriggerBacking, DurablePlacementBacking, HotTables, PgBootstrap, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};

const MODEL_MODE_ENV: &str = "MYELIN_HOSTED_MODEL_MODE";
const DETERMINISTIC_DEVELOPMENT_MODE: &str = "deterministic-development";

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("hosted-agent-worker");
    let bootstrap = PgBootstrap::from_env(Mode::RequireEnv)
        .await
        .unwrap_or_else(|error| refuse_start("database bootstrap", error));
    bootstrap
        .migrate_foundation()
        .await
        .unwrap_or_else(|error| refuse_start("substrate foundation migration", error));
    bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .unwrap_or_else(|error| refuse_start("durable migration aggregate", error));
    let provider = bootstrap
        .into_runtime()
        .await
        .unwrap_or_else(|error| refuse_start("database runtime handoff", error));

    let cell_id = required_cell_id().unwrap_or_else(|error| refuse_start("cell binding", error));
    let tenants = DurablePlacementBacking::new(provider.db_pool().clone())
        .local_tenants(&cell_id)
        .await
        .unwrap_or_else(|error| refuse_start("local-tenant directory", error))
        .into_iter()
        .filter(|placement| placement.active)
        .map(|placement| TenantId(placement.tenant_id))
        .collect::<Vec<_>>();
    if tenants.is_empty() {
        refuse_start(
            "local-tenant directory",
            format!("cell `{cell_id}` has no active tenants"),
        );
    }

    let runtime = tokio::runtime::Handle::current();
    let seal_key = seal_key_from_env().unwrap_or_else(|error| refuse_start("seal key", error));
    let host = Arc::new(
        AgentHost::new(provider.clone(), cell_id, &seal_key, runtime.clone())
            .await
            .unwrap_or_else(|error| refuse_start("hosted run identity", error)),
    );
    let models = model_factory().unwrap_or_else(|error| refuse_start("model mode", error));
    let edge_url = std::env::var("MYELIN_PUBLIC_BASE_URL").unwrap_or_else(|_| {
        refuse_start("governed tool broker", "MYELIN_PUBLIC_BASE_URL is missing")
    });
    let tool_executor = Arc::new(
        EdgeMcpToolExecutor::new(edge_url)
            .unwrap_or_else(|error| refuse_start("governed tool broker", error)),
    );
    let activity: Arc<dyn HostedAgentRunExecutor> = Arc::new(
        AgentHostActivityExecutor::new(host, provider.clone(), runtime.clone(), models)
            .with_tool_executor(tool_executor),
    );
    let region = Region(provider.config().region.clone());
    let mut workers = Vec::with_capacity(tenants.len() * PARTITION_COUNT as usize);
    for tenant in &tenants {
        for partition in 0..PARTITION_COUNT as i16 {
            let actor = Actor(Principal::new(
                tenant.clone(),
                region.clone(),
                PrincipalId("svc:hosted-agent-worker".into()),
                PrincipalKind::Service,
                DataRole::Processor,
                PrincipalStatus::Active,
            ));
            let scope = PgWorkerScope::new(
                tenant.clone(),
                region.clone(),
                partition,
                format!("hosted-agent-{partition}"),
                30,
                actor,
                1,
            )
            .unwrap_or_else(|error| refuse_start("workflow worker scope", error));
            let mut worker = PgFlowWorker::new(
                provider.db_pool().clone(),
                runtime.clone(),
                Arc::new(UlidMinter::new()),
                scope,
            );
            register_hosted_agent_workflow(
                &mut worker,
                HostedAgentInputResolver::new(provider.clone()),
                activity.clone(),
            )
            .unwrap_or_else(|error| refuse_start("hosted workflow registration", error));
            workers.push(worker);
        }
    }

    eprintln!(
        "hosted-agent-worker: driving {} exact tenant partitions in {}",
        workers.len(),
        region.0
    );
    run_workers(workers, provider, tenants).await;
}

async fn run_workers(
    workers: Vec<PgFlowWorker>,
    provider: SubstrateProvider,
    tenants: Vec<TenantId>,
) {
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let mut tasks = tokio::task::JoinSet::new();
    for worker in workers {
        let receiver = shutdown_rx.clone();
        tasks.spawn(async move {
            worker
                .run_until_shutdown(receiver, Duration::from_millis(250), 32)
                .await
                .map_err(|error| error.to_string())
        });
    }
    for tenant in tenants {
        let mut receiver = shutdown_rx.clone();
        let triggers = DurableAgentTriggerBacking::new(provider.clone());
        let handoff = TriggerRunHandoff::new(provider.clone(), tokio::runtime::Handle::current());
        handoff
            .register_workflow(&tenant)
            .unwrap_or_else(|error| refuse_start("trigger workflow registration", error));
        let handoff_tenant = tenant.clone();
        let handoff_triggers = triggers.clone();
        let mut handoff_shutdown = shutdown_rx.clone();
        tasks.spawn(async move {
            let claim_request = AgentTriggerClaimRequest::new(
                myelin_identity_service::HOSTED_LUNA_RUNTIME,
                "hosted-agent-handoff",
                30,
            )
            .map_err(str::to_string)?;
            loop {
                tokio::select! {
                    changed = handoff_shutdown.changed() => {
                        if changed.is_err() || *handoff_shutdown.borrow() {
                            return Ok(());
                        }
                    }
                    () = tokio::time::sleep(Duration::from_millis(100)) => {
                        if let Some(claim) = handoff_triggers
                            .claim_next_firing(&handoff_tenant.0, claim_request.clone())
                            .await
                            .map_err(|error| error.to_string())?
                        {
                            handoff
                                .start(&handoff_tenant, &claim)
                                .await
                                .map_err(|error| error.to_string())?;
                        }
                    }
                }
            }
        });
        tasks.spawn(async move {
            loop {
                tokio::select! {
                    changed = receiver.changed() => {
                        if changed.is_err() || *receiver.borrow() {
                            return Ok(());
                        }
                    }
                    () = tokio::time::sleep(Duration::from_millis(250)) => {
                        triggers
                            .reconcile_terminal_firings(&tenant.0, 100)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
        });
    }

    let early_failure = tokio::select! {
        () = shutdown_signal() => None,
        result = tasks.join_next() => Some(result),
    };
    let _ = shutdown_tx.send(true);
    if let Some(result) = early_failure {
        match result {
            Some(Ok(Ok(()))) => refuse_start("workflow worker", "stopped before shutdown"),
            Some(Ok(Err(error))) => refuse_start("workflow worker", error),
            Some(Err(error)) => refuse_start("workflow worker task", error),
            None => refuse_start("workflow worker set", "ended unexpectedly"),
        }
    }

    let drained = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => refuse_start("workflow worker drain", error),
                Err(error) => refuse_start("workflow worker task drain", error),
            }
        }
    })
    .await;
    if drained.is_err() {
        refuse_start("workflow worker drain", "did not finish within 10 seconds");
    }
}

fn model_factory() -> Result<Arc<dyn HostedModelFactory>, String> {
    match std::env::var(MODEL_MODE_ENV)
        .unwrap_or_else(|_| "luna".into())
        .as_str()
    {
        "luna" => Ok(Arc::new(LunaModelFactory)),
        DETERMINISTIC_DEVELOPMENT_MODE => {
            eprintln!(
                "hosted-agent-worker: using explicit deterministic development model; no provider call will be made"
            );
            Ok(Arc::new(DeterministicDevelopmentModelFactory))
        }
        value => Err(format!(
            "{MODEL_MODE_ENV} must be `luna` or `{DETERMINISTIC_DEVELOPMENT_MODE}`, got `{value}`"
        )),
    }
}

struct DeterministicDevelopmentModelFactory;

impl HostedModelFactory for DeterministicDevelopmentModelFactory {
    fn client(&self) -> Result<Box<dyn ModelClient + Send + Sync>, ModelError> {
        Ok(Box::new(DeterministicDevelopmentModel))
    }
}

struct DeterministicDevelopmentModel;

impl ModelClient for DeterministicDevelopmentModel {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let has_ci_triage_tools = ["ci.read_run", "issues.create"]
            .iter()
            .all(|name| request.tools.iter().any(|tool| tool.name == *name));
        let tool_results = request
            .turns
            .iter()
            .filter(|turn| matches!(turn, ModelTurn::ToolResults(_)))
            .count();
        let reply = match (has_ci_triage_tools, tool_results) {
            (true, 0) => ModelReply::ToolCalls(vec![ToolCallRequest {
                id: "read-triggering-ci-run".into(),
                name: "ci.read_run".into(),
                arguments: serde_json::json!({
                    "run_id": triggering_ci_run_id(request)?,
                }),
            }]),
            (true, 1) => ModelReply::ToolCalls(vec![ToolCallRequest {
                id: "open-triage-issue".into(),
                name: "issues.create".into(),
                arguments: serde_json::json!({
                    "project_id": required_development_value("MYELIN_ISSUES_PROJECT")?,
                    "type_id": required_development_value("MYELIN_ISSUES_TYPE")?,
                    "prefix": required_development_value("MYELIN_ISSUES_PREFIX")?,
                    "title": format!(
                        "CI failure {} needs triage",
                        triggering_ci_run_id(request)?
                    ),
                }),
            }]),
            _ => ModelReply::Final {
                content: "Read the failing CI run and opened one governed triage issue.".into(),
            },
        };
        Ok(ModelResponse {
            reply,
            usage: Usage::Reported {
                input: 100,
                cached_input: 0,
                output: 10,
            },
        })
    }
}

fn triggering_ci_run_id(request: &ModelRequest) -> Result<String, ModelError> {
    let prompt = request
        .turns
        .iter()
        .find_map(|turn| match turn {
            ModelTurn::User { content } => Some(content.as_str()),
            _ => None,
        })
        .ok_or_else(|| ModelError::Parse("development run has no user prompt".into()))?;
    let marker = "/ci/run/";
    let start = prompt
        .find(marker)
        .map(|index| index + marker.len())
        .ok_or_else(|| ModelError::Parse("development trigger has no CI run reference".into()))?;
    let run_id = prompt[start..]
        .chars()
        .take_while(|character| character.is_ascii_hexdigit() || *character == '-')
        .collect::<String>();
    let parsed = uuid::Uuid::parse_str(&run_id)
        .map_err(|_| ModelError::Parse("development trigger CI run is not a UUID".into()))?;
    if parsed.to_string() != run_id {
        return Err(ModelError::Parse(
            "development trigger CI run is not canonical".into(),
        ));
    }
    Ok(run_id)
}

fn required_development_value(name: &str) -> Result<String, ModelError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty() && !value.contains("{{"))
        .ok_or_else(|| ModelError::Parse(format!("development model requires {name}")))
}

fn required_cell_id() -> Result<String, &'static str> {
    let value = std::env::var("MYELIN_CELL_ID").map_err(|_| "MYELIN_CELL_ID is required")?;
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err("MYELIN_CELL_ID must be a trimmed opaque token of at most 128 bytes");
    }
    Ok(value)
}

fn refuse_start(context: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("hosted-agent-worker: {context} refused to continue: {error}");
    std::process::exit(1)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .unwrap_or_else(|error| refuse_start("SIGTERM handler", error));
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    refuse_start("SIGINT handler", error);
                }
            }
            signal = terminate.recv() => {
                if signal.is_none() {
                    refuse_start("SIGTERM stream", "closed unexpectedly");
                }
            }
        }
    }
    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        refuse_start("shutdown handler", error);
    }
}
