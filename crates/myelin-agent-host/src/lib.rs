use std::sync::{Arc, Mutex};

mod durable_model;

pub mod git_read_tool;
pub use git_read_tool::{
    git_check_status_read_tool_def, git_check_status_read_tool_schema, GitCheckStatusReadExecutor,
    GIT_READ_CHECK_STATUS_TOOL,
};

pub mod identity;
pub use identity::timestamp_from_epoch;
pub mod workflow;
use durable_model::DurableModelClient;
use identity::{IdentityRunMinter, IdentityRunRevoker};
pub use workflow::{
    register_hosted_agent_workflow, HostedAgentInputResolver, HostedAgentRunExecutor,
    HostedAgentWorkflowInput,
};

use myelin_agent::{
    MeteredRuntime, ToolCall, ToolDef, ToolName, ToolResult, ToolSchema, ToolSurface,
};
use myelin_agent_model::{
    LlmAgentRuntime, ModelClient, ModelError, ModelReply, ModelRequest, ModelResponse, ModelTurn,
    ToolSpec,
};
use myelin_agent_service::{
    RunOutcomeKind, RunSubstrate, SkeletonAgent, SkeletonError, SkeletonTelemetry, ToolExecError,
    ToolExecutor,
};
use myelin_events::{IdMinter, OutboxStore};
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter, WfJournal};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal, Zookie,
};
use myelin_storage::agent_wallet::AgentWallet;
use myelin_storage::reserve_settle::CostLedger;
use myelin_storage::AgentModelStepStore;
use myelin_storage::{
    DurableCellRootBacking, DurableDelegationPolicyBacking, DurableRevocationBacking, SealKey,
    SubstrateProvider, TenantScope,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use myelin_identity_service::{
    CellTokenAuthority, DelegationPolicySource, PasetoCapabilitySigner, RevocationStore,
    RunTokenMinter as IdentityRunTokenMinter,
};

pub use myelin_agent_service::{RunTokenRevoker, RunWallet};
pub use myelin_storage::agent_wallet::{CreditKind, DebitOutcome, MicroUsd, WalletError};

#[derive(Clone, Debug)]
pub struct LlmRunTask {
    pub tenant: TenantId,
    pub agent: Principal,
    pub trigger_actor: Principal,
    pub agent_id: String,
    pub run_id: String,
    pub delegation_caveats: DelegationCaveats,
    pub system: String,
    pub prompt: String,
    pub token_ttl_secs: u64,
    pub estimate: MicroUsd,
    pub now_secs: i64,
    pub available: MicroUsd,
    pub max_output_tokens: Option<u32>,
}

impl LlmRunTask {
    pub fn new(
        tenant: TenantId,
        agent: Principal,
        agent_id: impl Into<String>,
        run_id: impl Into<String>,
        system: impl Into<String>,
        prompt: impl Into<String>,
    ) -> LlmRunTask {
        let trigger_actor = agent.clone();
        LlmRunTask {
            tenant,
            agent,
            trigger_actor,
            agent_id: agent_id.into(),
            run_id: run_id.into(),
            delegation_caveats: DelegationCaveats(vec![]),
            system: system.into(),
            prompt: prompt.into(),
            token_ttl_secs: 300,
            estimate: MicroUsd(100_000),
            available: MicroUsd(1_000_000),
            now_secs: 0,
            max_output_tokens: None,
        }
    }

    pub fn with_max_output_tokens(mut self, max: u32) -> LlmRunTask {
        self.max_output_tokens = Some(max);
        self
    }

    pub fn with_now_secs(mut self, now_secs: i64) -> LlmRunTask {
        self.now_secs = now_secs;
        self
    }

    pub fn with_reservation_budget(mut self, budget: MicroUsd) -> LlmRunTask {
        self.estimate = budget;
        self
    }

    pub fn with_delegation(
        mut self,
        trigger_actor: Principal,
        caveats: DelegationCaveats,
    ) -> LlmRunTask {
        self.trigger_actor = trigger_actor;
        self.delegation_caveats = caveats;
        self
    }
}

#[derive(Clone, Debug)]
pub struct LlmRunReport {
    pub outcome: myelin_agent::RunOutcome,
    pub answer: String,
    pub charged_micro: u64,
    pub telemetry: SkeletonTelemetry,
}

#[derive(Clone, Debug)]
pub enum AgentHostError {
    Run(SkeletonError),
    Identity(String),
}

impl core::fmt::Display for AgentHostError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AgentHostError::Run(e) => write!(f, "hosted-agent run failed: {e}"),
            AgentHostError::Identity(e) => write!(f, "hosted-agent identity failed: {e}"),
        }
    }
}

impl std::error::Error for AgentHostError {}

impl From<SkeletonError> for AgentHostError {
    fn from(e: SkeletonError) -> AgentHostError {
        AgentHostError::Run(e)
    }
}

pub struct RunSubstrateWiring<'a> {
    pub ledger: &'a mut CostLedger,
    pub outbox: &'a OutboxStore,
    pub id_minter: Arc<dyn IdMinter>,
    pub journal: WfJournal,
}

pub struct Tools<'a> {
    pub catalogue: &'a dyn ToolSurface,
    pub executor: &'a dyn ToolExecutor,
    pub advertised: &'a [ToolSchema],
}

impl<'a> Tools<'a> {
    pub fn none() -> Tools<'static> {
        Tools {
            catalogue: &NoToolSurface,
            executor: &NoToolExecutor,
            advertised: &[],
        }
    }
}

#[derive(Clone, Default)]
struct AnswerSlot(Arc<Mutex<Option<String>>>);

impl AnswerSlot {
    fn set(&self, answer: String) {
        *self.0.lock().expect("answer slot lock") = Some(answer);
    }
    fn take(&self) -> Option<String> {
        self.0.lock().expect("answer slot lock").take()
    }
}

struct HostModelClient {
    system: String,
    prompt: String,
    tool_specs: Vec<ToolSpec>,
    inner: Box<dyn ModelClient + Send + Sync>,
    answer: AnswerSlot,
}

impl HostModelClient {
    fn wrap(
        system: String,
        prompt: String,
        tool_specs: Vec<ToolSpec>,
        inner: Box<dyn ModelClient + Send + Sync>,
    ) -> (HostModelClient, AnswerSlot) {
        let answer = AnswerSlot::default();
        (
            HostModelClient {
                system,
                prompt,
                tool_specs,
                inner,
                answer: answer.clone(),
            },
            answer,
        )
    }
}

impl ModelClient for HostModelClient {
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let mut req = request.clone();
        if req.turns.is_empty() {
            if req.system.trim().is_empty() {
                req.system = self.system.clone();
            }
            if !self.prompt.is_empty() {
                req.turns.push(ModelTurn::User {
                    content: self.prompt.clone(),
                });
            }
        }
        if !self.tool_specs.is_empty() {
            req.tools = self.tool_specs.clone();
        }
        let response = self.inner.complete(&req)?;
        if let ModelReply::Final { content } = &response.reply {
            self.answer.set(content.clone());
        }
        Ok(response)
    }
}

#[derive(Default)]
struct SyntheticRunMinter;

impl RunTokenMinter for SyntheticRunMinter {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        _caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        Ok(RunTokenHandle {
            token: format!("host-tok:{agent_id}:{run_id}"),
            jti: format!("host-jti:{agent_id}:{run_id}"),
            ttl_secs,
        })
    }
}

#[derive(Default)]
struct SyntheticRunRevoker {
    revoked: Mutex<std::collections::HashSet<String>>,
}

impl RunTokenRevoker for SyntheticRunRevoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        let mut g = self.revoked.lock().expect("revoker lock");
        if !g.insert(jti.to_string()) {
            return 0;
        }
        (now_secs - teardown_secs).max(0) as u64
    }
    fn is_dead(&self, jti: &str, _now_secs: i64) -> bool {
        self.revoked.lock().expect("revoker lock").contains(jti)
    }
}

#[derive(Default)]
pub struct NoToolSurface;

impl ToolSurface for NoToolSurface {
    fn register_tool(&mut self, _def: ToolDef) {}
    fn resolve(&self, _name: &ToolName) -> Option<&ToolDef> {
        None
    }
}

#[derive(Default)]
pub struct NoToolExecutor;

impl ToolExecutor for NoToolExecutor {
    fn execute(&self, def: &ToolDef, _call: &ToolCall) -> Result<ToolResult, ToolExecError> {
        Err(ToolExecError::Failed(format!(
            "no-tools run attempted to execute `{}` (bug)",
            def.name.0
        )))
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolCatalogue {
    defs: Vec<ToolDef>,
}

impl ToolCatalogue {
    pub fn new(defs: impl IntoIterator<Item = ToolDef>) -> ToolCatalogue {
        ToolCatalogue {
            defs: defs.into_iter().collect(),
        }
    }
}

impl ToolSurface for ToolCatalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

fn tool_schema_to_spec(schema: &ToolSchema) -> ToolSpec {
    ToolSpec {
        name: schema.name.0.clone(),
        description: schema.description.clone(),
        input_schema: serde_json::from_str(&schema.input_schema)
            .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
    }
}

type ToolResourceResolver = dyn Fn(&ToolDef, &ToolCall) -> Option<ArtifactRef> + Send + Sync;

pub fn git_read_check_status_resource(_def: &ToolDef, call: &ToolCall) -> Option<ArtifactRef> {
    call.arguments
        .get("repo")
        .and_then(|v| v.as_str())
        .map(|repo| ArtifactRef(repo.to_string()))
}

fn strong_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

pub struct CapEnforcingExecutor<'a> {
    identity: Arc<dyn IdentityService + Send + Sync>,
    principal: Principal,
    inner: &'a dyn ToolExecutor,
    resource_of: Box<ToolResourceResolver>,
}

impl<'a> CapEnforcingExecutor<'a> {
    pub fn new(
        identity: Arc<dyn IdentityService + Send + Sync>,
        principal: Principal,
        inner: &'a dyn ToolExecutor,
        resource_of: Box<ToolResourceResolver>,
    ) -> CapEnforcingExecutor<'a> {
        CapEnforcingExecutor {
            identity,
            principal,
            inner,
            resource_of,
        }
    }

    pub fn for_git_read_tool(
        identity: Arc<dyn IdentityService + Send + Sync>,
        principal: Principal,
        inner: &'a dyn ToolExecutor,
    ) -> CapEnforcingExecutor<'a> {
        CapEnforcingExecutor::new(
            identity,
            principal,
            inner,
            Box::new(git_read_check_status_resource),
        )
    }
}

impl ToolExecutor for CapEnforcingExecutor<'_> {
    fn execute(&self, def: &ToolDef, call: &ToolCall) -> Result<ToolResult, ToolExecError> {
        for cap in &def.required_caps {
            let resource = (self.resource_of)(def, call).ok_or_else(|| {
                ToolExecError::Failed(format!(
                    "cap-enforcement DENY: no ReBAC resource for `{}` (cap `{cap}`)",
                    def.name.0
                ))
            })?;
            match self.identity.check(
                &self.principal,
                &Permission(cap.clone()),
                &resource,
                &strong_latest(),
                None,
            ) {
                Ok(Decision::Allow) => {}
                Ok(other) => {
                    return Err(ToolExecError::Failed(format!(
                        "cap-enforcement DENY: `{}` not authorized for `{cap}` on `{}` ({other:?})",
                        self.principal.principal_id.0, resource.0
                    )))
                }
                Err(e) => {
                    return Err(ToolExecError::Failed(format!(
                        "cap-enforcement DENY: ReBAC check for `{cap}` on `{}` failed ({e:?})",
                        resource.0
                    )))
                }
            }
        }
        self.inner.execute(def, call)
    }
}

struct RunTokenSeams<'a> {
    minter: Arc<dyn RunTokenMinter + Send + Sync>,
    revoker: &'a dyn RunTokenRevoker,
}

/// Test harness for the model/runtime loop. Production hosts must use [`AgentHost::new`]
/// so the run receives a real, short-lived Identity credential and durable teardown revocation.
#[doc(hidden)]
pub fn dispatch_test_run_with_synthetic_identity(
    wallet: &dyn RunWallet,
    region: Region,
    task: &LlmRunTask,
    wiring: &mut RunSubstrateWiring<'_>,
    model_client: Box<dyn ModelClient + Send + Sync>,
    tools: Tools<'_>,
) -> Result<LlmRunReport, AgentHostError> {
    let revoker = SyntheticRunRevoker::default();
    dispatch_core(
        wallet,
        region,
        task,
        wiring,
        model_client,
        tools,
        RunTokenSeams {
            minter: Arc::new(SyntheticRunMinter),
            revoker: &revoker,
        },
    )
}

fn dispatch_core(
    wallet: &dyn RunWallet,
    region: Region,
    task: &LlmRunTask,
    wiring: &mut RunSubstrateWiring<'_>,
    model_client: Box<dyn ModelClient + Send + Sync>,
    tools: Tools<'_>,
    seams: RunTokenSeams<'_>,
) -> Result<LlmRunReport, AgentHostError> {
    let tool_specs = tools.advertised.iter().map(tool_schema_to_spec).collect();
    let (host_client, answer) = HostModelClient::wrap(
        default_system(&task.system),
        task.prompt.clone(),
        tool_specs,
        model_client,
    );
    let mut runtime = LlmAgentRuntime::new(Box::new(host_client));
    if let Some(max) = task.max_output_tokens {
        runtime = runtime.with_max_output_tokens(max);
    }

    let mut gate = myelin_storage::agent_run_gate::AgentRunGate::new();
    let mut telemetry = SkeletonTelemetry::new();
    let agent_loop = SkeletonAgent::new();

    let outstanding = wiring
        .ledger
        .outstanding_reservations(&task.tenant)
        .map_err(|e| AgentHostError::Run(SkeletonError::DispatchRefused(e.to_string())))?;
    let available = MicroUsd(wallet.balance(&task.tenant).0.saturating_sub(outstanding.0));

    let mut sub = RunSubstrate {
        tenant: task.tenant.clone(),
        region,
        agent: task.agent.clone(),
        run_id: task.run_id.clone(),
        minter_token: seams.minter,
        agent_id: task.agent_id.clone(),
        caveats: task.delegation_caveats.clone(),
        token_ttl_secs: task.token_ttl_secs,
        revoker: seams.revoker,
        catalogue: tools.catalogue,
        executor: tools.executor,
        wallet: Some(wallet),
        gate: &mut gate,
        ledger: wiring.ledger,
        available,
        estimate: task.estimate,
        outbox: wiring.outbox,
        minter: wiring.id_minter.clone(),
        journal: wiring.journal.clone(),
        now_secs: task.now_secs,
    };

    let outcome = agent_loop.handle_run(
        &runtime as &dyn MeteredRuntime,
        &mut sub,
        &mut telemetry,
        RunOutcomeKind::Completed,
    )?;

    Ok(LlmRunReport {
        answer: answer.take().unwrap_or_default(),
        charged_micro: telemetry.charged_micro(),
        outcome,
        telemetry,
    })
}

fn default_system(system: &str) -> String {
    if system.trim().is_empty() {
        "You are a hosted agent. You are labelled as an agent. Answer the user's request \
         concisely and directly."
            .to_string()
    } else {
        system.to_string()
    }
}

pub struct AgentHost {
    region: Region,
    wallet: AgentWallet,
    model_steps: AgentModelStepStore,
    identity: HostIdentity,
    runtime: tokio::runtime::Handle,
}

struct HostIdentity {
    minter: IdentityRunTokenMinter,
    revocations: RevocationStore,
    policies: DelegationPolicySource,
}

#[derive(Debug)]
pub enum HostIdentityError {
    CellRootUnavailable(String),
    InvalidCellRoot(String),
}

impl core::fmt::Display for HostIdentityError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HostIdentityError::CellRootUnavailable(e) => {
                write!(
                    f,
                    "hosted-agent Identity root refused to start (fail-closed): {e}"
                )
            }
            HostIdentityError::InvalidCellRoot(e) => {
                write!(
                    f,
                    "hosted-agent Identity cell-authority material is invalid: {e}"
                )
            }
        }
    }
}

impl std::error::Error for HostIdentityError {}

impl AgentHost {
    pub async fn new(
        provider: SubstrateProvider,
        cell_id: impl Into<String>,
        seal_key: &SealKey,
        rt: tokio::runtime::Handle,
    ) -> Result<AgentHost, HostIdentityError> {
        let region = Region(provider.config().region.clone());
        let material = DurableCellRootBacking::new(provider.db_pool().clone(), cell_id)
            .load_or_generate(seal_key)
            .await
            .map_err(|e| HostIdentityError::CellRootUnavailable(e.to_string()))?;
        let cell = Arc::new(
            CellTokenAuthority::from_material(&material)
                .map_err(|e| HostIdentityError::InvalidCellRoot(format!("{e:?}")))?,
        );
        let revocations =
            RevocationStore::with_pg(DurableRevocationBacking::new(provider.clone()), rt.clone());
        let signer = Arc::new(PasetoCapabilitySigner::new(cell));
        let minter =
            IdentityRunTokenMinter::with_signer_and_tuples(revocations.clone(), None, signer);
        let policies =
            DelegationPolicySource::with_pg(DurableDelegationPolicyBacking::new(provider.clone()));
        Ok(AgentHost {
            region,
            wallet: AgentWallet::new(provider.clone()),
            model_steps: AgentModelStepStore::with_runtime(provider, rt.clone()),
            identity: HostIdentity {
                minter,
                revocations,
                policies,
            },
            runtime: rt,
        })
    }

    pub fn wallet(&self) -> &AgentWallet {
        &self.wallet
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn revocations(&self) -> &RevocationStore {
        &self.identity.revocations
    }

    fn identity_seams(
        &self,
        task: &LlmRunTask,
    ) -> Result<(Arc<dyn RunTokenMinter + Send + Sync>, IdentityRunRevoker), AgentHostError> {
        let id = &self.identity;
        let scope = TenantScope::from_verified_token(&task.agent, self.region.clone());
        let now = timestamp_from_epoch(task.now_secs);
        let resolved_policy = bridge(
            &self.runtime,
            id.policies.resolve_for_run(
                &scope,
                &task.agent,
                &task.trigger_actor,
                &myelin_identity::RunId(task.run_id.clone()),
            ),
        )
        .map_err(|error| AgentHostError::Identity(error.to_string()))?;
        let minter: Arc<dyn RunTokenMinter + Send + Sync> = Arc::new(IdentityRunMinter::new(
            id.minter.clone(),
            scope.clone(),
            task.agent.clone(),
            task.trigger_actor.clone(),
            resolved_policy,
            now,
        ));
        Ok((
            minter,
            IdentityRunRevoker::new(id.revocations.clone(), scope),
        ))
    }

    pub fn run(
        &self,
        task: &LlmRunTask,
        wiring: &mut RunSubstrateWiring<'_>,
        model_client: Box<dyn ModelClient + Send + Sync>,
        tools: Tools<'_>,
    ) -> Result<LlmRunReport, AgentHostError> {
        let (minter, revoker) = self.identity_seams(task)?;
        let model_client = Box::new(DurableModelClient::new(
            task.tenant.clone(),
            task.run_id.clone(),
            self.model_steps.clone(),
            model_client,
        ));
        dispatch_core(
            &self.wallet,
            self.region.clone(),
            task,
            wiring,
            model_client,
            tools,
            RunTokenSeams {
                minter,
                revoker: &revoker,
            },
        )
    }
}

fn bridge<F: std::future::Future>(runtime: &tokio::runtime::Handle, future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| runtime.block_on(future)),
        Err(_) => runtime.block_on(future),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent_model::{ModelReply, Usage};

    struct Spy {
        seen: Mutex<Vec<ModelRequest>>,
        answer: String,
        usage: Usage,
    }
    impl Spy {
        fn new(answer: &str, usage: Usage) -> Arc<Spy> {
            Arc::new(Spy {
                seen: Mutex::new(Vec::new()),
                answer: answer.into(),
                usage,
            })
        }
    }
    impl ModelClient for Spy {
        fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
            self.seen.lock().unwrap().push(request.clone());
            Ok(ModelResponse {
                reply: ModelReply::Final {
                    content: self.answer.clone(),
                },
                usage: self.usage,
            })
        }
    }
    struct SharedSpy(Arc<Spy>);
    impl ModelClient for SharedSpy {
        fn complete(&self, r: &ModelRequest) -> Result<ModelResponse, ModelError> {
            self.0.complete(r)
        }
    }

    #[test]
    fn host_client_injects_system_and_prompt_on_the_first_step() {
        let spy = Spy::new(
            "ready",
            Usage::Reported {
                input: 5,
                cached_input: 0,
                output: 1,
            },
        );
        let (client, answer) = HostModelClient::wrap(
            "SYS".into(),
            "do the thing".into(),
            vec![],
            Box::new(SharedSpy(spy.clone())),
        );
        let resp = client.complete(&ModelRequest::default()).unwrap();
        assert!(matches!(resp.reply, ModelReply::Final { .. }));
        assert_eq!(answer.take().as_deref(), Some("ready"));
        let seen = spy.seen.lock().unwrap()[0].clone();
        assert_eq!(seen.system, "SYS");
        match &seen.turns[..] {
            [ModelTurn::User { content }] => assert_eq!(content, "do the thing"),
            other => panic!("expected one injected user turn, got {other:?}"),
        }
    }

    #[test]
    fn empty_system_falls_back_to_the_default_framing() {
        assert!(default_system("   ").contains("labelled as an agent"));
        assert_eq!(default_system("custom"), "custom");
    }

    #[test]
    fn automated_work_keeps_its_human_trigger_and_attenuation() {
        let tenant = TenantId("acme".into());
        let agent = Principal::stub(
            myelin_identity::PrincipalId("agent:triage".into()),
            myelin_identity::PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("hosted:luna".into()),
                on_behalf_of: Some(myelin_identity::PrincipalId("founder".into())),
            },
            tenant.clone(),
        );
        let founder = Principal::stub(
            myelin_identity::PrincipalId("founder".into()),
            myelin_identity::PrincipalKind::Human,
            tenant.clone(),
        );
        let task = LlmRunTask::new(tenant, agent, "agent:triage", "run-1", "", "Fix CI")
            .with_delegation(
                founder.clone(),
                DelegationCaveats(vec!["repo:core".into(), "issue:create".into()]),
            );
        assert_eq!(task.trigger_actor, founder);
        assert_eq!(
            task.delegation_caveats.0,
            vec!["repo:core".to_string(), "issue:create".to_string()]
        );
    }

    #[test]
    fn host_client_injects_tool_specs_on_every_step() {
        let spy = Spy::new("ok", Usage::NotReported);
        let specs = vec![tool_schema_to_spec(&git_check_status_read_tool_schema())];
        let (client, _) = HostModelClient::wrap(
            "SYS".into(),
            "task".into(),
            specs,
            Box::new(SharedSpy(spy.clone())),
        );
        client.complete(&ModelRequest::default()).unwrap();
        client
            .complete(&ModelRequest {
                turns: vec![ModelTurn::User {
                    content: "prior".into(),
                }],
                ..Default::default()
            })
            .unwrap();
        let seen = spy.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        for req in seen.iter() {
            assert_eq!(req.tools.len(), 1);
            assert_eq!(req.tools[0].name, GIT_READ_CHECK_STATUS_TOOL);
        }
    }

    #[test]
    fn tool_catalogue_resolves_registered_tools() {
        let cat = ToolCatalogue::new([git_check_status_read_tool_def()]);
        assert!(cat
            .resolve(&ToolName(GIT_READ_CHECK_STATUS_TOOL.into()))
            .is_some());
        assert!(cat.resolve(&ToolName("nope".into())).is_none());
    }

    #[test]
    fn revoker_is_idempotent() {
        let r = SyntheticRunRevoker::default();
        assert!(!r.is_dead("j1", 10));
        let _ = r.revoke("j1", 10, 5);
        assert!(r.is_dead("j1", 10));
        assert_eq!(r.revoke("j1", 10, 5), 0);
    }
}
