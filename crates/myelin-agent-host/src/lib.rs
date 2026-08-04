//! Composition root for a hosted agent run: real brain → driving loop → per-turn wallet metering.
//!
//! Lives here (not in `myelin-agent-service`) because the service must never depend on the vendor
//! crate `myelin-agent-model` (the `no-llm-in-platform` boundary). This crate is a leaf that depends
//! on both, builds the real `LlmAgentRuntime`, and hands it to `handle_run` as a `&dyn MeteredRuntime`.
//!
//! Two invariants the types enforce: **F1** — a real run is always billed: the wallet is a
//! non-optional `&dyn RunWallet` and the one `RunSubstrate` site sets `wallet: Some(..)`. **F2** — the
//! wallet's region is the run's region: `AgentHost` derives both from one `SubstrateProvider`.

use std::sync::{Arc, Mutex};

pub mod git_read_tool;
pub use git_read_tool::{
    git_check_status_read_tool_def, git_check_status_read_tool_schema, GitCheckStatusReadExecutor,
    GIT_READ_CHECK_STATUS_TOOL,
};

pub mod identity;
pub use identity::timestamp_from_epoch;
use identity::{IdentityRunMinter, IdentityRunRevoker};

use myelin_agent::{MeteredRuntime, ToolCall, ToolDef, ToolName, ToolResult, ToolSchema, ToolSurface};
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
use myelin_storage::{
    DurableCellRootBacking, DurableRevocationBacking, SealKey, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use myelin_identity_service::{
    CellTokenAuthority, PasetoCapabilitySigner, RevocationStore,
    RunTokenMinter as IdentityRunTokenMinter,
};

pub use myelin_agent_service::{RunTokenRevoker, RunWallet};
pub use myelin_storage::agent_wallet::{CreditKind, MicroUsd, WalletError};

// ── the run description + result ──────────────────────────────────────────────────────────────────

/// One run's inputs, minus the region (supplied by the composition root so it can't disagree with the
/// wallet's — F2). Reserve/lifecycle knobs default via [`LlmRunTask::new`].
#[derive(Clone, Debug)]
pub struct LlmRunTask {
    pub tenant: TenantId,
    pub agent: Principal,
    pub agent_id: String,
    pub run_id: String,
    /// System framing (the task is folded into system context; empty ⇒ a default agent-labelled one).
    pub system: String,
    /// The task, injected as the opening user message.
    pub prompt: String,
    pub token_ttl_secs: u64,
    /// Nominal reserve estimate for the reserve/settle gate — distinct from the real micro-dollar
    /// wallet debit.
    pub estimate: MicroUsd,
    pub now_secs: i64,
    /// Nominal available-balance floor, retained for the no-wallet reserve shape. The REAL hosted-run
    /// path (`dispatch_core`) IGNORES this and reads the live wallet balance minus the tenant's
    /// outstanding reservations instead (unified-wallet slice 3) — so a hosted run's affordability gate
    /// is the actual prepaid balance, not this constant.
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
        LlmRunTask {
            tenant,
            agent,
            agent_id: agent_id.into(),
            run_id: run_id.into(),
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
}

/// The outcome of a metered run: the loop outcome, the model's final answer, total micro-dollars
/// debited, and the survival-signal telemetry (`reserved == settled`, trace, revocation lag).
#[derive(Clone, Debug)]
pub struct LlmRunReport {
    pub outcome: myelin_agent::RunOutcome,
    pub answer: String,
    pub charged_micro: u64,
    pub telemetry: SkeletonTelemetry,
}

/// A run that refused or aborted (a failed mint, a spend-cap halt, an unmetered turn, …). The token
/// is always torn down and consumed turns are always billed.
#[derive(Clone, Debug)]
pub enum AgentHostError {
    Run(SkeletonError),
}

impl core::fmt::Display for AgentHostError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AgentHostError::Run(e) => write!(f, "hosted-agent run failed: {e}"),
        }
    }
}

impl std::error::Error for AgentHostError {}

impl From<SkeletonError> for AgentHostError {
    fn from(e: SkeletonError) -> AgentHostError {
        AgentHostError::Run(e)
    }
}

/// The durable-or-in-memory `RunSubstrate` pieces the caller wires: the reserve/settle ledger, the
/// outbox the trace co-commits into, the id minter, the workflow journal. The wallet is *not* here —
/// it is threaded separately and non-optionally (F1).
pub struct RunSubstrateWiring<'a> {
    pub ledger: &'a mut CostLedger,
    pub outbox: &'a OutboxStore,
    pub id_minter: Arc<dyn IdMinter>,
    pub journal: WfJournal,
}

/// The tools a run offers, or [`Tools::none`] for a tool-less run.
pub struct Tools<'a> {
    /// The permissioned catalogue the loop's `validate_call` resolves each call against.
    pub catalogue: &'a dyn ToolSurface,
    /// Runs each validated call.
    pub executor: &'a dyn ToolExecutor,
    /// What the model is told it may call (the loop leaves `conv.tools` empty, so specs are injected
    /// at the vendor seam — the AG-P7 push-down floor).
    pub advertised: &'a [ToolSchema],
}

impl<'a> Tools<'a> {
    /// A tool-less run: byte-identical to before tools existed.
    pub fn none() -> Tools<'static> {
        Tools {
            catalogue: &NoToolSurface,
            executor: &NoToolExecutor,
            advertised: &[],
        }
    }
}

// ── ModelClient wrapper: inject the task, capture the answer ───────────────────────────────────────

/// A handle to the answer [`HostModelClient`] captures, cloned out before the wrapper is boxed.
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

/// Wraps the vendor client to (a) inject the run's system + task and (b) capture the final answer —
/// because `handle_run` drives an empty conversation and discards the submission. Neither `handle_run`
/// nor the runtime is touched; usage flows through verbatim so metering stays faithful.
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
        // Inject framing + task only on the first step (later tool-loop steps already carry history).
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
        // Advertise tools on every step (the model needs them on the tool-result turn too).
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

// ── in-process token + no-tools seams (the hermetic/dev floor; real Identity is in `AgentHost`) ─────

/// Deterministic in-process token minter — the attribution floor for hermetic drills.
#[derive(Default)]
pub struct HostRunMinter;

impl RunTokenMinter for HostRunMinter {
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

/// In-process revoker — idempotent (a re-revoke is a no-op success).
#[derive(Default)]
pub struct HostRunRevoker {
    revoked: Mutex<std::collections::HashSet<String>>,
}

impl RunTokenRevoker for HostRunRevoker {
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

/// Empty catalogue for a no-tools run.
#[derive(Default)]
pub struct NoToolSurface;

impl ToolSurface for NoToolSurface {
    fn register_tool(&mut self, _def: ToolDef) {}
    fn resolve(&self, _name: &ToolName) -> Option<&ToolDef> {
        None
    }
}

/// No-op executor for a no-tools run — fails loud if ever reached (that would be a bug).
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

/// A real, `Vec`-backed catalogue a tools-enabled run validates calls against.
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

/// Model-facing [`ToolSchema`] → the vendor [`ToolSpec`] sent to the model (permissive object schema
/// if the schema string is empty/unparseable, so the request stays wire-valid).
fn tool_schema_to_spec(schema: &ToolSchema) -> ToolSpec {
    ToolSpec {
        name: schema.name.0.clone(),
        description: schema.description.clone(),
        input_schema: serde_json::from_str(&schema.input_schema)
            .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
    }
}

// ── cap enforcement: the ReBAC gate before a governed tool executes ─────────────────────────────────

/// Derives the ReBAC resource a tool call targets from its *validated* arguments (the *what*; the
/// *whose* is the verified token). `None` ⇒ the resource can't be derived ⇒ deny fail-closed.
type ToolResourceResolver = dyn Fn(&ToolDef, &ToolCall) -> Option<ArtifactRef> + Send + Sync;

/// The `git.read_check_status` resource is its `repo` argument.
pub fn git_read_check_status_resource(_def: &ToolDef, call: &ToolCall) -> Option<ArtifactRef> {
    call.arguments
        .get("repo")
        .and_then(|v| v.as_str())
        .map(|repo| ArtifactRef(repo.to_string()))
}

/// Strong consistency at latest — a tool authorization is never served from the fail-static cache.
fn strong_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

/// A [`ToolExecutor`] decorator that denies a governed call unless the run's principal holds the
/// tool's `required_caps` on the target resource, checked on the real ReBAC engine. Fail-closed on
/// anything but `Allow` (Deny, Conditional, check error, underivable resource); the inner executor is
/// never reached on a denied cap. An empty `required_caps` set dispatches straight through.
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

    /// For the `git.read_check_status` tool (resource = its `repo` argument).
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
            // Does the verified principal hold `cap` on `resource`? The check scopes (tenant, region)
            // from the subject's token and authorizes an agent exactly as a human.
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

// ── the one metered dispatcher + the two entry points ───────────────────────────────────────────────

/// The per-run token seams the loop composes the mint → run → revoke lifecycle over. The one
/// dispatcher is generic over these, so the in-process floor and real Identity drive the same path.
struct RunTokenSeams<'a> {
    minter: Arc<dyn RunTokenMinter + Send + Sync>,
    revoker: &'a dyn RunTokenRevoker,
}

/// Dispatch a metered run over the in-process token floor (what hermetic drills call directly). Real
/// runs go through [`AgentHost`], which supplies real Identity seams. `region` must be the wallet's
/// region (F2 — prefer `AgentHost`).
pub fn dispatch_metered_llm_run(
    wallet: &dyn RunWallet,
    region: Region,
    task: &LlmRunTask,
    wiring: &mut RunSubstrateWiring<'_>,
    model_client: Box<dyn ModelClient + Send + Sync>,
    tools: Tools<'_>,
) -> Result<LlmRunReport, AgentHostError> {
    let revoker = HostRunRevoker::default();
    dispatch_core(
        wallet,
        region,
        task,
        wiring,
        model_client,
        tools,
        RunTokenSeams {
            minter: Arc::new(HostRunMinter),
            revoker: &revoker,
        },
    )
}

/// The one `RunSubstrate` construction site (F1: `wallet: Some(..)`). Wraps the vendor client (task
/// inject + answer capture), assembles the substrate over the caller's token `seams`, drives
/// `handle_run`. A failed mint aborts before reserve (fail-closed).
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

    // ── REAL "no balance → no run": the reserve gate reads the ACTUAL prepaid wallet balance ────────
    // The affordability gate's `available` is the tenant's live wallet balance MINUS the amount already
    // committed to its unsettled reservations (`Reserved`/`InFlight`), saturating at 0. This makes the
    // gate a real "can't afford `estimate` → can't dispatch" check, and makes it concurrency-correct: a
    // second run reserving against the same balance sees `available` reduced by the first run's
    // outstanding amount, so two runs cannot over-reserve past one balance. Both amounts are `MicroUsd`
    // (unified money type) — no unit bridge, just a saturating subtract.
    //
    // `dispatch_core` ALWAYS has a real wallet (F1). The no-wallet skeleton/mock/drill paths build
    // `RunSubstrate` directly with `wallet: None` and their own nominal `available` — they never reach
    // here, so those runs stay BYTE-IDENTICAL (the reserved==settled drills are unaffected). The nominal
    // `task.available` literal is retained only for those no-wallet shapes.
    //
    // ATOMICITY NOTE (v1 — documented, not built): reading `balance` + `outstanding` then reserving is
    // three ops, NOT one atomic transaction. Under N-way concurrency (N dispatchers reading the same
    // pre-reserve `outstanding` snapshot) the over-admission is bounded by (N−1)×`estimate`. This has
    // NO overspend impact: over-admission only inflates reservations; the atomic `wallet.debit`
    // (`FOR UPDATE` + checked_sub, refuses on insufficient) is the real money backstop, so an
    // over-admitted run simply halts at its debit — the balance can never go negative. The fully-atomic
    // reserve-against-(balance − outstanding) in one cross-ledger transaction is a named follow-on
    // (unified-wallet slice 4+).
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
        caveats: DelegationCaveats(vec![]),
        token_ttl_secs: task.token_ttl_secs,
        revoker: seams.revoker,
        catalogue: tools.catalogue,
        executor: tools.executor,
        wallet: Some(wallet), // F1
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

/// The default agent-labelled system framing (agents are never disguised as human).
fn default_system(system: &str) -> String {
    if system.trim().is_empty() {
        "You are a hosted agent. You are labelled as an agent. Answer the user's request \
         concisely and directly."
            .to_string()
    } else {
        system.to_string()
    }
}

// ── AgentHost: the F2-airtight front door ───────────────────────────────────────────────────────────

/// The hosted-agent front door. Owns the durable wallet + region, derived from one provider so they
/// can't disagree (F2). Build inside a Tokio multi-thread runtime (the wallet's per-turn debit bridges
/// sync→async via `block_in_place`).
pub struct AgentHost {
    region: Region,
    wallet: AgentWallet,
    /// Real Identity mint + durable revocation — `Some` via [`AgentHost::with_identity`], `None` via
    /// [`AgentHost::new`] (the in-process floor).
    identity: Option<HostIdentity>,
}

struct HostIdentity {
    minter: IdentityRunTokenMinter,
    revocations: RevocationStore,
}

/// The real Identity providers refused to start — a bad seal key or unreachable store. A host is never
/// built with a degraded signing root (it would mint unverifiable tokens).
#[derive(Debug)]
pub enum HostIdentityError {
    CellRootUnavailable(String),
    InvalidCellRoot(String),
}

impl core::fmt::Display for HostIdentityError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HostIdentityError::CellRootUnavailable(e) => {
                write!(f, "hosted-agent Identity root refused to start (fail-closed): {e}")
            }
            HostIdentityError::InvalidCellRoot(e) => {
                write!(f, "hosted-agent Identity cell-authority material is invalid: {e}")
            }
        }
    }
}

impl std::error::Error for HostIdentityError {}

impl AgentHost {
    /// Host over the in-process token floor (hermetic/dev — no DB, no crypto). For a real multi-tenant
    /// agent use [`AgentHost::with_identity`].
    pub fn new(provider: SubstrateProvider) -> AgentHost {
        let region = Region(provider.config().region.clone());
        AgentHost {
            region,
            wallet: AgentWallet::new(provider),
            identity: None,
        }
    }

    /// Production host wired to real per-run token mint + durable S7 revocation. The signing root is
    /// recovered from `cell_root` under `seal_key` (the same path edge/CI use — never a fresh root that
    /// orphans tokens); the S7 denylist is PG-backed, RLS-scoped, crash-safe. Fail-closed here, before
    /// any run, if the root won't unseal or the store is unreachable. Needs the cell-root + identity
    /// migrations applied.
    pub async fn with_identity(
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
            RevocationStore::with_pg(DurableRevocationBacking::new(provider.clone()), rt);
        let signer = Arc::new(PasetoCapabilitySigner::new(cell));
        let minter = IdentityRunTokenMinter::with_signer_and_tuples(revocations.clone(), None, signer);
        Ok(AgentHost {
            region,
            wallet: AgentWallet::new(provider),
            identity: Some(HostIdentity {
                minter,
                revocations,
            }),
        })
    }

    pub fn wallet(&self) -> &AgentWallet {
        &self.wallet
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The durable S7 revocation store — `Some` only on a real-Identity host. Lets a drill assert a
    /// run's token was really revoked (and durably so).
    pub fn revocations(&self) -> Option<&RevocationStore> {
        self.identity.as_ref().map(|id| &id.revocations)
    }

    /// The run's real Identity seams, bound to its verified scope + agent principal, or `None` on an
    /// in-process-floor host. The returned revoker is a local the caller keeps alive for the borrow.
    fn identity_seams(
        &self,
        task: &LlmRunTask,
    ) -> Option<(Arc<dyn RunTokenMinter + Send + Sync>, IdentityRunRevoker)> {
        let id = self.identity.as_ref()?;
        let scope = TenantScope::from_verified_token(&task.agent, self.region.clone());
        let now = timestamp_from_epoch(task.now_secs);
        let minter: Arc<dyn RunTokenMinter + Send + Sync> = Arc::new(IdentityRunMinter::new(
            id.minter.clone(),
            scope.clone(),
            task.agent.clone(),
            task.agent.clone(),
            now,
        ));
        Some((minter, IdentityRunRevoker::new(id.revocations.clone(), scope)))
    }

    /// Drive one metered run over this host's wallet (F1) + region (F2), minting a real per-run token
    /// when built with identity else the in-process floor. Pass `Tools::none()` for a reasoning-only
    /// run, or a real catalogue/executor to let it act. Brain: `Box::new(LunaClient::from_env()?)`.
    pub fn run(
        &self,
        task: &LlmRunTask,
        wiring: &mut RunSubstrateWiring<'_>,
        model_client: Box<dyn ModelClient + Send + Sync>,
        tools: Tools<'_>,
    ) -> Result<LlmRunReport, AgentHostError> {
        match self.identity_seams(task) {
            Some((minter, revoker)) => dispatch_core(
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
            ),
            None => dispatch_metered_llm_run(
                &self.wallet,
                self.region.clone(),
                task,
                wiring,
                model_client,
                tools,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent_model::{ModelReply, Usage};

    /// An inner client that records every request it's handed and returns a fixed final answer.
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
    /// Forwards to a shared `Spy` (the orphan rule forbids `impl ModelClient for Arc<Spy>`).
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
        let (client, answer) =
            HostModelClient::wrap("SYS".into(), "do the thing".into(), vec![], Box::new(SharedSpy(spy.clone())));
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
    fn host_client_injects_tool_specs_on_every_step() {
        let spy = Spy::new("ok", Usage::NotReported);
        let specs = vec![tool_schema_to_spec(&git_check_status_read_tool_schema())];
        let (client, _) =
            HostModelClient::wrap("SYS".into(), "task".into(), specs, Box::new(SharedSpy(spy.clone())));
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
        assert!(cat.resolve(&ToolName(GIT_READ_CHECK_STATUS_TOOL.into())).is_some());
        assert!(cat.resolve(&ToolName("nope".into())).is_none());
    }

    #[test]
    fn revoker_is_idempotent() {
        let r = HostRunRevoker::default();
        assert!(!r.is_dead("j1", 10));
        let _ = r.revoke("j1", 10, 5);
        assert!(r.is_dead("j1", 10));
        assert_eq!(r.revoke("j1", 10, 5), 0);
    }
}
