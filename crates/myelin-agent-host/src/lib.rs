//! # `myelin-agent-host` — the hosted-agent COMPOSITION ROOT (real brain → loop → metering → answer).
//!
//! This crate is the ONE place the three otherwise-separated pieces of a hosted agent run are
//! composed into a **functioning, metered, end-to-end run**:
//!
//! 1. the **real brain** — [`myelin_agent_model::LlmAgentRuntime`] over a
//!    [`myelin_agent_model::LunaClient`] (the sanctioned vendor SDK home);
//! 2. the **platform-owned driving loop** — [`myelin_agent_service::SkeletonAgent::handle_run`],
//!    which drives the bounded, metered mint → reserve → step → trace → settle → revoke chain; and
//! 3. the **durable prepaid wallet** — [`myelin_storage::agent_wallet::AgentWallet`], debited
//!    per-turn for the priced token usage.
//!
//! ## Why this crate exists — the `no-llm-in-platform` boundary (contract 1.6)
//! `myelin-agent-service` must NEVER depend on `myelin-agent-model` (the platform stays
//! provider-agnostic behind the [`AgentRuntime`](myelin_agent::AgentRuntime) strategy seam). So the
//! real [`LlmAgentRuntime`](myelin_agent_model::LlmAgentRuntime) cannot be constructed inside the
//! service — it must be built at a composition root that depends on BOTH crates and handed to
//! `handle_run` as a `&dyn `[`MeteredRuntime`](myelin_agent::MeteredRuntime). This crate is that
//! root, and it is a LEAF (nothing depends on it), so the `myelin-agent-model` edge lives in exactly
//! one place and can never close a dependency cycle.
//!
//! ## F1 (the money guard) — a real paid run can NEVER be dispatched unbilled
//! The single [`RunSubstrate`](myelin_agent_service::RunSubstrate) builder in this crate
//! ([`dispatch_metered_llm_run`]) takes a **non-optional** wallet (`&dyn `[`RunWallet`]) and
//! **always** sets `wallet: Some(..)`. There is no code path — public or private — that dispatches a
//! real [`LlmAgentRuntime`](myelin_agent_model::LlmAgentRuntime) run with `wallet: None`. The guard
//! is STRUCTURAL: the type carries no `None`, and the one construction site hard-codes `Some`. (The
//! reserve/settle nominal gate is a separate nominal layer; the wallet is the real per-turn billing
//! LAYERED ON TOP — see [`myelin_agent_service::skeleton`].)
//!
//! ## F2 (region correctness) — the wallet's region must be the run's region
//! [`AgentWallet`] resolves its region from its own instance (the provider it was built over), so the
//! instance MUST be built for the run's region. [`AgentHost`] makes this airtight: it derives BOTH
//! the wallet and the [`Region`] from ONE [`SubstrateProvider`](myelin_storage::SubstrateProvider),
//! so they cannot disagree. The lower-level [`run_llm_agent`] / [`dispatch_metered_llm_run`] take the
//! region as a parameter and document the invariant (the caller builds the wallet for that region).
//!
//! ## The task, the answer, and the metering
//! `handle_run` drives an EMPTY [`Conversation`](myelin_agent::Conversation) and discards the final
//! submission (it keeps only the trace ref). To make a real, useful run we therefore work at the
//! [`ModelClient`](myelin_agent_model::ModelClient) seam via a transparent [`HostModelClient`]
//! wrapper that (a) injects the run's system framing + task prompt into the request, and (b) captures
//! the model's final text answer — WITHOUT modifying `handle_run` or the runtime. The per-turn wallet
//! debit and the balanced reserve/settle ledger are the service's; this crate only composes them.

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
use myelin_flow::{
    DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter, WfJournal,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal, Zookie,
};
use myelin_storage::agent_wallet::AgentWallet;
use myelin_storage::reserve_settle::{CostLedger, MinorUnits};
use myelin_storage::{
    DurableCellRootBacking, DurableRevocationBacking, SealKey, SubstrateProvider, TenantScope,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

// The REAL Identity per-run token providers the composition root wires behind the mint/revoke seams.
use myelin_identity_service::{
    CellTokenAuthority, PasetoCapabilitySigner, RevocationStore,
    RunTokenMinter as IdentityRunTokenMinter,
};

// Re-export the seams a caller/drill composes against without a second import of the underlying
// crate: the wallet metering seam + the teardown seam, the durable wallet's unit + kinds + error.
pub use myelin_agent_service::{RunTokenRevoker, RunWallet};
pub use myelin_storage::agent_wallet::{CreditKind, MicroUsd, WalletError};

// ───────────────────────── the run description (region-free) + the report ───────────────────────

/// **The inputs of one hosted-agent run, WITHOUT the region.** The region is supplied by the
/// composition root ([`AgentHost`] derives it from the wallet's provider; [`run_llm_agent`] takes it
/// as a parameter) so a caller can never accidentally run under a region the wallet is not scoped to
/// (F2). Construct with [`LlmRunTask::new`] and override the reserve/lifecycle knobs as needed.
#[derive(Clone, Debug)]
pub struct LlmRunTask {
    /// The verified tenant the run executes for (tenant-from-token; never a path).
    pub tenant: TenantId,
    /// The agent principal the run acts as (a `Principal` with `kind = agent`).
    pub agent: Principal,
    /// The agent principal id the per-run token is minted for.
    pub agent_id: String,
    /// The durable-workflow run id.
    pub run_id: String,
    /// The system framing injected as the model's system message (Tier-0: the task is folded into
    /// the system context; see [`myelin_agent_model`]). Empty is allowed — a default framing is used.
    pub system: String,
    /// The task the agent is asked to do — injected as the opening user message of the conversation.
    pub prompt: String,
    /// The per-run token TTL bound, in seconds (token life == run life).
    pub token_ttl_secs: u64,
    /// The run's NOMINAL reserve estimate (cents / minor-units) for the reserve/settle gate. This is
    /// the nominal cost layer, DISTINCT from the micro-dollar wallet debit (the real per-turn bill).
    pub estimate: MinorUnits,
    /// The NOMINAL available balance the reserve gate reads (cents / minor-units). Nominal only.
    pub now_secs: i64,
    /// The nominal available balance the reserve gate reads (cents / minor-units). Nominal only.
    pub available: MinorUnits,
    /// A per-call output-token ceiling that bounds single-call overshoot (`None` ⇒ provider default).
    pub max_output_tokens: Option<u32>,
}

impl LlmRunTask {
    /// A run task with sensible reserve/lifecycle defaults (300s token TTL, a small nominal reserve).
    /// The `system` framing + `prompt` are the caller's; everything else can be tuned via the fields.
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
            estimate: MinorUnits(10),
            available: MinorUnits(100),
            now_secs: 0,
            max_output_tokens: None,
        }
    }

    /// Set the per-call output-token ceiling (builder-style).
    pub fn with_max_output_tokens(mut self, max: u32) -> LlmRunTask {
        self.max_output_tokens = Some(max);
        self
    }

    /// Set the engine clock the lease/revocation reads (builder-style).
    pub fn with_now_secs(mut self, now_secs: i64) -> LlmRunTask {
        self.now_secs = now_secs;
        self
    }
}

/// **The result of a metered hosted-agent run.** Carries the platform loop outcome, the model's real
/// final text answer (captured at the [`ModelClient`](myelin_agent_model::ModelClient) seam), the
/// total micro-dollars debited from the wallet across the run's turns, and the full survival-signal
/// telemetry (the balanced reserve/settle ledger, the trace, the token revocation).
#[derive(Clone, Debug)]
pub struct LlmRunReport {
    /// The platform-owned loop outcome (contract 8.5) — the trace ref + settle summary as a machine
    /// string (references-not-payloads).
    pub outcome: myelin_agent::RunOutcome,
    /// **The model's final text answer** (the real Luna answer on a live run). Empty only if the
    /// model produced no final content.
    pub answer: String,
    /// **The total micro-dollars DEBITED from the wallet across this run** (`Σ` of each turn's priced
    /// `wholesale + markup`). A metered run with a real reported-usage brain is `> 0`.
    pub charged_micro: u64,
    /// The contract-1.8 survival signals (`reserved == settled`, the trace, the revocation lag, the
    /// raw token totals).
    pub telemetry: SkeletonTelemetry,
}

/// **An error composing or driving a hosted-agent run.** Wraps the service's loud
/// [`SkeletonError`](myelin_agent_service::SkeletonError) (a refused dispatch, a failed mint, a
/// spend-cap halt, a fail-closed unmetered turn, an arithmetic overflow, …). A real paid run that
/// fails NEVER leaves an unbilled consumed turn — the wallet debit stands for consumed turns and the
/// teardown always fires (see [`myelin_agent_service::skeleton`]).
#[derive(Clone, Debug)]
pub enum AgentHostError {
    /// The driving loop refused/aborted the run (surfaced loud; the token was still torn down).
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

// ───────────────────────── the nominal substrate the caller provides ─────────────────────────────

/// **The NOMINAL substrate pieces the caller wires (the durable-or-in-memory parts of the
/// `RunSubstrate` this crate cannot conjure feature-cleanly).** The reserve/settle ledger, the
/// transactional outbox, the id-minter, and the workflow journal. In production these are the durable
/// backings ([`CostLedger::with_pg`](myelin_storage::reserve_settle::CostLedger::with_pg), a durable
/// [`OutboxStore`]); in a hermetic drill they are the in-memory doubles. The wallet is NOT here — it
/// is the money, threaded separately and NON-OPTIONALLY (F1).
///
/// The per-run identity (token mint/revoke) and the no-tools catalogue/executor are provided by this
/// crate ([`HostRunMinter`] / [`HostRunRevoker`] / [`NoToolSurface`] / [`NoToolExecutor`]) — the
/// v1 in-process attribution floor; wiring the real Identity mint/revoke provider is a named
/// follow-on (it slots behind the same seams).
pub struct RunSubstrateWiring<'a> {
    /// The reserve/settle cost ledger (11.7) — durable in production, in-memory in a drill.
    pub ledger: &'a mut CostLedger,
    /// The transactional outbox the trace activity co-commits its terminal emit into.
    pub outbox: &'a OutboxStore,
    /// The ULID minter the outbox stamps emitted event ids with.
    pub id_minter: Arc<dyn IdMinter>,
    /// The `wf_history` / `wf_activity_attempt` journal the trace activity writes (9.2).
    pub journal: WfJournal,
}

// ───────────────────────── the ModelClient wrapper: inject the task + capture the answer ─────────

/// A shared handle to the final answer the [`HostModelClient`] captured. Cloned before the wrapper is
/// boxed into the runtime, so the answer is readable after the run drives to completion.
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

/// **A transparent [`ModelClient`](myelin_agent_model::ModelClient) wrapper that (a) injects the run's
/// system framing + opening task, and (b) captures the model's final answer.**
///
/// [`handle_run`](myelin_agent_service::SkeletonAgent::handle_run) drives an EMPTY conversation and
/// throws away the submission, so a run driven straight through it would send the model no task and
/// return no answer. This wrapper closes both gaps at the vendor seam — the layer BELOW the runtime —
/// so neither `handle_run` nor [`LlmAgentRuntime`](myelin_agent_model::LlmAgentRuntime) is touched:
/// - on the FIRST step (the request carries no prior turns) it fills an empty `system` with the
///   configured framing and prepends the task as the opening user message;
/// - it passes the request through to the inner client UNCHANGED otherwise (usage flows through
///   verbatim, so the metering is faithful), and records the content of any final answer.
struct HostModelClient {
    system: String,
    prompt: String,
    /// The vendor tool specs the run offers the model (name + description + parsed input schema). The
    /// platform loop drives an EMPTY [`Conversation`](myelin_agent::Conversation) — it does NOT yet
    /// populate `conv.tools` from the catalogue (the delegation-scoped tool-list push-down is the
    /// named AG-P7 floor). So the run's tools reach the model HERE, at the same vendor seam that
    /// injects the system + task — on EVERY step (the model needs the tool definitions on the
    /// tool-result turn too), WITHOUT touching `handle_run` or the runtime. Empty ⇒ a tool-less run
    /// (byte-identical to before).
    tool_specs: Vec<ToolSpec>,
    inner: Box<dyn ModelClient + Send + Sync>,
    answer: AnswerSlot,
}

impl HostModelClient {
    /// Wrap `inner`, returning the wrapper and a handle to read the captured answer after the run.
    /// `tool_specs` is the model-facing tool catalogue this run offers (empty ⇒ a tool-less run).
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
        // Inject the framing + the task on the first step (no prior turns yet). On later tool-loop
        // steps the request already carries history — leave it untouched so linkage is preserved.
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
        // Inject the run's tool specs on EVERY step so the model knows what it may call (the loop
        // leaves `conv.tools` empty). A tool-less run carries no specs and this is a no-op.
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

// ───────────────────────── the v1 in-process per-run identity + no-tools seams ───────────────────

/// **The v1 in-process per-run token minter (the attribution FLOOR).** Mints a deterministic
/// per-run handle so the run is attributed and torn down. The real Identity `mint_run_token` provider
/// (contract 4.7) slots behind this SAME [`RunTokenMinter`] seam in a named follow-on; the money and
/// brain composition this crate proves does not change when it lands.
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

/// **The v1 in-process per-run token revoker (the teardown FLOOR).** Records revocations so the
/// teardown is idempotent (a re-revoke is a no-op success). The real Identity revocation store slots
/// behind this SAME [`RunTokenRevoker`] seam in a named follow-on.
#[derive(Default)]
pub struct HostRunRevoker {
    revoked: Mutex<std::collections::HashSet<String>>,
}

impl RunTokenRevoker for HostRunRevoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        let mut g = self.revoked.lock().expect("revoker lock");
        if !g.insert(jti.to_string()) {
            return 0; // idempotent re-revoke.
        }
        (now_secs - teardown_secs).max(0) as u64
    }
    fn is_dead(&self, jti: &str, _now_secs: i64) -> bool {
        self.revoked.lock().expect("revoker lock").contains(jti)
    }
}

/// **The empty tool catalogue for a NO-TOOLS run.** The brain answers directly (no `UseTools`), so the
/// catalogue is never consulted; it resolves nothing by construction. A tool-driving run replaces this
/// with the real permissioned [`ToolSurface`].
#[derive(Default)]
pub struct NoToolSurface;

impl ToolSurface for NoToolSurface {
    fn register_tool(&mut self, _def: ToolDef) {
        // A no-tools run registers nothing; this is intentionally inert.
    }
    fn resolve(&self, _name: &ToolName) -> Option<&ToolDef> {
        None
    }
}

/// **The no-op tool executor for a NO-TOOLS run.** Never called (the brain submits without tools). If
/// it ever is, it fails LOUD rather than silently returning a fake result — a no-tools run that
/// somehow reached execution is a bug, not a success.
#[derive(Default)]
pub struct NoToolExecutor;

impl ToolExecutor for NoToolExecutor {
    fn execute(&self, def: &ToolDef, _call: &ToolCall) -> Result<ToolResult, ToolExecError> {
        Err(ToolExecError::Failed(format!(
            "no-tools hosted run attempted to execute `{}` — this run registers no tools (bug)",
            def.name.0
        )))
    }
}

// ───────────────────────── the production tool catalogue (a real ToolSurface) ─────────────────────

/// **A production, `Vec`-backed [`ToolSurface`] — the real permissioned catalogue a tools-enabled run
/// validates each proposed [`ToolCall`](myelin_agent::ToolCall) against.** Replaces [`NoToolSurface`]
/// for a run that offers tools: register the run's [`ToolDef`]s, and the driving loop's `validate_call`
/// security checkpoint resolves each call against them (an unregistered tool, or arguments that fail
/// the tool's schema, abort the run fail-closed BEFORE the executor is reached). Unlike
/// [`NoToolSurface`] this is NOT test-gated — it is the real catalogue a hosted run uses.
#[derive(Clone, Debug, Default)]
pub struct ToolCatalogue {
    defs: Vec<ToolDef>,
}

impl ToolCatalogue {
    /// A catalogue seeded with the tools this run may call.
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

/// Convert a model-facing [`ToolSchema`] (name + description + JSON-schema STRING) into the vendor
/// [`ToolSpec`] the client sends the model. The schema string is parsed into the normalized object
/// carrier, falling back to a permissive object schema if it is empty/unparseable so the request
/// stays wire-valid (mirrors the runtime's own `build_request` fallback).
fn tool_schema_to_spec(schema: &ToolSchema) -> ToolSpec {
    ToolSpec {
        name: schema.name.0.clone(),
        description: schema.description.clone(),
        input_schema: serde_json::from_str(&schema.input_schema)
            .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
    }
}

// ───────────────────────── cap enforcement: the ReBAC gate BEFORE a governed tool executes ───────

/// A resolver that derives the ReBAC **resource** ([`ArtifactRef`]) a governed tool call targets from
/// its already-VALIDATED arguments. Held by [`CapEnforcingExecutor`] and invoked ONCE per declared
/// `required_cap`. Returning `None` means the resource cannot be derived — the enforcer then DENIES
/// the call fail-closed (never guesses, never falls open). The resolver reads ONLY the tool's own
/// arguments (the *what*); the *whose* (principal + tenant) is the run's verified token, never a
/// model argument.
type ToolResourceResolver = dyn Fn(&ToolDef, &ToolCall) -> Option<ArtifactRef> + Send + Sync;

/// **Derive the `git.read_check_status` resource: the `repo` argument.** The tool's `required_caps =
/// ["pull"]` are checked against the ReBAC grant on THIS repo — the resource is the `repo` ref the
/// validated arguments carry (e.g. `myelin://<tenant>/git/repo/<id>`, which the ReBAC engine
/// canonicalises to the `repo:<id>` tuple key). `None` if the argument is absent/non-string (the
/// enforcer denies fail-closed) — though `validate_call` has already proven `repo` is a required
/// string, so on the validated path this always resolves.
pub fn git_read_check_status_resource(_def: &ToolDef, call: &ToolCall) -> Option<ArtifactRef> {
    call.arguments
        .get("repo")
        .and_then(|v| v.as_str())
        .map(|repo| ArtifactRef(repo.to_string()))
}

/// The consistency token the cap gate reads under: **Strong at the latest snapshot** — the freshest
/// authoritative grant, bypassing the fail-static availability cache (a tool authorization must never
/// be served stale). An empty `at_least` zookie means "latest" (include every written grant).
fn strong_latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::Strong,
    }
}

/// **The cap-enforcement checkpoint — a [`ToolExecutor`] decorator that DENIES a governed tool call
/// fail-closed unless the run's agent principal actually HOLDS the tool's `required_caps` on the
/// target resource, checked on the REAL ReBAC engine ([`IdentityService::check`]).**
///
/// A [`ToolDef`]'s `required_caps` (e.g. the git read tool's `["pull"]`) were declarative-but-
/// UNENFORCED: the loop `validate_call`d the untrusted arguments (schema) and dispatched to the
/// executor, but nothing checked that the principal was *authorized* to call the tool — only RLS/
/// tenant scoping gated the underlying read. This decorator closes that gap. It sits at the SAME
/// dispatch boundary as `validate_call` (the loop calls `executor.execute(def, call)` only after a
/// call passes validation), and BEFORE the inner executor runs it consults the real ReBAC decision
/// engine for every declared cap:
///
/// - the **principal** is the run's verified agent [`Principal`] (from the minted token, never model
///   input);
/// - the **permission** is each `required_cap` (a git `pull`, …);
/// - the **resource** is derived from the VALIDATED arguments by [`ToolResourceResolver`] (the git
///   read tool's `repo`) — the *what*, never the *whose*;
/// - the **decision** is [`IdentityService::check`]'s, which derives the `(tenant, region)` scope from
///   the subject's own verified token internally (tenant-from-token, ID-3) and authorizes an agent the
///   SAME fail-closed way as a human (it never branches on principal kind).
///
/// **Fail-CLOSED everywhere:** any cap that is not an explicit [`Decision::Allow`] — a `Deny`, a
/// `Conditional`, a check `Err`, or a resource that cannot be derived — returns an
/// [`Err(ToolExecError)`], which the driving loop turns into an aborted run WITH teardown (the exact
/// path a `validate_call` rejection takes). The inner executor is NEVER reached on a denied cap. A
/// tool with an empty `required_caps` set is dispatched straight through (nothing to check).
///
/// It borrows the inner `&dyn `[`ToolExecutor`] for the run, so the caller keeps ownership and can
/// still inspect it afterwards (e.g. `GitCheckStatusReadExecutor::invocations`).
pub struct CapEnforcingExecutor<'a> {
    /// The REAL ReBAC decision engine (a `StoreBackedCheck` in production/drills — never a stub in the
    /// production path). Held behind `Arc<dyn IdentityService>` so the same check surface a human's
    /// request flows through gates the agent's tool call.
    identity: Arc<dyn IdentityService + Send + Sync>,
    /// The run's verified agent principal — the SUBJECT of every cap check. From the minted token, so
    /// the model can never widen *whose* authority is consulted.
    principal: Principal,
    /// The tool being executed (borrowed for the run).
    inner: &'a dyn ToolExecutor,
    /// Derives the ReBAC resource from the validated arguments (the git read tool's `repo`).
    resource_of: Box<ToolResourceResolver>,
}

impl<'a> CapEnforcingExecutor<'a> {
    /// Wrap `inner` so every governed call it would execute is first cap-checked on the REAL ReBAC
    /// engine `identity` for `principal`, with the resource derived by `resource_of`.
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

    /// Wrap `inner` for the `git.read_check_status` tool: the resource is its `repo` argument (the
    /// common case — [`CapEnforcingExecutor::new`] with [`git_read_check_status_resource`]).
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
        // Enforce EVERY declared cap on the real ReBAC engine BEFORE any side-effect-free-or-not work
        // reaches the inner executor. An empty `required_caps` set skips the loop (nothing to gate).
        for cap in &def.required_caps {
            // Derive the resource from the VALIDATED arguments (the *what*). A resource that cannot be
            // derived is genuine uncertainty about WHAT is being authorized → DENY fail-closed.
            let resource = (self.resource_of)(def, call).ok_or_else(|| {
                ToolExecError::Failed(format!(
                    "cap-enforcement DENY: could not derive the ReBAC resource for tool `{}` \
                     (required cap `{cap}`) from its arguments — fail-closed (no execute)",
                    def.name.0
                ))
            })?;

            // THE REAL REBAC CHECK: does the run's verified principal hold `cap` on `resource`? The
            // check derives the (tenant, region) scope from the subject's own token (tenant-from-token)
            // and authorizes an agent exactly as it authorizes a human (never branches on kind).
            let decision = self.identity.check(
                &self.principal,
                &Permission(cap.clone()),
                &resource,
                &strong_latest(),
                None,
            );
            match decision {
                Ok(Decision::Allow) => {} // this cap is granted — continue to the next.
                // A Deny / Conditional (a caveat the tool path does not supply) / a check Err are ALL
                // "not granted" → abort fail-closed. The loop tears the run down on this Err (the same
                // teardown path a `validate_call` rejection takes); the inner executor never runs.
                Ok(other) => {
                    return Err(ToolExecError::Failed(format!(
                        "cap-enforcement DENY: principal `{}` is not authorized for cap `{cap}` on \
                         `{}` (decision {other:?}) — tool `{}` refused fail-closed",
                        self.principal.principal_id.0,
                        resource.0,
                        def.name.0
                    )));
                }
                Err(e) => {
                    return Err(ToolExecError::Failed(format!(
                        "cap-enforcement DENY: the ReBAC check for cap `{cap}` on `{}` failed \
                         ({e:?}) — fail-closed (no execute)",
                        resource.0
                    )));
                }
            }
        }

        // Every required cap is granted (or there were none) — dispatch to the real executor.
        self.inner.execute(def, call)
    }
}

// ───────────────────────── the F1 core: the ONE metered RunSubstrate dispatcher ──────────────────

/// **The ONE place a hosted-agent [`RunSubstrate`](myelin_agent_service::RunSubstrate) is built and
/// driven — with the wallet threaded NON-OPTIONALLY (F1).**
///
/// This is the composition seam every real run funnels through. It takes a `&dyn `[`RunWallet`]
/// (there is no `None` — F1 is structural: the single `RunSubstrate` literal below hard-codes
/// `wallet: Some(wallet)`), builds the real [`LlmAgentRuntime`](myelin_agent_model::LlmAgentRuntime)
/// over a [`HostModelClient`] (task injected, answer captured), assembles the no-tools substrate, and
/// calls [`handle_run`](myelin_agent_service::SkeletonAgent::handle_run). It is generic over the
/// wallet impl so BOTH the durable [`AgentWallet`] (a real run) and a hermetic in-memory double (the
/// mock drill) drive the identical, fully-metered path.
///
/// `region` MUST be the region the wallet is scoped to (F2). Prefer [`AgentHost`], which derives both
/// from one provider so they cannot disagree.
pub fn dispatch_metered_llm_run(
    wallet: &dyn RunWallet,
    region: Region,
    task: &LlmRunTask,
    wiring: &mut RunSubstrateWiring<'_>,
    model_client: Box<dyn ModelClient + Send + Sync>,
) -> Result<LlmRunReport, AgentHostError> {
    // The TOOL-LESS path: the no-tools catalogue + executor + no advertised tools. Byte-identical to
    // the original body — it funnels through the ONE tools-aware core below with empty tool wiring.
    let catalogue = NoToolSurface;
    let executor = NoToolExecutor;
    dispatch_metered_llm_run_with_tools(
        wallet,
        region,
        task,
        wiring,
        model_client,
        &catalogue,
        &executor,
        &[],
    )
}

/// **The ONE metered `RunSubstrate` dispatcher, with the run's TOOLS wired (F1 preserved).**
///
/// The tools-enabled superset of [`dispatch_metered_llm_run`]: same F1-structural wallet threading
/// (`wallet: Some(wallet)`, always), same [`HostModelClient`] task-inject + answer-capture — PLUS a
/// real permissioned [`ToolSurface`] `catalogue` (the loop `validate_call`s each proposed call against
/// it — the security checkpoint), a [`ToolExecutor`] the loop runs each VALIDATED call through, and
/// the `advertised` [`ToolSchema`]s the model is told it may call (injected at the vendor seam because
/// the platform loop leaves `conv.tools` empty — the AG-P7 push-down floor). A tools-enabled run is
/// ADDITIVE: pass `&NoToolSurface`, `&NoToolExecutor`, `&[]` and it is byte-identical to the tool-less
/// path (that is exactly what [`dispatch_metered_llm_run`] does).
///
/// The loop already routes a `Read` tool `Direct` to the executor, appends each [`ToolResult`] into
/// the conversation, and steps again — so the run is: model turn (a tool call, metered) → executor
/// read → model turn (the answer, metered). Both turns debit the wallet (a real tool-executing run is
/// billed for the tool turn AND the answer turn). `region` MUST be the wallet's region (F2).
#[allow(clippy::too_many_arguments)]
pub fn dispatch_metered_llm_run_with_tools(
    wallet: &dyn RunWallet,
    region: Region,
    task: &LlmRunTask,
    wiring: &mut RunSubstrateWiring<'_>,
    model_client: Box<dyn ModelClient + Send + Sync>,
    catalogue: &dyn ToolSurface,
    executor: &dyn ToolExecutor,
    advertised: &[ToolSchema],
) -> Result<LlmRunReport, AgentHostError> {
    // The in-process attribution FLOOR (deterministic, DB-free): the mock/hermetic drills that call
    // this free function directly compose the token lifecycle over the in-process stubs. A real hosted
    // run goes through [`AgentHost`], which composes the SAME loop over the REAL Identity mint/revoke
    // seams ([`IdentityRunMinter`] / [`IdentityRunRevoker`]) — see [`AgentHost::run_llm_agent_with_tools`].
    let minter: Arc<dyn RunTokenMinter + Send + Sync> = Arc::new(HostRunMinter);
    let revoker = HostRunRevoker::default();
    dispatch_core(
        wallet,
        region,
        task,
        wiring,
        model_client,
        catalogue,
        executor,
        advertised,
        RunTokenSeams {
            minter,
            revoker: &revoker,
        },
    )
}

/// **The per-run identity seams the driving loop composes the token lifecycle over.** The mint half
/// ([`myelin_flow::RunTokenMinter`], `Arc` so `handle_run` can hold it) + the teardown half
/// ([`myelin_agent_service::RunTokenRevoker`], borrowed for the run). The ONE metered dispatcher
/// ([`dispatch_core`]) is generic over the impl behind these seams, so BOTH the in-process
/// attribution floor (the hermetic drills' [`HostRunMinter`] / [`HostRunRevoker`]) and the REAL
/// Identity providers ([`IdentityRunMinter`] / [`IdentityRunRevoker`], wired by [`AgentHost`]) drive
/// the identical mint → run → revoke path.
struct RunTokenSeams<'a> {
    minter: Arc<dyn RunTokenMinter + Send + Sync>,
    revoker: &'a dyn RunTokenRevoker,
}

/// **The ONE metered `RunSubstrate` dispatcher core — the single construction site (F1 preserved).**
/// Wraps the vendor client (task inject + answer capture), assembles the run substrate with the
/// caller-supplied identity `seams`, and drives [`handle_run`](myelin_agent_service::SkeletonAgent::handle_run).
/// The mint is the ONLY token path: a failed mint aborts the run BEFORE any reserve (fail-closed).
#[allow(clippy::too_many_arguments)]
fn dispatch_core(
    wallet: &dyn RunWallet,
    region: Region,
    task: &LlmRunTask,
    wiring: &mut RunSubstrateWiring<'_>,
    model_client: Box<dyn ModelClient + Send + Sync>,
    catalogue: &dyn ToolSurface,
    executor: &dyn ToolExecutor,
    advertised: &[ToolSchema],
    seams: RunTokenSeams<'_>,
) -> Result<LlmRunReport, AgentHostError> {
    // Wrap the vendor client so the run's task + tool specs are injected and its final answer captured
    // — WITHOUT touching handle_run or the runtime.
    let tool_specs = advertised.iter().map(tool_schema_to_spec).collect();
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

    // The per-run identity seams (the caller wired either the in-process floor or REAL Identity). The
    // catalogue + executor are the caller's (the tool-less path passes the no-tools pair).
    let mut gate = myelin_storage::agent_run_gate::AgentRunGate::new();
    let mut telemetry = SkeletonTelemetry::new();
    let agent_loop = SkeletonAgent::new();

    // ── F1: the ONE RunSubstrate construction site — `wallet: Some(wallet)`, ALWAYS. There is no
    //    path here that sets `None`, and the `&dyn RunWallet` parameter carries no `None`. A real
    //    paid run therefore cannot be dispatched unbilled. ──
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
        catalogue,
        executor,
        wallet: Some(wallet), // F1 — the metered wallet is always present for a real run.
        gate: &mut gate,
        ledger: wiring.ledger,
        available: task.available,
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

/// **The real-run entry point over the durable [`AgentWallet`] (F1 + the literal money signature).**
/// Takes a NON-OPTIONAL `&`[`AgentWallet`] (the durable prepaid wallet) and the run's `region`, and
/// dispatches through the single metered path [`dispatch_metered_llm_run`] — so the wallet is ALWAYS
/// `Some` (F1) and the run is metered per turn.
///
/// **F2 (region):** `region` MUST be the region the `wallet` was built for (the wallet debits under
/// its own instance's region). Prefer [`AgentHost`], which derives both from one provider so the
/// invariant is structural. Construct the runtime's brain by passing
/// `Box::new(`[`LunaClient::from_env`](myelin_agent_model::LunaClient::from_env)`()?)` as
/// `model_client` for a real Luna run.
pub fn run_llm_agent(
    wallet: &AgentWallet,
    region: Region,
    task: &LlmRunTask,
    wiring: &mut RunSubstrateWiring<'_>,
    model_client: Box<dyn ModelClient + Send + Sync>,
) -> Result<LlmRunReport, AgentHostError> {
    // `&AgentWallet` coerces to `&dyn RunWallet` — non-optional, so F1 holds.
    dispatch_metered_llm_run(wallet, region, task, wiring, model_client)
}

/// The default system framing when the caller supplies none — a legible, agent-labelled instruction
/// (agents are first-class and never disguised as human; ADR-08 / AI-Act).
fn default_system(system: &str) -> String {
    if system.trim().is_empty() {
        "You are a hosted agent. You are labelled as an agent. Answer the user's request \
         concisely and directly."
            .to_string()
    } else {
        system.to_string()
    }
}

// ───────────────────────── AgentHost: the F2-airtight composition object ─────────────────────────

/// **The hosted-agent composition object — the F2-airtight front door.** Owns the durable
/// [`AgentWallet`] and the [`Region`], both derived from ONE
/// [`SubstrateProvider`](myelin_storage::SubstrateProvider), so the wallet's region and the run's
/// region CANNOT disagree (F2 is structural, not a caller convention). Its [`run_llm_agent`] method
/// is the recommended real-run entry point.
///
/// Build it inside a Tokio runtime (the wallet captures the current runtime handle for its
/// sync→async bridge; the per-turn debit runs via `block_in_place`, so the run must be driven on a
/// multi-thread runtime worker — mirror [`AgentWallet`]'s contract).
pub struct AgentHost {
    region: Region,
    wallet: AgentWallet,
    /// The REAL Identity per-run token providers (the mint + the durable S7 denylist). `Some` on a
    /// production host built via [`AgentHost::with_identity`]; `None` on a host built via
    /// [`AgentHost::new`] (the in-process attribution floor — the hermetic/dev path). When `Some`,
    /// every run mints + revokes a REAL per-run token; when `None`, the deterministic in-process
    /// stubs attribute the run (no DB, no crypto).
    identity: Option<HostIdentity>,
}

/// **The REAL Identity per-run token providers a production [`AgentHost`] mints + revokes through.**
/// The Identity minter ([`myelin_identity_service::RunTokenMinter`], a cloneable handle over the
/// durable S7 store + the PASETO signer) and the durable S7 [`RevocationStore`] (the
/// `(tenant, region)`-partitioned, RLS-scoped, PG-backed denylist). Both are run-agnostic and cheap
/// to clone; the host binds them to each run's verified scope + agent principal per dispatch.
struct HostIdentity {
    minter: IdentityRunTokenMinter,
    revocations: RevocationStore,
}

/// **An error building the REAL Identity providers for an [`AgentHost`] (fail-closed at
/// construction).** The durable cell token-authority root could not be recovered/generated (a wrong
/// seal key that does not unseal an existing root, or an unreachable store), or the recovered
/// material was invalid — a host is NEVER built with a degraded/absent signing root (that would mint
/// unverifiable or orphaned tokens).
#[derive(Debug)]
pub enum HostIdentityError {
    /// The durable cell-authority root could not be recovered or generated (unreachable store, or a
    /// seal key that does not unseal an existing sealed root — fail-closed, never a fresh root).
    CellRootUnavailable(String),
    /// The recovered cell-authority material was invalid (a corrupt sealed root).
    InvalidCellRoot(String),
}

impl core::fmt::Display for HostIdentityError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HostIdentityError::CellRootUnavailable(e) => write!(
                f,
                "hosted-agent Identity root refused to start (fail-closed, never a fresh/degraded \
                 signing root): {e}"
            ),
            HostIdentityError::InvalidCellRoot(e) => {
                write!(f, "hosted-agent Identity cell-authority material is invalid: {e}")
            }
        }
    }
}

impl std::error::Error for HostIdentityError {}

impl AgentHost {
    /// **Build a host over the IN-PROCESS attribution floor (the hermetic / dev path).** The wallet
    /// AND the region come from that ONE provider (F2). Per-run token mint/revoke uses the
    /// deterministic in-process stubs ([`HostRunMinter`] / [`HostRunRevoker`]) — no DB, no crypto. For
    /// a real multi-tenant hosted agent, use [`AgentHost::with_identity`], which mints + revokes REAL
    /// per-run tokens through Identity's durable S7 denylist.
    pub fn new(provider: SubstrateProvider) -> AgentHost {
        let region = Region(provider.config().region.clone());
        let wallet = AgentWallet::new(provider);
        AgentHost {
            region,
            wallet,
            identity: None,
        }
    }

    /// **Build a PRODUCTION host wired to the REAL Identity per-run token mint + durable revocation
    /// (the composition root for a real multi-tenant hosted agent).** The wallet + region come from
    /// the provider (F2); the per-run token lifecycle is composed over:
    ///
    /// - Identity's real [`myelin_identity_service::RunTokenMinter`] — a PASETO-signed per-run
    ///   attenuated token under the cell token authority (recovered durably from `cell_root` under the
    ///   operator `seal_key`, the SAME key path the edge gateway + CI runner use — never a fresh root
    ///   that would orphan minted tokens), whose authority is the run's delegation caveats intersected
    ///   monotonically and bound to the run's agent principal + a per-run `run:<id>` caveat; and
    /// - the durable S7 [`RevocationStore`] over the provider's PG pool — the `(tenant, region)`-
    ///   partitioned, RLS-scoped denylist the teardown revokes into (durable across a process restart,
    ///   idempotent even on a double-fire).
    ///
    /// The caller supplies `cell_id` + `seal_key` (from `MYELIN_CELL_ID` / `MYELIN_KMS_SEAL_KEY`, or a
    /// test-controlled dev key) and `rt` (the runtime handle the durable store's sync→async bridge
    /// drives on). Requires the `cell_root_durable_migrations` + `identity_durable_migrations` tables
    /// to have been applied (via the provider's `migrate`). Fail-closed: a root that will not unseal
    /// or an unreachable store errors here, BEFORE any run.
    pub async fn with_identity(
        provider: SubstrateProvider,
        cell_id: impl Into<String>,
        seal_key: &SealKey,
        rt: tokio::runtime::Handle,
    ) -> Result<AgentHost, HostIdentityError> {
        let region = Region(provider.config().region.clone());
        // Recover (or first-boot generate) the durable cell token-authority signing root — the SAME
        // durable-cell-root + seal-key path the edge gateway and the CI runner compose. Fail-closed on
        // a wrong seal key / an unreachable store (never a fresh root that orphans minted tokens).
        let material = DurableCellRootBacking::new(provider.db_pool().clone(), cell_id)
            .load_or_generate(seal_key)
            .await
            .map_err(|e| HostIdentityError::CellRootUnavailable(e.to_string()))?;
        let cell = Arc::new(
            CellTokenAuthority::from_material(&material)
                .map_err(|e| HostIdentityError::InvalidCellRoot(format!("{e:?}")))?,
        );
        // The durable S7 denylist over the provider's PG pool (RLS-scoped, crash-safe).
        let revocations =
            RevocationStore::with_pg(DurableRevocationBacking::new(provider.clone()), rt);
        // The real minter: the monotone-intersection delegation algebra + the S7 TTL register + the
        // REAL PASETO signer (no structural mock crypto in the production graph).
        let signer = Arc::new(PasetoCapabilitySigner::new(cell));
        let minter = IdentityRunTokenMinter::with_signer_and_tuples(revocations.clone(), None, signer);
        let wallet = AgentWallet::new(provider);
        Ok(AgentHost {
            region,
            wallet,
            identity: Some(HostIdentity {
                minter,
                revocations,
            }),
        })
    }

    /// The durable wallet (seed it with [`AgentWallet::credit`](myelin_storage::agent_wallet::AgentWallet::credit)
    /// before a run, inspect the balance after).
    pub fn wallet(&self) -> &AgentWallet {
        &self.wallet
    }

    /// The region this host (and its wallet) is scoped to.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The durable S7 revocation store — `Some` only on a host built via [`AgentHost::with_identity`]
    /// (the real-Identity path). Exposed so a caller/drill can assert a run's per-run token was REALLY
    /// revoked on teardown (its `run_token_state` is `TornDown`) and that the deny is durable (a fresh
    /// consult over the same pool sees the denylist entry).
    pub fn revocations(&self) -> Option<&RevocationStore> {
        self.identity.as_ref().map(|id| &id.revocations)
    }

    /// Compose the run's per-run identity seams: the REAL Identity mint/revoke (bound to THIS run's
    /// verified scope + agent principal + mint instant) when the host was built with identity, else
    /// `None` (the caller falls back to the in-process floor via the free dispatch functions). The
    /// returned revoker is a local the caller must keep alive for the borrow the seams hold.
    fn identity_seams(&self, task: &LlmRunTask) -> Option<(Arc<dyn RunTokenMinter + Send + Sync>, IdentityRunRevoker)> {
        let id = self.identity.as_ref()?;
        // The run's verified `(tenant, region)` scope — tenant-from-token (the agent principal),
        // region from THIS host (same provider as the wallet — F2). The mint's TTL register + the
        // teardown are both scoped to it (no cross-tenant path).
        let scope = TenantScope::from_verified_token(&task.agent, self.region.clone());
        let now = timestamp_from_epoch(task.now_secs);
        // The trigger actor is the agent itself (a hosted run acts as its agent); the delegation
        // intersection is driven by the run's caveats, so this does not widen authority.
        let minter: Arc<dyn RunTokenMinter + Send + Sync> = Arc::new(IdentityRunMinter::new(
            id.minter.clone(),
            scope.clone(),
            task.agent.clone(),
            task.agent.clone(),
            now,
        ));
        let revoker = IdentityRunRevoker::new(id.revocations.clone(), scope);
        Some((minter, revoker))
    }

    /// **Drive one metered hosted-agent run (F1 + F2).** Uses THIS host's durable wallet (always
    /// `Some` — F1) and THIS host's region (same provider as the wallet — F2). A host built with
    /// [`AgentHost::with_identity`] mints + revokes a REAL per-run token; else the in-process floor
    /// attributes the run. Pass
    /// `Box::new(`[`LunaClient::from_env`](myelin_agent_model::LunaClient::from_env)`()?)` as the brain
    /// for a real Luna run.
    pub fn run_llm_agent(
        &self,
        task: &LlmRunTask,
        wiring: &mut RunSubstrateWiring<'_>,
        model_client: Box<dyn ModelClient + Send + Sync>,
    ) -> Result<LlmRunReport, AgentHostError> {
        self.run_llm_agent_with_tools(
            task,
            wiring,
            model_client,
            &NoToolSurface,
            &NoToolExecutor,
            &[],
        )
    }

    /// **Drive one metered TOOL-EXECUTING hosted-agent run (F1 + F2).** The tools-enabled sibling of
    /// [`run_llm_agent`](AgentHost::run_llm_agent): the run may `validate_call` → `execute` the
    /// `advertised` tools through `catalogue` + `executor`, still over THIS host's durable wallet
    /// (F1) and region (F2). A host built with [`AgentHost::with_identity`] mints + revokes a REAL
    /// per-run token bounding the run; else the in-process floor attributes it. Pass a real
    /// [`ToolCatalogue`] + a `Direct` [`ToolExecutor`] (e.g. [`GitCheckStatusReadExecutor`]) + the
    /// matching [`ToolSchema`]s.
    #[allow(clippy::too_many_arguments)]
    pub fn run_llm_agent_with_tools(
        &self,
        task: &LlmRunTask,
        wiring: &mut RunSubstrateWiring<'_>,
        model_client: Box<dyn ModelClient + Send + Sync>,
        catalogue: &dyn ToolSurface,
        executor: &dyn ToolExecutor,
        advertised: &[ToolSchema],
    ) -> Result<LlmRunReport, AgentHostError> {
        match self.identity_seams(task) {
            // REAL Identity mint + durable revoke (a production host).
            Some((minter, revoker)) => dispatch_core(
                &self.wallet,
                self.region.clone(),
                task,
                wiring,
                model_client,
                catalogue,
                executor,
                advertised,
                RunTokenSeams {
                    minter,
                    revoker: &revoker,
                },
            ),
            // The in-process attribution floor (a hermetic/dev host built via `new`).
            None => dispatch_metered_llm_run_with_tools(
                &self.wallet,
                self.region.clone(),
                task,
                wiring,
                model_client,
                catalogue,
                executor,
                advertised,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent_model::{ModelReply, Usage};

    #[test]
    fn host_client_injects_system_and_prompt_on_the_first_step() {
        // A capturing inner client that records the request it was handed.
        struct Spy(Mutex<Option<ModelRequest>>);
        impl ModelClient for Spy {
            fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
                *self.0.lock().unwrap() = Some(request.clone());
                Ok(ModelResponse {
                    reply: ModelReply::Final {
                        content: "ready".into(),
                    },
                    usage: Usage::Reported {
                        input: 5,
                        cached_input: 0,
                        output: 1,
                    },
                })
            }
        }
        let spy = Arc::new(Spy(Mutex::new(None)));
        // The wrapper cannot share the Arc directly (it takes a Box), so wrap a thin forwarder.
        struct Fwd(Arc<Spy>);
        impl ModelClient for Fwd {
            fn complete(&self, r: &ModelRequest) -> Result<ModelResponse, ModelError> {
                self.0.complete(r)
            }
        }
        let (client, answer) = HostModelClient::wrap(
            "SYS".into(),
            "do the thing".into(),
            vec![],
            Box::new(Fwd(spy.clone())),
        );
        // An empty request (what LlmAgentRuntime builds from an empty Conversation).
        let resp = client.complete(&ModelRequest::default()).unwrap();
        assert!(matches!(resp.reply, ModelReply::Final { .. }));
        // The captured answer is the model's final content.
        assert_eq!(answer.take().as_deref(), Some("ready"));
        // The inner client SAW the injected system + the prompt as the opening user turn.
        let seen = spy.0.lock().unwrap().clone().unwrap();
        assert_eq!(seen.system, "SYS");
        assert_eq!(seen.turns.len(), 1);
        match &seen.turns[0] {
            ModelTurn::User { content } => assert_eq!(content, "do the thing"),
            other => panic!("expected an injected user turn, got {other:?}"),
        }
    }

    #[test]
    fn empty_system_falls_back_to_the_default_framing() {
        assert!(default_system("   ").contains("labelled as an agent"));
        assert_eq!(default_system("custom"), "custom");
    }

    /// **The wrapper injects the run's tool specs on EVERY step** (the loop leaves `conv.tools`
    /// empty, so the model would otherwise never learn the tool exists) — including a later tool-loop
    /// step whose request already carries history.
    #[test]
    fn host_client_injects_tool_specs_on_every_step() {
        struct Spy(Mutex<Vec<ModelRequest>>);
        impl ModelClient for Spy {
            fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError> {
                self.0.lock().unwrap().push(request.clone());
                Ok(ModelResponse {
                    reply: ModelReply::Final {
                        content: "ok".into(),
                    },
                    usage: Usage::NotReported,
                })
            }
        }
        let spy = Arc::new(Spy(Mutex::new(Vec::new())));
        struct Fwd(Arc<Spy>);
        impl ModelClient for Fwd {
            fn complete(&self, r: &ModelRequest) -> Result<ModelResponse, ModelError> {
                self.0.complete(r)
            }
        }
        let specs = vec![tool_schema_to_spec(&git_check_status_read_tool_schema())];
        let (client, _answer) =
            HostModelClient::wrap("SYS".into(), "task".into(), specs, Box::new(Fwd(spy.clone())));

        // First step (empty turns): tools injected.
        client.complete(&ModelRequest::default()).unwrap();
        // A later step that already carries history: tools STILL injected (the model needs the tool
        // definitions on the tool-result turn too).
        client
            .complete(&ModelRequest {
                turns: vec![ModelTurn::User {
                    content: "prior".into(),
                }],
                ..Default::default()
            })
            .unwrap();

        let seen = spy.0.lock().unwrap();
        assert_eq!(seen.len(), 2);
        for req in seen.iter() {
            assert_eq!(req.tools.len(), 1, "the run's tool is advertised every step");
            assert_eq!(req.tools[0].name, GIT_READ_CHECK_STATUS_TOOL);
            assert_eq!(req.tools[0].input_schema["type"], "object");
        }
    }

    /// **`ToolCatalogue` register/resolve round-trips** (the real permissioned catalogue a
    /// tools-enabled run validates each call against); an unknown name is `None`.
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
        let r = HostRunRevoker::default();
        assert!(!r.is_dead("j1", 10));
        let _ = r.revoke("j1", 10, 5);
        assert!(r.is_dead("j1", 10));
        // A re-revoke is a no-op success (lag 0).
        assert_eq!(r.revoke("j1", 10, 5), 0);
    }
}
