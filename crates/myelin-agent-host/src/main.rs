use std::sync::Arc;
use std::time::Duration;

use myelin_agent_host::supervision::PollBackoff;
use myelin_agent_host::{
    register_hosted_agent_workflow, AgentHost, AgentHostActivityExecutor, EdgeMcpToolExecutor,
    HostedAgentInputResolver, HostedAgentRunExecutor, HostedModelFactory, LunaModelFactory,
};
#[cfg(any(test, feature = "deterministic-development-model"))]
use myelin_agent_model::{
    ModelClient, ModelError, ModelReply, ModelRequest, ModelResponse, ModelTurn, ToolCallRequest,
    Usage,
};
use myelin_agent_service::trigger_handoff::{TriggerHandoffDisposition, TriggerRunHandoff};
use myelin_config::Mode;
use myelin_events::{Actor, UlidMinter};
use myelin_flow::{PgFlowWorker, PgWorkerScope, PARTITION_COUNT};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_storage::{
    all_durable_migrations, seal_key_from_env, AgentTriggerClaimRequest,
    DurableAgentTriggerBacking, DurablePlacementBacking, HotTables, PgBootstrap, SubstrateProvider,
    TerminalizeAgentTriggerClaimOutcome,
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
        EdgeMcpToolExecutor::new(edge_url, runtime.clone())
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
    let handoff_worker_id = format!("hosted-agent-handoff-{}", uuid::Uuid::new_v4());
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
        let claim_request = AgentTriggerClaimRequest::new(
            myelin_identity_service::HOSTED_LUNA_RUNTIME,
            handoff_worker_id.clone(),
            30,
        )
        .unwrap_or_else(|error| refuse_start("trigger claim identity", error));
        tasks.spawn(async move {
            run_trigger_handoffs(
                handoff_tenant,
                handoff_triggers,
                handoff,
                claim_request,
                &mut handoff_shutdown,
            )
            .await
        });
        tasks
            .spawn(async move { reconcile_trigger_firings(tenant, triggers, &mut receiver).await });
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

async fn run_trigger_handoffs(
    tenant: TenantId,
    triggers: DurableAgentTriggerBacking,
    handoff: TriggerRunHandoff,
    claim_request: AgentTriggerClaimRequest,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let mut backoff = PollBackoff::new(Duration::from_millis(100), Duration::from_secs(5));
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            () = tokio::time::sleep(backoff.next_delay()) => {
                let claim = match triggers.claim_next_firing(&tenant.0, claim_request.clone()).await {
                    Ok(claim) => {
                        backoff.succeeded();
                        claim
                    }
                    Err(error) => {
                        eprintln!(
                            "hosted-agent-worker: trigger queue unavailable for tenant {}: {error}",
                            tenant.0,
                        );
                        backoff.failed();
                        continue;
                    }
                };
                let Some(claim) = claim else { continue };
                let Err(error) = handoff.start(&tenant, &claim).await else { continue };
                match error.disposition() {
                    TriggerHandoffDisposition::ClaimLost => eprintln!(
                        "hosted-agent-worker: trigger claim was canceled or reclaimed for tenant {}",
                        tenant.0,
                    ),
                    TriggerHandoffDisposition::Retry => eprintln!(
                        "hosted-agent-worker: trigger handoff will retry after its lease for tenant {}: {error}",
                        tenant.0,
                    ),
                    TriggerHandoffDisposition::Terminal { reason } => {
                        isolate_poison_firing(&triggers, &tenant, &claim, &reason).await;
                    }
                }
            }
        }
    }
}

async fn isolate_poison_firing(
    triggers: &DurableAgentTriggerBacking,
    tenant: &TenantId,
    claim: &myelin_storage::ClaimedAgentTriggerFiring,
    reason: &str,
) {
    match triggers.terminalize_claim(&tenant.0, claim, reason).await {
        Ok(TerminalizeAgentTriggerClaimOutcome::Terminalized) => eprintln!(
            "hosted-agent-worker: isolated an invalid trigger firing for tenant {}: {reason}",
            tenant.0,
        ),
        Ok(TerminalizeAgentTriggerClaimOutcome::ClaimUnavailable) => eprintln!(
            "hosted-agent-worker: invalid trigger claim was already canceled or reclaimed for tenant {}",
            tenant.0,
        ),
        Err(error) => eprintln!(
            "hosted-agent-worker: could not isolate an invalid trigger firing for tenant {}: {error}",
            tenant.0,
        ),
    }
}

async fn reconcile_trigger_firings(
    tenant: TenantId,
    triggers: DurableAgentTriggerBacking,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let mut backoff = PollBackoff::new(Duration::from_millis(250), Duration::from_secs(5));
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
            () = tokio::time::sleep(backoff.next_delay()) => {
                match triggers.reconcile_terminal_firings(&tenant.0, 100).await {
                    Ok(_) => backoff.succeeded(),
                    Err(error) => {
                        eprintln!(
                            "hosted-agent-worker: trigger reconciliation unavailable for tenant {}: {error}",
                            tenant.0,
                        );
                        backoff.failed();
                    }
                }
            }
        }
    }
}

fn model_factory() -> Result<Arc<dyn HostedModelFactory>, String> {
    let mode = std::env::var(MODEL_MODE_ENV).unwrap_or_else(|_| "luna".into());
    model_factory_for(&mode)
}

fn model_factory_for(mode: &str) -> Result<Arc<dyn HostedModelFactory>, String> {
    match mode {
        "luna" => Ok(Arc::new(LunaModelFactory)),
        DETERMINISTIC_DEVELOPMENT_MODE => deterministic_development_model_factory(),
        value => Err(format!(
            "{MODEL_MODE_ENV} must be `luna` or `{DETERMINISTIC_DEVELOPMENT_MODE}`, got `{value}`"
        )),
    }
}

#[cfg(any(test, feature = "deterministic-development-model"))]
fn deterministic_development_model_factory() -> Result<Arc<dyn HostedModelFactory>, String> {
    let project_id = required_development_project_id()?;
    eprintln!(
        "hosted-agent-worker: using explicit deterministic development model; no provider call will be made"
    );
    Ok(Arc::new(DeterministicDevelopmentModelFactory {
        project_id,
    }))
}

#[cfg(not(any(test, feature = "deterministic-development-model")))]
fn deterministic_development_model_factory() -> Result<Arc<dyn HostedModelFactory>, String> {
    Err(format!(
        "{DETERMINISTIC_DEVELOPMENT_MODE} is unavailable in this worker build"
    ))
}

#[cfg(any(test, feature = "deterministic-development-model"))]
struct DeterministicDevelopmentModelFactory {
    project_id: String,
}

#[cfg(any(test, feature = "deterministic-development-model"))]
impl HostedModelFactory for DeterministicDevelopmentModelFactory {
    fn client(&self) -> Result<Box<dyn ModelClient + Send + Sync>, ModelError> {
        Ok(Box::new(DeterministicDevelopmentModel {
            project_id: self.project_id.clone(),
        }))
    }
}

#[cfg(any(test, feature = "deterministic-development-model"))]
struct DeterministicDevelopmentModel {
    project_id: String,
}

#[cfg(any(test, feature = "deterministic-development-model"))]
impl ModelClient for DeterministicDevelopmentModel {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let has_ci_triage_tools = ["ci.read_run", "issues.create"]
            .iter()
            .all(|name| request.tools.iter().any(|tool| tool.name == *name));
        let has_merge_tool = request.tools.iter().any(|tool| tool.name == "git.merge");
        let tool_results = request
            .turns
            .iter()
            .filter(|turn| matches!(turn, ModelTurn::ToolResults(_)))
            .count();
        let reply = match (has_ci_triage_tools, has_merge_tool, tool_results) {
            (true, _, 0) => ModelReply::ToolCalls(vec![ToolCallRequest {
                id: "read-triggering-ci-run".into(),
                name: "ci.read_run".into(),
                arguments: serde_json::json!({
                    "run_id": triggering_ci_run(request)?.run_id,
                }),
            }]),
            (true, _, 1) => {
                let triggering_run = triggering_ci_run(request)?;
                ModelReply::ToolCalls(vec![ToolCallRequest {
                    id: "open-triage-issue".into(),
                    name: "issues.create".into(),
                    arguments: serde_json::json!({
                        "project_ref": format!(
                            "myelin://{}/identity/project/{}",
                            triggering_run.tenant,
                            self.project_id,
                        ),
                        "title": format!(
                            "CI failure {} needs triage",
                            triggering_run.run_id,
                        ),
                    }),
                }])
            }
            (false, true, 0) => {
                let (repo, number) = triggering_merge_target(request)?;
                ModelReply::ToolCalls(vec![ToolCallRequest {
                    id: "merge-approved-pull-request".into(),
                    name: "git.merge".into(),
                    arguments: serde_json::json!({ "repo": repo, "number": number }),
                }])
            }
            (false, true, _) => ModelReply::Final {
                content: "Applied the exact pull-request merge approved by a human.".into(),
            },
            (true, false, _) => ModelReply::Final {
                content: "Read the failing CI run and opened one governed triage issue.".into(),
            },
            _ => ModelReply::Final {
                content: "The delegated tools do not match a scripted development workflow; no action was taken."
                    .into(),
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

#[cfg(any(test, feature = "deterministic-development-model"))]
fn triggering_merge_target(request: &ModelRequest) -> Result<(String, u64), ModelError> {
    let prompt = request
        .turns
        .iter()
        .find_map(|turn| match turn {
            ModelTurn::User { content } => Some(content.as_str()),
            _ => None,
        })
        .ok_or_else(|| ModelError::Parse("development run has no user prompt".into()))?;
    let marker = "Merge pull request ";
    let target = prompt
        .find(marker)
        .map(|start| &prompt[start + marker.len()..])
        .and_then(|remainder| remainder.split_whitespace().next())
        .and_then(|target| target.trim_end_matches('.').split_once('#'))
        .ok_or_else(|| {
            ModelError::Parse(
                "development merge task must contain `Merge pull request <repo>#<number>`".into(),
            )
        })?;
    let (repo, number) = target;
    if repo.is_empty()
        || repo.len() > 255
        || !repo
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
    {
        return Err(ModelError::Parse(
            "development merge task has an invalid repository slug".into(),
        ));
    }
    let number = number
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| {
            ModelError::Parse("development merge task has an invalid PR number".into())
        })?;
    Ok((repo.to_string(), number))
}

#[cfg(any(test, feature = "deterministic-development-model"))]
struct TriggeringCiRun {
    tenant: String,
    run_id: String,
}

#[cfg(any(test, feature = "deterministic-development-model"))]
fn triggering_ci_run(request: &ModelRequest) -> Result<TriggeringCiRun, ModelError> {
    let prompt = request
        .turns
        .iter()
        .find_map(|turn| match turn {
            ModelTurn::User { content } => Some(content.as_str()),
            _ => None,
        })
        .ok_or_else(|| ModelError::Parse("development run has no user prompt".into()))?;
    let marker = "Trigger: ci.run.failed on ";
    let artifact = prompt
        .find(marker)
        .map(|index| &prompt[index + marker.len()..])
        .and_then(|remainder| remainder.lines().next())
        .ok_or_else(|| ModelError::Parse("development trigger has no CI run reference".into()))?;
    let parsed = myelin_refs::parse_scoped(artifact)
        .map_err(|_| ModelError::Parse("development trigger CI run is not canonical".into()))?;
    if parsed.subsystem != "ci" || parsed.type_ != "run" || parsed.sub.is_some() {
        return Err(ModelError::Parse(
            "development trigger does not name a CI run root".into(),
        ));
    }
    let run_id = uuid::Uuid::parse_str(&parsed.id)
        .map_err(|_| ModelError::Parse("development trigger CI run is not a UUID".into()))?;
    if run_id.to_string() != parsed.id {
        return Err(ModelError::Parse(
            "development trigger CI run is not canonical".into(),
        ));
    }
    Ok(TriggeringCiRun {
        tenant: parsed.tenant.0,
        run_id: parsed.id,
    })
}

#[cfg(any(test, feature = "deterministic-development-model"))]
fn required_development_project_id() -> Result<String, String> {
    let name = "MYELIN_ISSUES_PROJECT";
    let value = std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty() && !value.contains("{{"))
        .ok_or_else(|| format!("development model requires {name}"))?;
    let parsed = uuid::Uuid::parse_str(&value)
        .map_err(|_| format!("{name} must be a canonical project UUID"))?;
    if parsed.to_string() != value {
        return Err(format!("{name} must be a canonical project UUID"));
    }
    Ok(value)
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

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent_model::{ToolCallResult, ToolSpec};

    const PROJECT_ID: &str = "20aee030-c7fa-4757-8243-700faf528690";

    fn development_model() -> DeterministicDevelopmentModel {
        DeterministicDevelopmentModel {
            project_id: PROJECT_ID.into(),
        }
    }

    fn request_with_tools(names: &[&str]) -> ModelRequest {
        ModelRequest {
            turns: vec![ModelTurn::User {
                content: "Find the failure.\n\nTrigger: ci.run.failed on myelin://acme/ci/run/65274e14-2e61-8bc9-e1a5-6345afea6ad6\nEvent payload: {}".into(),
            }],
            tools: names
                .iter()
                .map(|name| ToolSpec {
                    name: (*name).into(),
                    description: String::new(),
                    input_schema: serde_json::json!({}),
                })
                .collect(),
            ..ModelRequest::default()
        }
    }

    #[test]
    fn the_development_model_never_claims_actions_hidden_by_delegation() {
        let response = development_model()
            .complete(&request_with_tools(&["issues.create"]))
            .expect("the explicit development model responds without provider I/O");

        assert_eq!(
            response.reply,
            ModelReply::Final {
                content: "The delegated tools do not match a scripted development workflow; no action was taken."
                    .into(),
            }
        );
    }

    #[test]
    fn the_development_model_uses_the_canonical_issue_create_contract() {
        let mut request = request_with_tools(&["ci.read_run", "issues.create"]);
        request
            .turns
            .push(ModelTurn::ToolResults(vec![ToolCallResult {
                id: "read-triggering-ci-run".into(),
                content: "the contract job failed".into(),
            }]));

        let response = development_model()
            .complete(&request)
            .expect("the explicit development model can propose its second step");

        assert_eq!(
            response.reply,
            ModelReply::ToolCalls(vec![ToolCallRequest {
                id: "open-triage-issue".into(),
                name: "issues.create".into(),
                arguments: serde_json::json!({
                    "project_ref": format!("myelin://acme/identity/project/{PROJECT_ID}"),
                    "title": "CI failure 65274e14-2e61-8bc9-e1a5-6345afea6ad6 needs triage",
                }),
            }])
        );
    }
}
