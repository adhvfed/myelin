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

use myelin_agent::{MeteredRuntime, ToolCall, ToolDef, ToolName, ToolResult, ToolSurface};
use myelin_agent_model::{
    LlmAgentRuntime, ModelClient, ModelError, ModelReply, ModelRequest, ModelResponse, ModelTurn,
};
use myelin_agent_service::{
    RunOutcomeKind, RunSubstrate, SkeletonAgent, SkeletonError, SkeletonTelemetry, ToolExecError,
    ToolExecutor,
};
use myelin_events::{IdMinter, OutboxStore};
use myelin_flow::{
    DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter, WfJournal,
};
use myelin_identity::Principal;
use myelin_storage::agent_wallet::AgentWallet;
use myelin_storage::reserve_settle::{CostLedger, MinorUnits};
use myelin_storage::SubstrateProvider;
use myelin_tenancy::{Region, TenantId};

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
    inner: Box<dyn ModelClient + Send + Sync>,
    answer: AnswerSlot,
}

impl HostModelClient {
    /// Wrap `inner`, returning the wrapper and a handle to read the captured answer after the run.
    fn wrap(
        system: String,
        prompt: String,
        inner: Box<dyn ModelClient + Send + Sync>,
    ) -> (HostModelClient, AnswerSlot) {
        let answer = AnswerSlot::default();
        (
            HostModelClient {
                system,
                prompt,
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
    // Wrap the vendor client so the run's task is injected and its final answer captured — WITHOUT
    // touching handle_run or the runtime.
    let (host_client, answer) = HostModelClient::wrap(
        default_system(&task.system),
        task.prompt.clone(),
        model_client,
    );
    let mut runtime = LlmAgentRuntime::new(Box::new(host_client));
    if let Some(max) = task.max_output_tokens {
        runtime = runtime.with_max_output_tokens(max);
    }

    // The v1 in-process identity + the no-tools catalogue/executor (locals that outlive the run).
    let minter: Arc<dyn RunTokenMinter + Send + Sync> = Arc::new(HostRunMinter);
    let revoker = HostRunRevoker::default();
    let catalogue = NoToolSurface;
    let executor = NoToolExecutor;
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
        minter_token: minter,
        agent_id: task.agent_id.clone(),
        caveats: DelegationCaveats(vec![]),
        token_ttl_secs: task.token_ttl_secs,
        revoker: &revoker,
        catalogue: &catalogue,
        executor: &executor,
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
}

impl AgentHost {
    /// Build a host from a provider — the wallet AND the region come from that ONE provider (F2).
    pub fn new(provider: SubstrateProvider) -> AgentHost {
        let region = Region(provider.config().region.clone());
        let wallet = AgentWallet::new(provider);
        AgentHost { region, wallet }
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

    /// **Drive one metered hosted-agent run (F1 + F2).** Uses THIS host's durable wallet (always
    /// `Some` — F1) and THIS host's region (same provider as the wallet — F2). Pass
    /// `Box::new(`[`LunaClient::from_env`](myelin_agent_model::LunaClient::from_env)`()?)` as the brain
    /// for a real Luna run.
    pub fn run_llm_agent(
        &self,
        task: &LlmRunTask,
        wiring: &mut RunSubstrateWiring<'_>,
        model_client: Box<dyn ModelClient + Send + Sync>,
    ) -> Result<LlmRunReport, AgentHostError> {
        dispatch_metered_llm_run(&self.wallet, self.region.clone(), task, wiring, model_client)
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
