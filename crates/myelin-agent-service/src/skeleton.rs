//! # `skeleton` — the SKELETON runtime: the gateway → identity → dispatch → reserve → trace path
//! at zero cost (AG-P4 → P-216, M2-A)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §3.1 (the SKELETON runtime —
//! *no model, no tools; drives the whole gateway/identity/dispatch/reserve/trace path at ~zero
//! cost*), §2.3 (`Agent::handle` — the bounded driven multi-turn loop), §5.1 (the agent loop driver:
//! build_conversation → reserve → step → route → settle), §5.6 (a run is a durable workflow — the
//! workflow owns budget/gates/state; step/exec are activities; reserve/settle are the bookends),
//! §5.7 (per-run identity: mint at dispatch, token life == run life, revoke on teardown — *the simple
//! form here*; the full mint/scrub/revoke + re-mint lands in AG-P13).
//!
//! **Contract-index:** OWNS 8.3 (`AgentRuntime::step` — the SKELETON impl) + 8.5 (`Agent::handle` —
//! the loop body). CONSUMES 4.7 (`mint_run_token`/`revoke` — the per-run token), 11.7
//! (`reserve`/`settle` — the cost-gate bookends), 9.1/9.2/9.5 (`DurableExecutor` + `WfCtx` — a run is
//! a durable workflow), 2.2 (`OutboxTx::emit(draft, cause)` — nested causality), 1.8 (the telemetry
//! signal set — the trace + the reserve/settle ledger are the green artifacts).
//!
//! ## What this prompt ships — the SKELETON path proven at zero cost
//!
//! The SKELETON is the **first-runnable proof slice** (roadmap M2-A): it exercises the WHOLE
//! substrate path — gateway dispatch, per-run identity, the reserve/settle cost gate, the durable
//! workflow, and the trace — **without thinking**. There is no model and no tools. The point is to
//! prove the substrate is right (EI-03 §3 SKELETON → mock → real build order; EI-01 §3 prove-it-or-
//! it-isn't-real) BEFORE a brain (AG-P5) or hands (AG-P6/AG-P15) plug in.
//!
//! The two contracts this owns:
//! - [`SkeletonAgentRuntime`] (8.3) — an [`AgentRuntime`] with no model and no tools; its
//!   [`step`](AgentRuntime::step) submits IMMEDIATELY ([`StepOutcome::Submit`]). It exercises the
//!   brain seam, it does not think. The `--use-mock` deterministic brain (AG-P5) plugs into the SAME
//!   handle loop behind the SAME `&dyn AgentRuntime` seam; the LLM brain (AG-P25, post-M5) likewise.
//! - [`SkeletonAgent`] (8.5) — the platform-owned `Agent::handle` loop body, wired as a **durable
//!   workflow** (§5.6). On an [`InboxEvent`] it runs the chained substrate path inside ONE
//!   [`WfCtx`](myelin_flow::WfCtx) co-commit transaction (so the trace journal row + its emit are
//!   atomic — the same FLOW-D5 silent-data-loss floor `myelin-flow` owns):
//!   1. **mint** a per-run attenuated token via [`RunTokenMinter::mint_run_token`] (4.7), token life
//!      == run life;
//!   2. **reserve** at dispatch via the storage [`AgentRunGate`](myelin_storage::agent_run_gate::AgentRunGate)
//!      (11.7 — no balance → no run);
//!   3. **build** the [`Conversation`] (EMPTY for the SKELETON — no trace history, no tools);
//!   4. **step** the brain (an activity; the SKELETON submits immediately);
//!   5. **write** the (near-empty) trace row as a journaled+co-committed activity, carrying nested
//!      causality via [`WfCtx::emit`](myelin_flow::WfCtx::emit)`(draft, cause)` (2.2);
//!   6. **settle** the reservation (reserved == settled — a zero-cost SKELETON bills 0);
//!   7. on **teardown** revoke the token IDEMPOTENTLY (even on crash) via
//!      [`RunTokenRevoker::revoke`], belt-and-suspenders with the token's auto-expiring TTL.
//!
//! ## The telemetry signal set (contract 1.8) — a path that emits no signal has FAILED the drill
//! Every run emits the survival signals into a [`SkeletonTelemetry`]: a balanced reserve/settle
//! ledger (`reserved == settled`), a written `trace_ref`, and the token-revocation lag. The drill
//! reads these — observability is part of the pass (EI-01 §3).
//!
//! ## AG-D8 (the no-tool leg) — per-run token revoked on teardown AND auto-expires; 0 leak
//! [`SkeletonAgent::handle`] revokes the per-run token on teardown **even when the run is killed
//! mid-flight** ([`RunOutcomeKind::KilledMidFlight`]). The child environment the run hands to a tool
//! is a [`ChildEnv`] minted from the per-run token ONLY — it inherits **no** shared platform token
//! (the anti-leak unset, §5.7). The drill asserts: revoked-on-teardown + auto-expiry ≤ W + 0 shared
//! token leaked into the child env + revocation-lag within bound. The **re-mint-on-resume leg is
//! AG-P13** (→ P-225).
//!
//! ## FLOORS named (this is the SKELETON — a skeleton that masquerades as a working agent is the
//! failure; VISION §3, EI-01 §1)
//! - **The BRAIN is a no-op.** [`SkeletonAgentRuntime::step`] submits immediately — no model, no
//!   reasoning. The deterministic scripted brain is `MockAgentRuntime` (**AG-P5 → P-217**); the real
//!   vendor brain is `LlmAgentRuntime` (**AG-P25, post-M5**, designed-not-built — the only place a
//!   model/SDK/prompt/model-name string ever appears; `no-llm-in-platform`, contract 1.6).
//! - **The TOOLS are absent.** The SKELETON builds an EMPTY [`Conversation`] (no `tools`) and routes
//!   nothing — `ToolHands::exec` (compute/external) + `EffectApi::apply`'s plan-then-apply pipeline
//!   (mutate) land in **AG-P6 → P-218 / AG-P15 → P-226**.
//! - **The FULL per-run identity** (mint / scrub the shared token / revoke idempotently / re-mint on
//!   resume) is **AG-P13 → P-225**. Here is the *simple form* — mint at dispatch, revoke on teardown,
//!   the anti-leak unset, the auto-expiry TTL. The re-mint-on-resume leg of AG-D8 is AG-P13's.
//! - **The mint/revoke + reserve/settle + outbox BODIES are the consumed subsystems'.** This crate is
//!   the CONSUMER: it drives Identity's `mint_run_token`/`revoke` (4.7) through the [`RunTokenMinter`]
//!   / [`RunTokenRevoker`] seams (the same trait-decoupling `myelin-flow` uses, so the DAG stays
//!   acyclic — no production dep on `myelin-identity-service`), Storage's [`AgentRunGate`] (11.7), and
//!   `myelin-flow`'s [`WfCtx`](myelin_flow::WfCtx) co-commit (9.2). The CDC pairs each with a real
//!   provider impl (`tests/`).

use crate::effect_api::validate_call;
use crate::tool_exec::ToolExecutor;
use myelin_agent::{
    Agent, AgentRuntime, Conversation, InboxEvent, MeteredRuntime, MeteredStep, RunOutcome,
    StepOutcome, Submission, TokenUsage, ToolOutcome, ToolSurface, Turn,
};
use myelin_events::{
    Actor, AggregateKey, DataRole, EmitContextBase, EventDraft, EventType, Timestamp, Visibility,
};
use myelin_flow::{
    DelegationCaveats, RetryPolicy, RunTokenError, RunTokenHandle, RunTokenMinter, WfCtx, WfJournal,
};
use myelin_identity::Principal;
use myelin_refs::ArtifactRef;
use myelin_storage::agent_run_gate::{AgentRunGate, DispatchError};
use myelin_storage::reserve_settle::{CostLedger, MinorUnits, RunId as StorageRunId};
use myelin_tenancy::{Region, TenantId};

/// The frozen event type the trace-written-and-emitted activity emits (BUS §6.2 token, PII-free).
/// A SKELETON run's terminal `agent.run.traced` event carries the trace `ArtifactRef`
/// references-not-payloads (never a reasoning body) so a downstream consumer can index/erase it.
pub const AGENT_RUN_TRACED_EVENT: &str = "agent.run.traced";

/// The metered-unit dimension the SKELETON bills (zero cost — a SKELETON step has no model call).
/// The dimension EXISTS so the settle ledger is a real `(unit, wholesale, markup)` row; the SKELETON
/// settles ZERO units (reserved == settled at the floor estimate, refund == reserved).
pub const SKELETON_STEP_UNIT: &str = "skeleton.step";

/// **The bounded driving loop's max-turns ceiling (§2.3 — the bounded, driven multi-turn loop).**
/// The runaway guard for THIS slice: the loop steps the brain at most `DEFAULT_MAX_TURNS` times; a
/// brain that never [`Submit`](StepOutcome::Submit)s within the bound terminates the run GRACEFULLY
/// as [`SkeletonError::MaxTurnsExhausted`] (never an unbounded loop, never a panic). A well-formed
/// run submits far below this.
///
/// This is a coarse structural bound, NOT metering: the per-call token cost → wallet debit + spend
/// cap is a SEPARATE, decision-gated follow-on (the reserve/settle gate is untouched here). It sits
/// alongside the run's other independent ceilings (the reserve/settle budget the gate enforces at
/// dispatch, and the causal-depth ceiling the flow runtime enforces).
pub const DEFAULT_MAX_TURNS: usize = 16;

// ───────────────────────── 8.3 — the SKELETON runtime (no model, no tools) ──────────────────────

/// **8.3 — the SKELETON [`AgentRuntime`] (no model, no tools).** Its [`step`](AgentRuntime::step)
/// submits IMMEDIATELY — it exercises the brain seam, it does NOT think. This is the §3.1 SKELETON:
/// the lever that drives the whole gateway/identity/dispatch/reserve/trace path at ~zero cost.
///
/// **Floor (named):** this is NOT a working brain. The deterministic scripted brain is
/// `MockAgentRuntime` (AG-P5 → P-217) on the SAME `&dyn AgentRuntime` seam; the real vendor brain is
/// `LlmAgentRuntime` (AG-P25, post-M5). No model/SDK/prompt/model-name string appears here (the
/// `no-llm-in-platform` ratchet, contract 1.6).
#[derive(Clone, Copy, Debug, Default)]
pub struct SkeletonAgentRuntime;

impl SkeletonAgentRuntime {
    /// A fresh SKELETON runtime.
    pub fn new() -> SkeletonAgentRuntime {
        SkeletonAgentRuntime
    }
}

impl AgentRuntime for SkeletonAgentRuntime {
    /// Submit immediately — the SKELETON has no model and no tools, so the very first step is the
    /// terminal one. It returns a fixed, content-free [`Submission`] so the loop drives to settle +
    /// trace deterministically (the SAME code path the mock/LLM brains hit; only the decision
    /// differs).
    fn step(&self, _conv: &Conversation) -> StepOutcome {
        StepOutcome::Submit(Submission(
            "skeleton: no model, no tools — immediate submit".into(),
        ))
    }
}

/// The SKELETON has no model, so it has no usage source: it inherits the default
/// [`MeteredRuntime::step_metered`] (the plain submit + [`TokenUsage::NotReported`]). Explicit (not
/// blanket) so the vendor `LlmAgentRuntime` override does not collide under coherence.
impl MeteredRuntime for SkeletonAgentRuntime {}

// ───────────────────────── per-run identity: the revoke seam (4.7) + the child-env anti-leak ─────

/// **The engine's view of the contract-4.7 `revoke` surface (CONSUMED, §5.7).** A trait so
/// `myelin-agent-service` does NOT take a production dependency on `myelin-identity-service` (the DAG
/// stays acyclic — the same decoupling [`RunTokenMinter`] uses for the mint half). The Identity
/// `RevocationStore::tear_down_run_token` provider is paired with this consumer seam in the CDC
/// (`tests/cdc_4_7_revoke.rs`, dev-dep only).
///
/// `revoke` is **idempotent even on crash** (§5.7): revoking an already-revoked / never-minted `jti`
/// is a no-op success — a teardown that fires twice (the explicit revoke + a crash-recovery sweep)
/// never errors. The teardown is belt-and-suspenders with the token's auto-expiring TTL.
pub trait RunTokenRevoker {
    /// **`revoke(jti)` (contract 4.7).** Revoke the per-run token by its `jti` — idempotently, even
    /// on crash. Returns the measured revocation lag (the seconds between the run's teardown instant
    /// and the revoke landing) so the [`SkeletonTelemetry`] can assert it is within bound. A re-revoke
    /// returns lag `0` (already denylisted — a no-op).
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64;

    /// **Has this `jti` been revoked OR auto-expired by `now_secs`?** The AG-D8 assertion reads this:
    /// a killed-mid-flight run's token is revoked-on-teardown AND, even absent the explicit revoke,
    /// auto-expires within the TTL window W. Both legs make the token dead — `true` once either fires.
    fn is_dead(&self, jti: &str, now_secs: i64) -> bool;
}

/// **The child environment a run hands a tool — minted from the per-run token ONLY (§5.7, the
/// anti-leak unset).** The SKELETON's run unsets any shared platform token in the child env (so a
/// tool the run spawns inherits NO ambient platform credential — *an agent cannot leak the platform's
/// authority into a child*). The AG-D8 drill asserts [`ChildEnv::shared_platform_token`] is `None` —
/// 0 shared token leaked — and that the only credential is the per-run `jti`.
///
/// Built by [`ChildEnv::for_run`]: the per-run `jti` is the ONLY credential; the shared platform
/// token slot is explicitly cleared (never inherited from the parent environment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildEnv {
    /// The per-run token's `jti` — the ONLY credential the child inherits (token life == run life).
    pub run_token_jti: String,
    /// The shared platform token — **always `None`**: the anti-leak unset clears it so no ambient
    /// platform credential leaks into a tool's environment (§5.7). 0 leak by construction.
    pub shared_platform_token: Option<String>,
}

impl ChildEnv {
    /// **Mint a child environment for a run from its per-run token ONLY.** The shared platform token
    /// is explicitly UNSET (cleared, never inherited) — the anti-leak property (§5.7). Even if the
    /// PARENT process holds a `shared_platform_token`, the child gets `None`.
    pub fn for_run(run_token_jti: impl Into<String>) -> ChildEnv {
        ChildEnv {
            run_token_jti: run_token_jti.into(),
            // The anti-leak unset: clear any inherited shared platform token (0 leak, §5.7).
            shared_platform_token: None,
        }
    }

    /// Whether this child env leaked a shared platform token (the AG-D8 headline: must be `false`).
    pub fn leaked_shared_token(&self) -> bool {
        self.shared_platform_token.is_some()
    }
}

// ───────────────────────── 1.8 — the telemetry signal set (the green artifacts) ─────────────────

/// **The contract-1.8 survival signals the SKELETON path emits (the green artifacts, §3.1).** A path
/// that survives but emits NO signal has FAILED the drill (EI-01 §3 — observability is part of the
/// pass). The drill reads: a balanced reserve/settle ledger (`reserved == settled`), a written
/// `trace_ref`, and the token-revocation lag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkeletonTelemetry {
    /// The total reserved across runs (integer minor-units).
    reserved: u64,
    /// The total settled across runs (integer minor-units) — `reserved == settled` is the balanced
    /// ledger the drill asserts (a SKELETON settles its floor reservation, refunding the rest).
    settled: u64,
    /// The number of trace rows written (one per completed run — `trace_ref` is non-empty).
    traces_written: u64,
    /// The maximum token-revocation lag observed (seconds between teardown and revoke landing). The
    /// AG-D8 drill asserts it is within the revocation bound.
    max_revocation_lag: u64,
    /// The number of per-run tokens revoked on teardown (one per run — even on a killed run).
    tokens_revoked: u64,
    /// The number of runs that completed the full chain (mint → reserve → step → trace → settle →
    /// revoke). Distinct from killed-mid-flight runs (which still revoke).
    runs_completed: u64,
    /// The number of runs killed mid-flight (AG-D8): the token is STILL revoked on teardown.
    runs_killed: u64,
    // ───────── raw per-run token usage totals (contract 8.3; NON-FINANCIAL — observability only) ──
    //
    // The run's total RAW provider token counts, accumulated per turn from the metered brain step
    // ([`MeteredRuntime::step_metered`]). This is a NEW, separate observability signal — it holds NO
    // money and does NOT touch the reserve/settle ledger above: pricing these counts into a bill is a
    // separate, decision-gated slice. A SKELETON/Mock run reports NotReported every turn, so its token
    // totals stay 0 and the balanced-ledger assertions are unaffected.
    /// Total non-cached prompt (input) tokens across all reported turns (saturating).
    tokens_input: u64,
    /// Total cached prompt (input) tokens across all reported turns (saturating).
    tokens_cached_input: u64,
    /// Total completion (output) tokens across all reported turns (saturating).
    tokens_output: u64,
    /// The number of turns whose usage was [`TokenUsage::NotReported`] (a usage-less brain, or a
    /// provider that omitted usage). The future metering slice reads this to fail closed.
    turns_usage_not_reported: u64,
}

impl SkeletonTelemetry {
    /// A fresh, zeroed signal set.
    pub fn new() -> SkeletonTelemetry {
        SkeletonTelemetry::default()
    }
    /// The total reserved (minor-units).
    pub fn reserved(&self) -> u64 {
        self.reserved
    }
    /// The total settled (minor-units). The balanced-ledger gate: `reserved == settled`.
    pub fn settled(&self) -> u64 {
        self.settled
    }
    /// **The balanced-ledger predicate (the §5.4 gate):** every minor-unit reserved was settled
    /// (billed + refunded), so the ledger nets to zero outstanding. A SKELETON bills 0 and refunds
    /// the whole reservation; the gate is `reserved == settled` regardless of the split.
    pub fn ledger_balanced(&self) -> bool {
        self.reserved == self.settled
    }
    /// The number of trace rows written (one per completed run).
    pub fn traces_written(&self) -> u64 {
        self.traces_written
    }
    /// The max token-revocation lag (seconds) observed.
    pub fn max_revocation_lag(&self) -> u64 {
        self.max_revocation_lag
    }
    /// The number of per-run tokens revoked on teardown.
    pub fn tokens_revoked(&self) -> u64 {
        self.tokens_revoked
    }
    /// The number of runs that completed the full chain.
    pub fn runs_completed(&self) -> u64 {
        self.runs_completed
    }
    /// The number of runs killed mid-flight (AG-D8).
    pub fn runs_killed(&self) -> u64 {
        self.runs_killed
    }
    /// Total raw non-cached prompt (input) tokens across all reported turns (NON-FINANCIAL).
    pub fn tokens_input(&self) -> u64 {
        self.tokens_input
    }
    /// Total raw cached prompt (input) tokens across all reported turns (NON-FINANCIAL).
    pub fn tokens_cached_input(&self) -> u64 {
        self.tokens_cached_input
    }
    /// Total raw completion (output) tokens across all reported turns (NON-FINANCIAL).
    pub fn tokens_output(&self) -> u64 {
        self.tokens_output
    }
    /// The number of turns whose usage was [`TokenUsage::NotReported`] (the fail-closed signal).
    pub fn turns_usage_not_reported(&self) -> u64 {
        self.turns_usage_not_reported
    }

    /// **Accumulate ONE metered turn's raw token usage (contract 8.3; observability only).** A
    /// reported turn saturating-adds its raw counts into the run totals; a `NotReported` turn bumps
    /// the not-reported counter (the future metering slice fails closed on it). This touches NO money
    /// and NO reserve/settle state — token totals are a separate signal from the cost ledger.
    fn record_token_usage(&mut self, usage: &TokenUsage) {
        match usage {
            TokenUsage::Reported {
                input,
                cached_input,
                output,
            } => {
                self.tokens_input = self.tokens_input.saturating_add(*input);
                self.tokens_cached_input = self.tokens_cached_input.saturating_add(*cached_input);
                self.tokens_output = self.tokens_output.saturating_add(*output);
            }
            TokenUsage::NotReported => {
                self.turns_usage_not_reported = self.turns_usage_not_reported.saturating_add(1);
            }
        }
    }

    fn record_reserve(&mut self, amount: u64) {
        self.reserved = self.reserved.saturating_add(amount);
    }
    fn record_settle(&mut self, amount: u64) {
        self.settled = self.settled.saturating_add(amount);
    }
    fn record_trace(&mut self) {
        self.traces_written = self.traces_written.saturating_add(1);
    }
    fn record_revoke(&mut self, lag: u64) {
        self.tokens_revoked = self.tokens_revoked.saturating_add(1);
        if lag > self.max_revocation_lag {
            self.max_revocation_lag = lag;
        }
    }
}

// ───────────────────────── 8.5 — the platform-owned Agent::handle loop body ─────────────────────

/// **How a SKELETON run terminated.** A run either drives the FULL chain to completion, or is KILLED
/// mid-flight (the AG-D8 no-tool leg: the failure-injection harness kills the run after dispatch but
/// before it submits — the teardown STILL revokes the token).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcomeKind {
    /// The run completed the chain: mint → reserve → step → trace → settle → revoke.
    Completed,
    /// The run was killed mid-flight (after dispatch, before submit) — the teardown still revoked the
    /// per-run token (revoke-even-on-crash, §5.7). The AG-D8 no-tool leg.
    KilledMidFlight,
}

/// **An error driving the SKELETON loop body.** Surfaced LOUD (never swallowed, EI-01 §2): a no-
/// balance dispatch, a failed mint, or a co-commit failure aborts the run with a typed value — never
/// a silent half-run. The teardown (token revoke) STILL fires on every error path (defer-on-drop
/// semantics — even an aborted run is torn down).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkeletonError {
    /// The reserve-at-dispatch was REFUSED (no balance → no run, 11.7 / AG-D11). The run never
    /// started; no trace, no settle — but the (never-minted-for-flight) token is still torn down.
    DispatchRefused(String),
    /// The per-run token mint failed (Identity unavailable / refused, 4.7). The run does not start
    /// under no token (never run unattributed — §5.7).
    MintFailed(String),
    /// The durable-workflow co-commit (the trace journal row + its emit) failed (9.2 / 2.2). Loud —
    /// a step is either fully journaled-and-emitted or neither (FLOW-D5).
    CoCommit(String),
    /// A proposed [`ToolCall`](myelin_agent::ToolCall) FAILED the `validate_call` security checkpoint
    /// (the tool is unregistered, or its untrusted model arguments don't satisfy the tool's schema).
    /// The run aborts FAIL-CLOSED — the unvalidated arguments are NEVER dispatched to a tool (§2.1,
    /// plan-then-apply survives). The teardown STILL fires. Carries the loud validation reason.
    ToolValidationRejected(String),
    /// The [`ToolExecutor`] returned an error executing an already-validated call. Surfaced LOUD; the
    /// run aborts and the teardown STILL fires (never a silent half-run, EI-01 §2).
    ToolExecFailed(String),
    /// The bounded driving loop reached [`DEFAULT_MAX_TURNS`] without the brain submitting (the
    /// runaway guard for this slice). The run terminates GRACEFULLY — 0 trace written, the teardown
    /// fires, and the reservation is left in-flight (the never-interrupt invariant, exactly as on a
    /// mid-flight kill). This is a coarse structural bound, NOT metering (a decision-gated follow-on).
    MaxTurnsExhausted {
        /// The run that exhausted its turn budget.
        run_id: String,
        /// The turn ceiling that was hit ([`DEFAULT_MAX_TURNS`]).
        turns: usize,
    },
}

impl core::fmt::Display for SkeletonError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SkeletonError::DispatchRefused(m) => write!(f, "SKELETON dispatch refused: {m}"),
            SkeletonError::MintFailed(m) => write!(f, "SKELETON mint failed: {m}"),
            SkeletonError::CoCommit(m) => write!(f, "SKELETON co-commit failed: {m}"),
            SkeletonError::ToolValidationRejected(m) => {
                write!(f, "SKELETON tool-call validation rejected (fail-closed): {m}")
            }
            SkeletonError::ToolExecFailed(m) => write!(f, "SKELETON tool execution failed: {m}"),
            SkeletonError::MaxTurnsExhausted { run_id, turns } => write!(
                f,
                "SKELETON bounded loop exhausted: run={run_id} reached max_turns={turns} without a submit"
            ),
        }
    }
}

impl std::error::Error for SkeletonError {}

impl From<DispatchError> for SkeletonError {
    fn from(e: DispatchError) -> SkeletonError {
        SkeletonError::DispatchRefused(e.to_string())
    }
}

impl From<RunTokenError> for SkeletonError {
    fn from(e: RunTokenError) -> SkeletonError {
        SkeletonError::MintFailed(e.to_string())
    }
}

/// **The per-run substrate context the SKELETON loop body drives over (the gateway → identity →
/// dispatch → reserve → trace path).** Carries the run identity (tenant/region/agent/run_id), the
/// per-run mint lease (4.7), the reserve/settle gate + ledger (11.7), the durable-workflow journal +
/// outbox (9.2), and the wallet balance the reserve debits. Built by the dispatch tier (3.6) from a
/// delivered [`InboxEvent`]; the SKELETON drives it once per run.
pub struct RunSubstrate<'a> {
    /// The verified `(tenant, region)` the run executes under (tenant-from-token; never a path).
    pub tenant: TenantId,
    /// The residency-pinned region (fr-par in dev/prod via env).
    pub region: Region,
    /// The agent principal the run acts as (a `Principal` with `kind=agent`).
    pub agent: Principal,
    /// The run id (the durable-workflow instance handle).
    pub run_id: String,
    /// The contract-4.7 mint seam (CONSUMED) — the SKELETON mints a per-run attenuated token ONCE at
    /// dispatch (token life == run life). A trait so this crate takes no production dep on Identity
    /// (the same decoupling `myelin-flow` uses); the CDC pairs it with the real provider.
    pub minter_token: std::sync::Arc<dyn RunTokenMinter + Send + Sync>,
    /// The agent principal id the per-run token is minted FOR (the run's agent identity).
    pub agent_id: String,
    /// The delegation caveats the mint attenuates the token with (the §6 grant chain — attenuate-only).
    pub caveats: DelegationCaveats,
    /// The per-run token TTL bound, in seconds (token life == run life; the fail-static window W).
    pub token_ttl_secs: u64,
    /// The contract-4.7 revoke seam — the teardown revoke (idempotent, even on crash).
    pub revoker: &'a dyn RunTokenRevoker,
    /// The one permissioned tool catalogue (8.1) the driving loop validates each proposed
    /// [`ToolCall`](myelin_agent::ToolCall) against ([`validate_call`], the security checkpoint). The
    /// SKELETON registers NO tools, so its catalogue is EMPTY and the loop body is never entered (the
    /// brain submits on turn 0). The delegation-scoped subset (AG-P7) is the same [`ToolSurface`].
    pub catalogue: &'a dyn ToolSurface,
    /// The [`ToolExecutor`] seam — turns a VALIDATED [`ToolCall`](myelin_agent::ToolCall) into a
    /// [`ToolResult`](myelin_agent::ToolResult). The loop's tool-dispatch dependency; the three real
    /// per-route impls (Read→subsystem read, Compute→sandbox exec, Mutate/External→EffectApi) are
    /// named-but-unwired follow-ons. Unused by the SKELETON (it never proposes a tool).
    pub executor: &'a dyn ToolExecutor,
    /// The reserve/settle gate (11.7) that fronts the run — no balance → no run.
    pub gate: &'a mut AgentRunGate,
    /// The Storage-owned durable cost ledger the gate drives (11.7).
    pub ledger: &'a mut CostLedger,
    /// The wallet balance the reserve debits (from Commercial; no balance → no run).
    pub available: MinorUnits,
    /// The run's estimated upper-bound cost reserved at dispatch (integer minor-units). A SKELETON's
    /// estimate is a small floor; it settles 0 and refunds the rest (reserved == settled).
    pub estimate: MinorUnits,
    /// The durable-workflow outbox the trace activity co-commits its emit into (BUS-2, the ONLY emit
    /// path — there is no second publish path).
    pub outbox: &'a myelin_events::OutboxStore,
    /// The ULID minter the outbox stamps emitted event ids with.
    pub minter: std::sync::Arc<dyn myelin_events::IdMinter>,
    /// The `wf_history`/`wf_activity_attempt` journal the trace activity writes (9.2).
    pub journal: WfJournal,
    /// The engine's epoch-seconds clock (the lease/revocation clock the teardown reads).
    pub now_secs: i64,
}

/// **8.5 — the platform-owned `Agent::handle` SKELETON loop body (the durable-workflow driver).**
///
/// This is the ONE platform-owned loop — identical for mock and real (the brain is the only
/// swappable part). It is NOT a strategy seam. For the SKELETON it drives the chained substrate path
/// inside one [`WfCtx`] co-commit (§5.6 — a run is a durable workflow). See the module doc for the
/// seven-step chain.
pub struct SkeletonAgent;

impl SkeletonAgent {
    /// A SKELETON agent (the platform-owned loop holder — stateless; the run state lives in the
    /// durable workflow + the trace, §3.1).
    pub fn new() -> SkeletonAgent {
        SkeletonAgent
    }

    /// **Drive a multi-day-paused run through RESUME using the per-run identity (AG-P13, §5.7 C6).**
    /// The loop-driver wiring the AG-P13 deliverable owns: a run that parked for *days* on a HITL gate
    /// (or a long `SCHEDULE_AND_RUN_JOB`, AG-P16) spans its per-run token's TTL. On wake the driver
    /// RE-MINTS a fresh attenuated token via [`crate::RunIdentity::remint_on_resume`] (same caveats,
    /// the REMAINING run life) BEFORE the resumed work runs, so the resumed activity executes under a
    /// FRESH live token — the run stays attributed within the TTL bound, 0 unattributed window. On
    /// teardown the CURRENT (re-minted) token is revoked idempotently.
    ///
    /// This is the engine-side counterpart to `myelin-flow::WfCtx::remint_on_resume` (the durable
    /// engine's automatic resume-leg hook): the Agent-Fabric driver additionally clamps the re-mint
    /// TTL to the run's *remaining* life (the §5.7 C6 tightening), so a long pause never widens the
    /// attribution window past the run's own deadline. Returns the fresh token's `jti` (the resumed
    /// run's live attribution principal) or a [`SkeletonError::MintFailed`] LOUD (a resume past the
    /// run deadline, or a refused mint, never silently runs unattributed).
    pub fn resume_run(
        &self,
        identity: &mut crate::RunIdentity,
        revoker: &dyn RunTokenRevoker,
        resume_at_secs: i64,
        telemetry: &mut SkeletonTelemetry,
    ) -> Result<String, SkeletonError> {
        // RE-MINT on resume (4.7, §5.7 C6): a fresh attenuated token with the SAME caveats and the
        // REMAINING run life. A resume past the run deadline (no remaining life) surfaces LOUD — the
        // resumed work must NOT run past the run's own allotted life (never widen attribution).
        let jti = identity
            .remint_on_resume(resume_at_secs)
            .map_err(|e| SkeletonError::MintFailed(e.to_string()))?
            .jti
            .clone();
        // TEARDOWN: revoke the CURRENT (re-minted) token idempotently even on crash (4.7, §5.7).
        let lag = identity.revoke_on_teardown(revoker, resume_at_secs, resume_at_secs);
        telemetry.record_revoke(lag);
        Ok(jti)
    }

    /// **Drive ONE SKELETON run end-to-end on the real substrate, CHAINING the operations (8.5,
    /// §5.1).** This is the chained-e2e path (EI-01 §4 — real sessions chain mutations, never a single
    /// handler call): deliver → mint → reserve → step → trace → settle → revoke. `kill` injects the
    /// AG-D8 mid-flight kill (the failure-injection harness): when `RunOutcomeKind::KilledMidFlight`
    /// the run is killed AFTER dispatch but BEFORE it submits — the teardown STILL revokes the token.
    ///
    /// Returns the [`RunOutcome`] (the platform-owned loop outcome, 8.5) with the trace ref + the
    /// settle outcome carried as a machine string (references-not-payloads), or a [`SkeletonError`]
    /// (a refused dispatch / a failed mint / a co-commit failure) — surfaced LOUD. The token is torn
    /// down on EVERY path (completed, killed, or errored) — the teardown is unconditional.
    pub fn handle_run(
        &self,
        runtime: &dyn MeteredRuntime,
        sub: &mut RunSubstrate<'_>,
        telemetry: &mut SkeletonTelemetry,
        kill: RunOutcomeKind,
    ) -> Result<RunOutcome, SkeletonError> {
        // (1) MINT a per-run attenuated token (4.7). Token life == run life. The mint is the ONLY
        //     token path — the SKELETON never fabricates a token, it always asks Identity (via the
        //     RunTokenMinter seam). The caveats are ATTENUATED per-run (a `run:<id>` caveat naming
        //     THIS run, attenuate-only). A failed mint aborts BEFORE any reserve (never run un-minted).
        let mut caveats = sub.caveats.clone();
        caveats.0.push(format!("run:{}", sub.run_id));
        let token: RunTokenHandle = sub
            .minter_token
            .mint_run_token(&sub.agent_id, &sub.run_id, &caveats, sub.token_ttl_secs)
            .map_err(SkeletonError::from)?;
        // The teardown instant is the run's life-end; the revoke must land within the bound from here.
        let teardown_at = sub.now_secs;

        // (2) RESERVE-at-dispatch (11.7). No balance → no run. On refusal the run is NEVER started —
        //     but the just-minted token is STILL torn down (the teardown is unconditional). The gate
        //     is correct-by-construction: a run cannot dispatch without going through reserve.
        let storage_run = StorageRunId::new(sub.run_id.clone());
        let in_flight = match sub.gate.dispatch(
            sub.ledger,
            sub.tenant.clone(),
            storage_run.clone(),
            sub.estimate,
            sub.available,
        ) {
            Ok(h) => h,
            Err(e) => {
                // Refused: tear down the (un-dispatched) token, then surface the refusal LOUD.
                self.teardown(sub, &token, teardown_at, telemetry);
                return Err(e.into());
            }
        };
        telemetry.record_reserve(in_flight.reserved().0);

        // The child environment a tool would inherit — minted from the per-run token ONLY (§5.7,
        // anti-leak). Built here so it exists the moment the run is in-flight (a tool could spawn at
        // any step); the AG-D8 drill reads it. NO shared platform token leaks in (0 leak).
        let _child_env = ChildEnv::for_run(&token.jti);

        // (3)+(4)+(5) Drive the durable-workflow body: build the EMPTY conversation, step the brain
        //     (an activity), write the trace (a journaled+co-committed activity carrying nested
        //     causality). One WfCtx co-commit (§5.6) — the trace journal row + its emit are atomic.
        let ctx_base = EmitContextBase {
            tenant: sub.tenant.clone(),
            region: sub.region.clone(),
            actor: Actor(sub.agent.clone()),
            schema_ver: 1,
            occurred_at: Timestamp(format!("skeleton-now:{}", sub.now_secs)),
            recorded_at: Timestamp(format!("skeleton-now:{}", sub.now_secs)),
            caused_by: None,
        };
        let mut ctx = WfCtx::begin(
            sub.outbox,
            sub.minter.clone(),
            sub.journal.clone(),
            ctx_base,
            sub.run_id.clone(),
            "agent.run",
            format!("skeleton-now:{}", sub.now_secs),
            /* rand_seed */ 0,
        );

        // (3) BUILD the conversation — EMPTY at the start (no trace history). The loop appends the
        //     brain's model steps + the tool results it routes so a stateful brain (the mock/LLM,
        //     which reads its own prior turns to know its position) advances; the SKELETON submits on
        //     turn 0 so the loop body is never entered. The from-trace build (AG-1) + the
        //     delegation-scoped tool subset (AG-P7) are the named floors.
        let mut conv = Conversation::default();

        // (4) DRIVE the bounded multi-turn loop (§2.3/§5.1 — build_conversation → step → route →
        //     append → step again). The ONE platform-owned loop, identical for mock and real; only the
        //     brain's decision differs. Each turn steps the brain and, per outcome:
        //       - Submit   → terminal: break with the submission (the SKELETON's single turn ends here,
        //         so its behaviour is UNCHANGED — it never enters the UseTools body);
        //       - UseTools → the §5.0 routing point: VALIDATE each call (the security checkpoint —
        //         fail-closed on Err; the untrusted model arguments are NEVER dispatched), EXECUTE it
        //         through the ToolExecutor seam, append the results to the conversation, and step again.
        //     The loop is BOUNDED by DEFAULT_MAX_TURNS (the runaway guard for this slice — NOT
        //     metering; the per-call cost meter + spend cap is a decision-gated follow-on, and the
        //     reserve/settle gate is untouched). The teardown is UNCONDITIONAL on every early-exit
        //     path below (validation abort, executor error, max-turns exhaustion) exactly as on the
        //     completed / killed / refused paths.
        let mut submission: Option<Submission> = None;
        for _turn in 0..DEFAULT_MAX_TURNS {
            // Step the brain OBSERVABLY: the decision PLUS its raw token usage. Accumulate the usage
            // into the run telemetry (a NEW, separate observability signal — NON-FINANCIAL: no
            // pricing, no reserve/settle touch). The SKELETON/Mock report NotReported, so their token
            // totals stay 0 and the balanced-ledger assertions are unchanged.
            let MeteredStep { outcome, usage } = runtime.step_metered(&conv);
            telemetry.record_token_usage(&usage);
            match outcome {
                StepOutcome::Submit(s) => {
                    // Terminal — record the model step into the transcript, then fall through to the
                    // (unchanged) trace → settle → teardown chain below.
                    conv.turns.push(Turn::Model(StepOutcome::Submit(s.clone())));
                    submission = Some(s);
                    break;
                }
                StepOutcome::UseTools(calls) => {
                    // Advance the transcript with the brain's decision (a stateful brain reads its own
                    // prior model turns to know its position — the platform owns history, §2.1).
                    conv.turns
                        .push(Turn::Model(StepOutcome::UseTools(calls.clone())));
                    let mut outcomes: Vec<ToolOutcome> = Vec::with_capacity(calls.len());
                    for call in &calls {
                        // THE SECURITY CHECKPOINT (fail-closed): an unregistered tool, or arguments
                        // that don't satisfy the tool's schema, ABORT the run — the untrusted args are
                        // NEVER handed to a tool (§2.1). The teardown still fires (unconditional).
                        if let Err(reason) = validate_call(sub.catalogue, call) {
                            drop(ctx); // abandon the co-commit — 0 ghost trace, 0 lost emit.
                            self.teardown(sub, &token, teardown_at, telemetry);
                            return Err(SkeletonError::ToolValidationRejected(reason));
                        }
                        // Validated ⇒ the tool resolves; hand its ToolDef + the call to the executor.
                        // Resolve defensively (fail-closed, never panic) even though validation proved
                        // it registered.
                        let def = match sub.catalogue.resolve(&call.name) {
                            Some(def) => def,
                            None => {
                                drop(ctx);
                                self.teardown(sub, &token, teardown_at, telemetry);
                                return Err(SkeletonError::ToolValidationRejected(format!(
                                    "tool `{}` vanished from the catalogue after validation",
                                    call.name.0
                                )));
                            }
                        };
                        match sub.executor.execute(def, call) {
                            Ok(result) => outcomes.push(ToolOutcome {
                                call_id: call.id.clone(),
                                result,
                            }),
                            Err(e) => {
                                drop(ctx);
                                self.teardown(sub, &token, teardown_at, telemetry);
                                return Err(SkeletonError::ToolExecFailed(e.to_string()));
                            }
                        }
                    }
                    // Append the routed tool results so the next step sees each keyed to its call id,
                    // then continue the loop → step again.
                    conv.turns.push(Turn::ToolResults(outcomes));
                }
            }
        }

        // The bounded guard tripped: the brain never submitted within DEFAULT_MAX_TURNS. Terminate
        // GRACEFULLY — abandon the co-commit (0 ghost trace), tear down the token (unconditional), and
        // surface the exhaustion LOUD. The reservation is left in-flight (the never-interrupt
        // invariant, exactly as on a mid-flight kill). Never an unbounded loop, never a panic.
        let submission = match submission {
            Some(s) => s,
            None => {
                drop(ctx);
                self.teardown(sub, &token, teardown_at, telemetry);
                return Err(SkeletonError::MaxTurnsExhausted {
                    run_id: sub.run_id.clone(),
                    turns: DEFAULT_MAX_TURNS,
                });
            }
        };

        // (AG-D8) KILL mid-flight: the failure-injection harness kills the run AFTER dispatch but
        // BEFORE the trace/settle. The WfCtx is DROPPED without commit (so neither the trace journal
        // row nor its emit becomes durable — emit-iff-committed, BUS-D4). The teardown STILL revokes
        // the token (the no-tool leg of AG-D8). The reservation is NEVER interrupted (the run is
        // in-flight; the only exit is settle — but a killed SKELETON leaves it reserved-not-settled,
        // which is the never-interrupt invariant working: the gate has no tear-down-in-flight API).
        if kill == RunOutcomeKind::KilledMidFlight {
            drop(ctx); // the co-commit transaction is abandoned — 0 ghost trace, 0 lost emit.
            self.teardown(sub, &token, teardown_at, telemetry);
            telemetry.runs_killed = telemetry.runs_killed.saturating_add(1);
            return Ok(RunOutcome(format!(
                "killed-mid-flight: run={} token-revoked (no trace, reservation left in-flight)",
                sub.run_id
            )));
        }

        // (5) WRITE the (near-empty) trace row as a journaled + co-committed activity, then emit the
        //     terminal `agent.run.traced` event carrying the trace ref with NESTED causality (2.2).
        //     The activity journals one wf_history row; the emit stages into the SAME OutboxTx; the
        //     commit makes BOTH durable atomically (FLOW-D5 — 0 ghost, 0 lost).
        let trace_ref = format!("myelin://{}/agent/trace/{}", sub.tenant.0, sub.run_id);
        let trace_artifact = ArtifactRef(trace_ref.clone());
        ctx.activity(RetryPolicy::default_policy(), {
            let tr = trace_artifact.clone();
            move |_id: &str, _attempt: u32| Ok(vec![tr.clone()])
        })
        .map_err(|e| SkeletonError::CoCommit(format!("{e:?}")))?;

        // The terminal trace event — references-not-payloads (the reasoning body stays in the erasable
        // trace holder; the event carries only the ArtifactRef). Causality is NESTED via emit(draft,
        // cause): a root SKELETON run has no incoming envelope, so cause = None (the dispatch tier
        // supplies the incoming envelope in the wired path; here the chained-e2e proves the emit seam).
        let draft = EventDraft {
            type_: EventType(AGENT_RUN_TRACED_EVENT.into()),
            subject: ArtifactRef(trace_ref.clone()),
            aggregate: AggregateKey(format!("run:{}", sub.run_id)),
            payload: serde_json::json!({ "trace_ref": trace_ref }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        ctx.emit(draft, None)
            .map_err(|e| SkeletonError::CoCommit(format!("{e:?}")))?;

        // CO-COMMIT: the trace journal row + the terminal emit become durable TOGETHER (or neither).
        ctx.commit()
            .map_err(|e| SkeletonError::CoCommit(format!("{e:?}")))?;
        telemetry.record_trace();

        // (6) SETTLE the reservation (11.7). A SKELETON bills ZERO units (no model call) — it settles
        //     with an EMPTY unit slice, so billed == 0 and the WHOLE reservation is refunded. The
        //     ledger is BALANCED: reserved == settled (billed + refunded). The settle is idempotent.
        let settle = in_flight
            .settle(sub.ledger, &[])
            .expect("a freshly-reserved in-flight run always settles (it was reserved this run)");
        // settled == billed_total + refunded == the reservation (the balanced-ledger gate).
        let settled_total = settle.billed_total.0.saturating_add(settle.refunded.0);
        telemetry.record_settle(settled_total);

        // (7) TEARDOWN: revoke the per-run token idempotently (even on crash), belt-and-suspenders
        //     with the auto-expiring TTL (§5.7). The teardown is unconditional — it fires on the
        //     completed path here exactly as it fired on the killed/errored paths above.
        self.teardown(sub, &token, teardown_at, telemetry);
        telemetry.runs_completed = telemetry.runs_completed.saturating_add(1);

        let _ = submission; // the SKELETON's submission is content-free; the trace is the artifact.
        Ok(RunOutcome(format!(
            "completed: run={} trace={} reserved={} settled={} token-revoked",
            sub.run_id,
            trace_ref,
            in_flight.reserved().0,
            settled_total
        )))
    }

    /// **The teardown: revoke the per-run token idempotently (4.7, §5.7).** Unconditional — fires on
    /// every run-exit path (completed / killed / errored). Records the revocation lag into the
    /// telemetry (the AG-D8 within-bound signal). Idempotent even on crash: a double-teardown (the
    /// explicit revoke + a crash sweep) is a no-op success (lag 0 on the re-revoke).
    fn teardown(
        &self,
        sub: &RunSubstrate<'_>,
        token: &RunTokenHandle,
        teardown_at: i64,
        telemetry: &mut SkeletonTelemetry,
    ) {
        let lag = sub.revoker.revoke(&token.jti, sub.now_secs, teardown_at);
        telemetry.record_revoke(lag);
    }
}

impl Default for SkeletonAgent {
    fn default() -> Self {
        SkeletonAgent::new()
    }
}

/// **8.5 — the frozen `Agent::handle(InboxEvent, &dyn AgentRuntime) -> RunOutcome` shape, owned.**
/// The trait body. This is the signature seam the dispatch tier (3.6) calls; the rich substrate-
/// chaining driver is [`SkeletonAgent::handle_run`] (which the wired path builds the [`RunSubstrate`]
/// for from the delivered event). `handle` here proves the frozen 8.5 shape is implemented by the
/// SKELETON loop; a call with no substrate drives the brain through the `&dyn` seam (the strategy
/// boundary) and returns the loop outcome.
impl Agent for SkeletonAgent {
    fn handle(&self, inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome {
        // The frozen-shape leg: drive the brain through the seam (the bounded loop's first step). The
        // full chained substrate path (mint/reserve/trace/settle/revoke) is handle_run, which the
        // wired dispatch consumer calls with a RunSubstrate built from `inbox`. A SKELETON step is
        // terminal (immediate Submit), so the bounded loop is one turn.
        let _ = runtime.step(&Conversation::default());
        RunOutcome(format!(
            "skeleton handle: delivered={} (chained path → handle_run)",
            inbox.0
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_exec::{MockToolExecutor, MockToolSurface, ToolExecError};
    use myelin_agent::{EffectKind, ToolCall, ToolCallId, ToolDef, ToolName, ToolResult};
    use myelin_identity::{PrincipalId, PrincipalKind};
    use std::sync::Arc;

    // ───────── a deterministic mint/revoke seam fake (a REAL impl on the consumed surface) ─────────

    /// A deterministic [`RunTokenMinter`] — mints a fresh `jti` per `(agent, run)`, under the lease's
    /// short TTL (token life == run life). It is a REAL impl on the contract-4.7 surface (the CDC
    /// pairs the engine consumer with the Identity provider); here it proves the SKELETON drives the
    /// mint seam (it never fabricates a token).
    #[derive(Default)]
    struct FakeMinter;
    impl RunTokenMinter for FakeMinter {
        fn mint_run_token(
            &self,
            agent_id: &str,
            run_id: &str,
            caveats: &myelin_flow::DelegationCaveats,
            ttl_secs: u64,
        ) -> Result<RunTokenHandle, RunTokenError> {
            // The jti is bound to (agent, run) — the token's life IS the run's life. The caveats carry
            // the per-run attenuation the lease added (run:<id>).
            assert!(
                caveats.0.iter().any(|c| c == &format!("run:{run_id}")),
                "the mint must carry the per-run attenuation caveat"
            );
            Ok(RunTokenHandle {
                token: format!("tok:{agent_id}:{run_id}"),
                jti: format!("jti:{agent_id}:{run_id}"),
                ttl_secs,
            })
        }
    }

    /// A deterministic [`RunTokenRevoker`] over a denylist + the token TTL — proves revoke-on-teardown
    /// (idempotent even on crash) AND auto-expiry. A REAL impl on the contract-4.7 revoke surface.
    #[derive(Default)]
    struct FakeRevoker {
        revoked: std::sync::Mutex<std::collections::HashMap<String, i64>>,
        /// the TTL window W (seconds) the token auto-expires after its mint instant.
        ttl_w: i64,
        /// the mint instant (epoch-seconds) the auto-expiry is measured from.
        minted_at: i64,
    }
    impl RunTokenRevoker for FakeRevoker {
        fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
            let mut g = self.revoked.lock().unwrap();
            if g.contains_key(jti) {
                return 0; // idempotent even on crash: a re-revoke is a no-op (lag 0).
            }
            g.insert(jti.to_string(), now_secs);
            // the revocation lag: seconds between the teardown instant and the revoke landing.
            (now_secs - teardown_secs).max(0) as u64
        }
        fn is_dead(&self, jti: &str, now_secs: i64) -> bool {
            // dead == revoked (explicit) OR auto-expired (now >= minted_at + W).
            self.revoked.lock().unwrap().contains_key(jti)
                || now_secs >= self.minted_at + self.ttl_w
        }
    }

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn agent() -> Principal {
        Principal::stub(
            PrincipalId("psn:agent-7".into()),
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("skeleton".into()),
                on_behalf_of: None,
            },
            tenant(),
        )
    }

    /// Build a [`RunSubstrate`] for a run (the dispatch tier builds this from the delivered event).
    /// `catalogue` + `executor` are the driving loop's tool seams — an EMPTY catalogue + a no-op
    /// executor for a SKELETON run (it proposes no tools); a tool-driving test passes a seeded
    /// catalogue + a scripted [`MockToolExecutor`].
    #[allow(clippy::too_many_arguments)]
    fn substrate<'a>(
        run_id: &str,
        revoker: &'a FakeRevoker,
        catalogue: &'a dyn ToolSurface,
        executor: &'a dyn ToolExecutor,
        gate: &'a mut AgentRunGate,
        ledger: &'a mut CostLedger,
        outbox: &'a myelin_events::OutboxStore,
        available: u64,
        estimate: u64,
        now_secs: i64,
    ) -> RunSubstrate<'a> {
        RunSubstrate {
            tenant: tenant(),
            region: region(),
            agent: agent(),
            run_id: run_id.into(),
            minter_token: Arc::new(FakeMinter),
            agent_id: "psn:agent-7".into(),
            caveats: DelegationCaveats(vec!["delegated:human-x".into()]),
            token_ttl_secs: 300,
            revoker,
            catalogue,
            executor,
            gate,
            ledger,
            available: MinorUnits(available),
            estimate: MinorUnits(estimate),
            outbox,
            minter: Arc::new(myelin_events::MonotonicMinter::new()),
            journal: WfJournal::new(),
            now_secs,
        }
    }

    /// **8.3 — the SKELETON runtime submits immediately (no model, no tools).** `step` returns a
    /// frozen-shape `Submit` decision; it never returns `UseTools` (the SKELETON has no tools).
    #[test]
    fn skeleton_runtime_submits_immediately() {
        let rt = SkeletonAgentRuntime::new();
        assert!(matches!(
            rt.step(&Conversation::default()),
            StepOutcome::Submit(_)
        ));
    }

    /// **8.5 — the chained-e2e SKELETON path: deliver → mint → reserve → step → trace → settle →
    /// revoke (EI-01 §4 — real sessions CHAIN mutations).** One run drives the WHOLE substrate path
    /// at zero cost: a trace row is written, the ledger is balanced (reserved == settled), and the
    /// run principal == the per-run token's principal (attribution intact). Telemetry signals emit.
    #[test]
    fn skeleton_chains_the_whole_substrate_path() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        // The SKELETON registers no tools: an EMPTY catalogue + a no-op executor (never entered).
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "R1",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            /* avail */ 100,
            /* est */ 10,
            /* now */ 1000,
        );

        let out = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the SKELETON chain completes");
        assert!(
            out.0.contains("completed"),
            "the run completed the chain: {out:?}"
        );

        // one trace row written (the trace_ref is non-empty).
        assert_eq!(tele.traces_written(), 1, "exactly one trace row written");
        // the ledger is BALANCED: reserved == settled (a SKELETON bills 0, refunds the reservation).
        assert!(
            tele.ledger_balanced(),
            "reserved {} == settled {}",
            tele.reserved(),
            tele.settled()
        );
        assert_eq!(tele.reserved(), 10, "reserved the estimate");
        assert_eq!(
            tele.settled(),
            10,
            "settled (billed 0 + refunded 10) == reserved"
        );
        // the token was revoked on teardown (attribution closed).
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "the per-run token revoked on teardown"
        );
        assert_eq!(tele.runs_completed(), 1);
        assert_eq!(tele.runs_killed(), 0);
        // ATTRIBUTION: the run principal == the per-run token's principal (the jti is bound to the
        // agent the run acts as). The minter binds jti to (agent_id, run_id) — the run's identity.
        let mut caveats = sub.caveats.clone();
        caveats.0.push("run:R1".into());
        let token = sub
            .minter_token
            .mint_run_token(&sub.agent_id, "R1", &caveats, sub.token_ttl_secs)
            .unwrap();
        assert_eq!(
            token.jti, "jti:psn:agent-7:R1",
            "the token jti is bound to (agent, run)"
        );
        assert_eq!(
            sub.agent_id, "psn:agent-7",
            "the run's agent principal == the token's principal"
        );
    }

    /// **AG-D8 (no-tool leg) — kill the run mid-flight → the per-run token is revoked on teardown AND
    /// auto-expires ≤ W; 0 shared platform token leaked into the child env.** The headline drill: a
    /// killed run still tears down its token; the child env inherits NO shared platform token.
    #[test]
    fn ag_d8_killed_run_revokes_token_and_leaks_nothing() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let w = 300i64;
        let minted_at = 1000i64;
        let revoker = FakeRevoker {
            ttl_w: w,
            minted_at,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "R2",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            minted_at,
        );

        let out = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::KilledMidFlight)
            .expect("a killed run still tears down cleanly");
        assert!(
            out.0.contains("killed-mid-flight"),
            "the run was killed: {out:?}"
        );

        // the per-run token was revoked on teardown (even though the run never completed).
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "killed run STILL revoked its token on teardown"
        );
        assert_eq!(tele.runs_killed(), 1);
        // 0 trace written + reservation left in-flight (never interrupted — the gate has no
        // tear-down-in-flight API; the run was killed, the reservation was NOT settled).
        assert_eq!(
            tele.traces_written(),
            0,
            "a killed run wrote no trace (0 ghost — co-commit abandoned)"
        );

        // the token is DEAD: revoked-on-teardown (now) AND auto-expires by minted_at + W.
        let jti = "jti:psn:agent-7:R2";
        assert!(
            revoker.is_dead(jti, minted_at),
            "revoked-on-teardown → dead now"
        );
        // even ABSENT the explicit revoke, it auto-expires ≤ W (belt-and-suspenders).
        let fresh = FakeRevoker {
            ttl_w: w,
            minted_at,
            ..Default::default()
        };
        assert!(!fresh.is_dead(jti, minted_at), "not yet expired before W");
        assert!(
            fresh.is_dead(jti, minted_at + w),
            "auto-expires by minted_at + W (≤ W window)"
        );

        // 0 SHARED token leaked into the child env (the anti-leak unset, §5.7).
        let child = ChildEnv::for_run(jti);
        assert!(
            !child.leaked_shared_token(),
            "0 shared platform token leaked into the child env"
        );
        assert_eq!(
            child.shared_platform_token, None,
            "the child env's shared-token slot is UNSET"
        );
        assert_eq!(
            child.run_token_jti, jti,
            "the child's ONLY credential is the per-run jti"
        );
        // the revocation lag is within bound (teardown == now in this run → lag 0 ≤ W).
        assert!(
            tele.max_revocation_lag() <= w as u64,
            "revocation lag within bound W"
        );
    }

    /// **No balance → no run (11.7 / AG-D11).** A dispatch against an exhausted wallet is REFUSED;
    /// the run never starts (no trace, no settle) — but the minted token is STILL torn down (the
    /// teardown is unconditional). The refusal surfaces LOUD.
    #[test]
    fn no_balance_no_run_but_token_still_torn_down() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        // available (1) < estimate (10) → no balance, no run.
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "R3", &revoker, &cat, &exec, &mut gate, &mut ledger, &outbox, 1, 10, 1000,
        );

        let err = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("an unfunded dispatch is refused");
        assert!(
            matches!(err, SkeletonError::DispatchRefused(_)),
            "no balance → no run: {err}"
        );
        // the run never started: 0 trace, 0 reserve recorded, 0 settle.
        assert_eq!(tele.traces_written(), 0);
        assert_eq!(
            tele.reserved(),
            0,
            "nothing reserved (the reserve was refused)"
        );
        assert_eq!(tele.settled(), 0);
        // but the token was STILL torn down (the teardown is unconditional).
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "the minted token is torn down even on a refused dispatch"
        );
        assert_eq!(
            gate.reserve_refusals(),
            1,
            "the gate counted the refusal (AG-D11 telemetry)"
        );
    }

    /// **The frozen 8.5 `Agent::handle` shape drives the brain through the `&dyn` seam.** The trait
    /// body (the dispatch tier's entry) drives one step through the strategy boundary and returns the
    /// loop outcome; the rich chained path is `handle_run`.
    #[test]
    fn agent_handle_frozen_shape_drives_the_seam() {
        let agent_loop = SkeletonAgent::new();
        let rt = SkeletonAgentRuntime::new();
        let out = agent_loop.handle(InboxEvent("issue.created".into()), &rt);
        assert!(
            out.0.contains("skeleton handle"),
            "the frozen 8.5 shape returns the loop outcome"
        );
    }

    /// **The balanced-ledger predicate is exact (mutation-floor support).** reserved == settled is
    /// the gate; a mutant that drops the settle (or double-counts the reserve) flips it.
    #[test]
    fn ledger_balanced_predicate_is_exact() {
        let mut t = SkeletonTelemetry::new();
        assert!(t.ledger_balanced(), "an empty ledger is balanced");
        t.record_reserve(10);
        assert!(!t.ledger_balanced(), "reserved-not-settled is UNbalanced");
        t.record_settle(10);
        assert!(t.ledger_balanced(), "reserved == settled is balanced");
    }

    /// **The contract-1.8 signal accessors are exact (mutation-floor — the signals ARE the green
    /// artifacts, §3.1; a mutant that fixes an accessor to a constant must be killed).** Each
    /// accessor returns the recorded value, distinguishably across values (so a `-> 0`/`-> 1`
    /// constant mutant flips an assertion).
    #[test]
    fn telemetry_signal_accessors_are_exact() {
        let mut t = SkeletonTelemetry::new();
        // zero state: every signal reads 0 (kills a `-> 1` constant mutant).
        assert_eq!(t.tokens_revoked(), 0);
        assert_eq!(t.runs_completed(), 0);
        assert_eq!(t.runs_killed(), 0);
        assert_eq!(t.max_revocation_lag(), 0);
        assert_eq!(t.traces_written(), 0);

        // record DISTINCT non-zero values so each accessor must return ITS field (kills `-> 0`).
        t.record_revoke(7); // tokens_revoked → 1, max_revocation_lag → 7.
        t.record_revoke(3); // a SMALLER lag must NOT lower the max (kills the `>` → `<`/`==`/`>=` mutants).
        t.record_revoke(9); // a LARGER lag must raise the max.
        t.record_trace();
        t.runs_completed = 2;
        t.runs_killed = 5;
        assert_eq!(
            t.tokens_revoked(),
            3,
            "tokens_revoked counts every revoke (kills -> 1)"
        );
        assert_eq!(
            t.max_revocation_lag(),
            9,
            "max lag is the MAXIMUM (7, then 3 ignored, then 9)"
        );
        assert_eq!(t.traces_written(), 1, "traces_written counts each trace");
        assert_eq!(
            t.runs_completed(),
            2,
            "runs_completed returns its field (kills -> 1)"
        );
        assert_eq!(t.runs_killed(), 5, "runs_killed returns its field");
        assert_eq!(
            t.reserved(),
            0,
            "reserved is independent (kills cross-field constant mutants)"
        );
    }

    /// **`record_revoke` keeps the MAX lag (kills the `>` comparator mutants).** A larger lag raises
    /// the max; a smaller one does not lower it; an equal one is a no-op — the exact `>` semantics.
    #[test]
    fn record_revoke_keeps_the_maximum_lag() {
        let mut t = SkeletonTelemetry::new();
        t.record_revoke(5);
        assert_eq!(t.max_revocation_lag(), 5);
        t.record_revoke(5); // equal → no change (kills `>=`, which would still set; and `==`).
        assert_eq!(t.max_revocation_lag(), 5);
        t.record_revoke(2); // smaller → no change (kills `<`).
        assert_eq!(t.max_revocation_lag(), 5);
        t.record_revoke(8); // larger → raise (kills `==`).
        assert_eq!(t.max_revocation_lag(), 8);
    }

    /// **`ChildEnv::leaked_shared_token` is exact (kills the `-> false` mutant — the AG-D8 anti-leak
    /// headline).** A child env with NO shared token does not leak; one WITH a shared token DOES.
    #[test]
    fn child_env_leak_predicate_is_exact() {
        let clean = ChildEnv::for_run("jti:R1");
        assert!(
            !clean.leaked_shared_token(),
            "a clean child env does not leak (the anti-leak unset)"
        );
        // a (hypothetical) leaked env: leaked_shared_token must return TRUE (kills `-> false`).
        let leaked = ChildEnv {
            run_token_jti: "jti:R1".into(),
            shared_platform_token: Some("PLATFORM-TOKEN".into()),
        };
        assert!(
            leaked.leaked_shared_token(),
            "a leaked shared token IS a leak (kills -> false)"
        );
    }

    /// **`SkeletonError` Display is non-empty + distinct per variant (kills the `fmt -> Ok(default)`
    /// mutant — a swallowed error message is a silent failure, EI-01 §2).** Each variant renders its
    /// machine reason loudly.
    #[test]
    fn skeleton_error_display_is_loud_and_distinct() {
        let refused = SkeletonError::DispatchRefused("no balance".into()).to_string();
        let mint = SkeletonError::MintFailed("id down".into()).to_string();
        let cc = SkeletonError::CoCommit("journal".into()).to_string();
        assert!(
            refused.contains("dispatch refused"),
            "Display renders the refusal: {refused}"
        );
        assert!(
            mint.contains("mint failed"),
            "Display renders the mint failure: {mint}"
        );
        assert!(
            cc.contains("co-commit failed"),
            "Display renders the co-commit failure: {cc}"
        );
        let val = SkeletonError::ToolValidationRejected("bad args".into()).to_string();
        let exec = SkeletonError::ToolExecFailed("subsystem down".into()).to_string();
        let maxt = SkeletonError::MaxTurnsExhausted {
            run_id: "Rx".into(),
            turns: 16,
        }
        .to_string();
        assert!(
            val.contains("validation rejected"),
            "Display renders the validation rejection: {val}"
        );
        assert!(
            exec.contains("tool execution failed"),
            "Display renders the executor failure: {exec}"
        );
        assert!(
            maxt.contains("max_turns=16"),
            "Display renders the max-turns exhaustion: {maxt}"
        );
        assert_ne!(refused, mint);
        assert_ne!(mint, cc);
        assert_ne!(val, exec);
        assert_ne!(exec, maxt);
        assert!(
            !refused.is_empty(),
            "the error message is non-empty (kills fmt -> Ok(default))"
        );
    }

    // ───────── the bounded driving loop (the ToolExecutor seam + validate → execute → append) ────

    /// A permissive [`ToolDef`] (empty schema — any args validate) for a tool-driving test.
    fn tool_def(name: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "test".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Read,
            side_effecting: false,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }

    /// A tool call with a deterministic id + empty-object args (satisfies the permissive schema).
    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(format!("call:{name}")),
            name: ToolName(name.into()),
            arguments: serde_json::json!({}),
        }
    }

    /// **The driving loop runs N tool turns then submits: the executor sees the VALIDATED calls, the
    /// conversation accumulates the ToolOutcomes, and the run settles + tears down.** A brain that
    /// drives `search → read → submit` (two tool turns) proves the loop body — validate → execute →
    /// append → step again — and that the completed chain (trace, balanced ledger, revoke) is intact.
    #[test]
    fn loop_drives_tool_turns_then_submits() {
        // A brain that inspects the platform-owned transcript: it records how many `ToolResults` turns
        // it can see at each step (proving the loop appended them) and drives search → read → submit.
        #[derive(Default)]
        struct DriveBrain {
            tool_result_turns_seen: std::sync::Mutex<Vec<usize>>,
            outcomes_at_submit: std::sync::Mutex<Vec<ToolOutcome>>,
        }
        impl AgentRuntime for DriveBrain {
            fn step(&self, conv: &Conversation) -> StepOutcome {
                let model_turns = conv
                    .turns
                    .iter()
                    .filter(|t| matches!(t, Turn::Model(_)))
                    .count();
                let tr_turns = conv
                    .turns
                    .iter()
                    .filter(|t| matches!(t, Turn::ToolResults(_)))
                    .count();
                self.tool_result_turns_seen.lock().unwrap().push(tr_turns);
                match model_turns {
                    0 => StepOutcome::UseTools(vec![tool_call("search")]),
                    1 => StepOutcome::UseTools(vec![tool_call("read")]),
                    _ => {
                        // At the submit step, snapshot the accumulated tool outcomes (proves the loop
                        // threaded each executor result back into the conversation).
                        let outcomes: Vec<ToolOutcome> = conv
                            .turns
                            .iter()
                            .flat_map(|t| match t {
                                Turn::ToolResults(rs) => rs.clone(),
                                _ => Vec::new(),
                            })
                            .collect();
                        *self.outcomes_at_submit.lock().unwrap() = outcomes;
                        StepOutcome::Submit(Submission("done".into()))
                    }
                }
            }
        }
        impl MeteredRuntime for DriveBrain {}

        let brain = DriveBrain::default();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rtools", &revoker, &cat, &exec, &mut gate, &mut ledger, &outbox, 100, 10, 1000,
        );

        let out = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the tool-driving run completes");
        assert!(out.0.contains("completed"), "the run completed: {out:?}");

        // THE EXECUTOR SAW THE VALIDATED CALLS, in order (search then read).
        assert_eq!(exec.call_count(), 2, "one execute per tool turn");
        let seen = exec.calls();
        assert_eq!(seen[0].name, ToolName("search".into()));
        assert_eq!(seen[1].name, ToolName("read".into()));

        // THE CONVERSATION ACCUMULATED THE ToolOutcomes: the brain saw 0 tool-results turns at step 0,
        // 1 at step 1, 2 at the submit step (each UseTools appended exactly one ToolResults turn).
        assert_eq!(
            *brain.tool_result_turns_seen.lock().unwrap(),
            vec![0, 1, 2],
            "each tool turn appended a ToolResults turn the next step reads"
        );
        let submit_outcomes = brain.outcomes_at_submit.lock().unwrap().clone();
        assert_eq!(submit_outcomes.len(), 2, "both tool round-trips accumulated");
        assert_eq!(submit_outcomes[0].call_id, ToolCallId("call:search".into()));
        assert_eq!(
            submit_outcomes[0].result,
            ToolResult("mock-exec:search:ok".into()),
            "the executor's result was threaded back, keyed to its call"
        );
        assert_eq!(submit_outcomes[1].call_id, ToolCallId("call:read".into()));

        // THE RUN SETTLED + TORE DOWN: one trace, balanced ledger, token revoked, run completed.
        assert_eq!(tele.traces_written(), 1);
        assert!(tele.ledger_balanced(), "reserved == settled");
        assert_eq!(tele.tokens_revoked(), 1, "torn down on the completed path");
        assert_eq!(tele.runs_completed(), 1);
    }

    /// A brain that ALWAYS proposes one (registered) tool and NEVER submits — drives the loop to its
    /// max-turns ceiling.
    struct AlwaysUseTool(ToolName);
    impl AgentRuntime for AlwaysUseTool {
        fn step(&self, _conv: &Conversation) -> StepOutcome {
            StepOutcome::UseTools(vec![ToolCall {
                id: ToolCallId("c".into()),
                name: self.0.clone(),
                arguments: serde_json::json!({}),
            }])
        }
    }
    impl MeteredRuntime for AlwaysUseTool {}

    /// **A run that never submits terminates GRACEFULLY at max_turns (the runaway guard) — bounded, no
    /// panic, teardown still fires.** After exactly [`DEFAULT_MAX_TURNS`] executed tool turns the loop
    /// returns [`SkeletonError::MaxTurnsExhausted`]; the token is revoked, and no trace is written.
    #[test]
    fn loop_hits_max_turns_and_terminates_gracefully() {
        let brain = AlwaysUseTool(ToolName("loop".into()));
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("loop")]);
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rmax", &revoker, &cat, &exec, &mut gate, &mut ledger, &outbox, 100, 10, 1000,
        );

        let err = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("a never-submitting brain trips the bounded ceiling");
        match err {
            SkeletonError::MaxTurnsExhausted { run_id, turns } => {
                assert_eq!(run_id, "Rmax");
                assert_eq!(turns, DEFAULT_MAX_TURNS, "the exact ceiling that tripped");
            }
            other => panic!("expected MaxTurnsExhausted, got {other:?}"),
        }
        // it ran EXACTLY max_turns tool executions, then stopped (bounded — never hung).
        assert_eq!(
            exec.call_count(),
            DEFAULT_MAX_TURNS,
            "one execute per bounded turn, then graceful termination"
        );
        // the teardown STILL fired (unconditional), and no trace was written (0 ghost).
        assert_eq!(tele.tokens_revoked(), 1, "torn down on the max-turns path");
        assert_eq!(tele.traces_written(), 0, "no trace on an exhausted run");
        assert_eq!(tele.runs_completed(), 0);
        assert_eq!(tele.runs_killed(), 0);
    }

    /// **A tool call that FAILS `validate_call` aborts the run FAIL-CLOSED — no dispatch, teardown
    /// still fires.** The brain proposes an UNREGISTERED tool against the (empty) catalogue; the
    /// security checkpoint rejects it BEFORE the executor is ever called, the run aborts with
    /// [`SkeletonError::ToolValidationRejected`], and the token is torn down.
    #[test]
    fn loop_validation_failure_aborts_fail_closed_without_dispatch() {
        let brain = AlwaysUseTool(ToolName("ghost".into())); // never registered
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        // EMPTY catalogue → the proposed `ghost` tool is unregistered → validate_call fails.
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rbad", &revoker, &cat, &exec, &mut gate, &mut ledger, &outbox, 100, 10, 1000,
        );

        let err = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("an unvalidated tool call aborts the run");
        assert!(
            matches!(err, SkeletonError::ToolValidationRejected(_)),
            "fail-closed on validation: {err}"
        );
        // FAIL-CLOSED: the executor was NEVER called — the untrusted args were not dispatched.
        assert_eq!(exec.call_count(), 0, "0 dispatch on a validation failure");
        // but the token was STILL torn down (the teardown is unconditional), and no trace was written.
        assert_eq!(tele.tokens_revoked(), 1, "torn down on the validation-abort path");
        assert_eq!(tele.traces_written(), 0);
        assert_eq!(tele.runs_completed(), 0);
    }

    /// **An executor ERROR mid-loop aborts the run + tears down (LOUD, fail-closed).** The call
    /// validates, but the [`ToolExecutor`] returns an error → the run aborts with
    /// [`SkeletonError::ToolExecFailed`], torn down, no trace.
    #[test]
    fn loop_executor_error_aborts_and_tears_down() {
        let brain = AlwaysUseTool(ToolName("read".into()));
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("read")]);
        // The executor fails on the first call.
        let exec = MockToolExecutor::with_results([Err(ToolExecError::Failed("subsystem down".into()))]);
        let mut sub = substrate(
            "Rerr", &revoker, &cat, &exec, &mut gate, &mut ledger, &outbox, 100, 10, 1000,
        );

        let err = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("an executor error aborts the run");
        assert!(
            matches!(err, SkeletonError::ToolExecFailed(_)),
            "loud executor failure: {err}"
        );
        assert_eq!(exec.call_count(), 1, "the failing call was attempted once");
        assert_eq!(tele.tokens_revoked(), 1, "torn down on the executor-error path");
        assert_eq!(tele.traces_written(), 0);
        assert_eq!(tele.runs_completed(), 0);
    }

    // ───────── raw token-usage telemetry (contract 8.3; NON-FINANCIAL — observability only) ──────

    /// **A SKELETON run's token totals are 0 / NotReported, and the ledger is UNAFFECTED.** The
    /// SKELETON has no model, so its single submit turn reports no usage: the token totals stay 0, the
    /// not-reported counter reads 1 (the one turn), and the balanced reserve/settle ledger is
    /// untouched (token usage is a NEW, separate signal from the cost ledger).
    #[test]
    fn skeleton_run_token_totals_are_zero_and_not_reported() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rtok0", &revoker, &cat, &exec, &mut gate, &mut ledger, &outbox, 100, 10, 1000,
        );

        agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the SKELETON chain completes");

        assert_eq!(tele.tokens_input(), 0, "no model → 0 input tokens");
        assert_eq!(tele.tokens_cached_input(), 0, "no model → 0 cached tokens");
        assert_eq!(tele.tokens_output(), 0, "no model → 0 output tokens");
        assert_eq!(
            tele.turns_usage_not_reported(),
            1,
            "the one submit turn reported no usage (fail-closed signal)"
        );
        // The token signal did NOT perturb the reserve/settle ledger.
        assert!(tele.ledger_balanced(), "reserved == settled is unaffected");
        assert_eq!(tele.reserved(), 10);
        assert_eq!(tele.settled(), 10);
    }

    /// **A multi-turn run accumulates the per-turn raw token counts into telemetry (observability
    /// only).** A brain with a real `step_metered` override reporting fixed counts drives search →
    /// read → submit (three metered steps); the loop saturating-sums each turn's counts into the run
    /// totals, and the reserve/settle ledger stays balanced (token totals are a separate signal — no
    /// pricing, no gating).
    #[test]
    fn run_accumulates_per_turn_token_usage_into_telemetry() {
        #[derive(Default)]
        struct MeteredBrain;
        impl AgentRuntime for MeteredBrain {
            fn step(&self, conv: &Conversation) -> StepOutcome {
                match model_turns(conv) {
                    0 => StepOutcome::UseTools(vec![tool_call("search")]),
                    1 => StepOutcome::UseTools(vec![tool_call("read")]),
                    _ => StepOutcome::Submit(Submission("done".into())),
                }
            }
        }
        // The REAL metered override: report fixed raw counts every step (the only brain with a usage
        // source in this test). NON-FINANCIAL — raw counts, no pricing.
        impl MeteredRuntime for MeteredBrain {
            fn step_metered(&self, conv: &Conversation) -> MeteredStep {
                MeteredStep {
                    outcome: self.step(conv),
                    usage: TokenUsage::Reported {
                        input: 100,
                        cached_input: 20,
                        output: 5,
                    },
                }
            }
        }
        fn model_turns(conv: &Conversation) -> usize {
            conv.turns
                .iter()
                .filter(|t| matches!(t, Turn::Model(_)))
                .count()
        }

        let brain = MeteredBrain;
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rtok", &revoker, &cat, &exec, &mut gate, &mut ledger, &outbox, 100, 10, 1000,
        );

        let out = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the metered run completes");
        assert!(out.0.contains("completed"), "the run completed: {out:?}");

        // Three metered steps (search, read, submit) × (100, 20, 5) summed into the run totals.
        assert_eq!(tele.tokens_input(), 300, "3 turns × 100 input");
        assert_eq!(tele.tokens_cached_input(), 60, "3 turns × 20 cached");
        assert_eq!(tele.tokens_output(), 15, "3 turns × 5 output");
        assert_eq!(
            tele.turns_usage_not_reported(),
            0,
            "every turn reported usage"
        );
        // The token signal is SEPARATE: the reserve/settle ledger is still balanced + unchanged.
        assert!(tele.ledger_balanced(), "reserved == settled is unaffected");
        assert_eq!(tele.reserved(), 10);
        assert_eq!(tele.settled(), 10);
    }

    /// **`record_token_usage` sums reported counts + counts NotReported turns (mutation-floor).** A
    /// mix of reported and not-reported turns lands in the right totals — a mutant that drops a field
    /// or miscounts flips an assertion.
    #[test]
    fn record_token_usage_sums_reported_and_counts_not_reported() {
        let mut t = SkeletonTelemetry::new();
        t.record_token_usage(&TokenUsage::Reported {
            input: 10,
            cached_input: 2,
            output: 3,
        });
        t.record_token_usage(&TokenUsage::NotReported);
        t.record_token_usage(&TokenUsage::Reported {
            input: 5,
            cached_input: 1,
            output: 4,
        });
        assert_eq!(t.tokens_input(), 15);
        assert_eq!(t.tokens_cached_input(), 3);
        assert_eq!(t.tokens_output(), 7);
        assert_eq!(t.turns_usage_not_reported(), 1);
    }
}
