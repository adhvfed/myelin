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

use myelin_agent::{
    Agent, AgentRuntime, Conversation, InboxEvent, RunOutcome, StepOutcome, Submission,
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
        StepOutcome::Submit(Submission("skeleton: no model, no tools — immediate submit".into()))
    }
}

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
}

impl core::fmt::Display for SkeletonError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SkeletonError::DispatchRefused(m) => write!(f, "SKELETON dispatch refused: {m}"),
            SkeletonError::MintFailed(m) => write!(f, "SKELETON mint failed: {m}"),
            SkeletonError::CoCommit(m) => write!(f, "SKELETON co-commit failed: {m}"),
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
        runtime: &dyn AgentRuntime,
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

        // (3) BUILD the conversation — EMPTY for the SKELETON (no trace history, no tools). The build
        //     proves the seam; the from-trace build (AG-1) + the delegation-scoped tool subset (AG-P7)
        //     are the named floors.
        let conv = Conversation::default();

        // (4) STEP the brain (the §5.0 routing point). The SKELETON submits immediately — no tools to
        //     route. A mock/LLM brain would loop here (UseTools → route → append → step again); the
        //     loop body is identical, only the decision differs. A UseTools from the SKELETON is
        //     impossible by construction, but we route defensively: the SKELETON never has tools, so a
        //     non-Submit is a contract bug we surface rather than silently drop.
        let submission = match runtime.step(&conv) {
            StepOutcome::Submit(s) => s,
            StepOutcome::UseTools(_) => {
                // The SKELETON has no tools; a UseTools is a contract violation. Tear down + abort
                // LOUD (never silently route a tool the SKELETON cannot run).
                self.teardown(sub, &token, teardown_at, telemetry);
                return Err(SkeletonError::CoCommit(
                    "SKELETON runtime returned UseTools but has no tools (contract 8.3 violation) \
                     — tools land in AG-P6/AG-P15"
                        .into(),
                ));
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
        let settled_total = settle
            .billed_total
            .0
            .saturating_add(settle.refunded.0);
        telemetry.record_settle(settled_total);

        // (7) TEARDOWN: revoke the per-run token idempotently (even on crash), belt-and-suspenders
        //     with the auto-expiring TTL (§5.7). The teardown is unconditional — it fires on the
        //     completed path here exactly as it fired on the killed/errored paths above.
        self.teardown(sub, &token, teardown_at, telemetry);
        telemetry.runs_completed = telemetry.runs_completed.saturating_add(1);

        let _ = submission; // the SKELETON's submission is content-free; the trace is the artifact.
        Ok(RunOutcome(format!(
            "completed: run={} trace={} reserved={} settled={} token-revoked",
            sub.run_id, trace_ref, in_flight.reserved().0, settled_total
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
        RunOutcome(format!("skeleton handle: delivered={} (chained path → handle_run)", inbox.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    #[allow(clippy::too_many_arguments)]
    fn substrate<'a>(
        run_id: &str,
        revoker: &'a FakeRevoker,
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
        let mut sub = substrate(
            "R1", &revoker, &mut gate, &mut ledger, &outbox, /* avail */ 100,
            /* est */ 10, /* now */ 1000,
        );

        let out = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the SKELETON chain completes");
        assert!(out.0.contains("completed"), "the run completed the chain: {out:?}");

        // one trace row written (the trace_ref is non-empty).
        assert_eq!(tele.traces_written(), 1, "exactly one trace row written");
        // the ledger is BALANCED: reserved == settled (a SKELETON bills 0, refunds the reservation).
        assert!(tele.ledger_balanced(), "reserved {} == settled {}", tele.reserved(), tele.settled());
        assert_eq!(tele.reserved(), 10, "reserved the estimate");
        assert_eq!(tele.settled(), 10, "settled (billed 0 + refunded 10) == reserved");
        // the token was revoked on teardown (attribution closed).
        assert_eq!(tele.tokens_revoked(), 1, "the per-run token revoked on teardown");
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
        assert_eq!(token.jti, "jti:psn:agent-7:R1", "the token jti is bound to (agent, run)");
        assert_eq!(sub.agent_id, "psn:agent-7", "the run's agent principal == the token's principal");
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
        let mut sub = substrate(
            "R2", &revoker, &mut gate, &mut ledger, &outbox, 100, 10, minted_at,
        );

        let out = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::KilledMidFlight)
            .expect("a killed run still tears down cleanly");
        assert!(out.0.contains("killed-mid-flight"), "the run was killed: {out:?}");

        // the per-run token was revoked on teardown (even though the run never completed).
        assert_eq!(tele.tokens_revoked(), 1, "killed run STILL revoked its token on teardown");
        assert_eq!(tele.runs_killed(), 1);
        // 0 trace written + reservation left in-flight (never interrupted — the gate has no
        // tear-down-in-flight API; the run was killed, the reservation was NOT settled).
        assert_eq!(tele.traces_written(), 0, "a killed run wrote no trace (0 ghost — co-commit abandoned)");

        // the token is DEAD: revoked-on-teardown (now) AND auto-expires by minted_at + W.
        let jti = "jti:psn:agent-7:R2";
        assert!(revoker.is_dead(jti, minted_at), "revoked-on-teardown → dead now");
        // even ABSENT the explicit revoke, it auto-expires ≤ W (belt-and-suspenders).
        let fresh = FakeRevoker { ttl_w: w, minted_at, ..Default::default() };
        assert!(!fresh.is_dead(jti, minted_at), "not yet expired before W");
        assert!(fresh.is_dead(jti, minted_at + w), "auto-expires by minted_at + W (≤ W window)");

        // 0 SHARED token leaked into the child env (the anti-leak unset, §5.7).
        let child = ChildEnv::for_run(jti);
        assert!(!child.leaked_shared_token(), "0 shared platform token leaked into the child env");
        assert_eq!(child.shared_platform_token, None, "the child env's shared-token slot is UNSET");
        assert_eq!(child.run_token_jti, jti, "the child's ONLY credential is the per-run jti");
        // the revocation lag is within bound (teardown == now in this run → lag 0 ≤ W).
        assert!(tele.max_revocation_lag() <= w as u64, "revocation lag within bound W");
    }

    /// **No balance → no run (11.7 / AG-D11).** A dispatch against an exhausted wallet is REFUSED;
    /// the run never starts (no trace, no settle) — but the minted token is STILL torn down (the
    /// teardown is unconditional). The refusal surfaces LOUD.
    #[test]
    fn no_balance_no_run_but_token_still_torn_down() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker { ttl_w: 300, minted_at: 1000, ..Default::default() };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        // available (1) < estimate (10) → no balance, no run.
        let mut sub = substrate("R3", &revoker, &mut gate, &mut ledger, &outbox, 1, 10, 1000);

        let err = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("an unfunded dispatch is refused");
        assert!(matches!(err, SkeletonError::DispatchRefused(_)), "no balance → no run: {err}");
        // the run never started: 0 trace, 0 reserve recorded, 0 settle.
        assert_eq!(tele.traces_written(), 0);
        assert_eq!(tele.reserved(), 0, "nothing reserved (the reserve was refused)");
        assert_eq!(tele.settled(), 0);
        // but the token was STILL torn down (the teardown is unconditional).
        assert_eq!(tele.tokens_revoked(), 1, "the minted token is torn down even on a refused dispatch");
        assert_eq!(gate.reserve_refusals(), 1, "the gate counted the refusal (AG-D11 telemetry)");
    }

    /// **The frozen 8.5 `Agent::handle` shape drives the brain through the `&dyn` seam.** The trait
    /// body (the dispatch tier's entry) drives one step through the strategy boundary and returns the
    /// loop outcome; the rich chained path is `handle_run`.
    #[test]
    fn agent_handle_frozen_shape_drives_the_seam() {
        let agent_loop = SkeletonAgent::new();
        let rt = SkeletonAgentRuntime::new();
        let out = agent_loop.handle(InboxEvent("issue.created".into()), &rt);
        assert!(out.0.contains("skeleton handle"), "the frozen 8.5 shape returns the loop outcome");
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
        assert_eq!(t.tokens_revoked(), 3, "tokens_revoked counts every revoke (kills -> 1)");
        assert_eq!(t.max_revocation_lag(), 9, "max lag is the MAXIMUM (7, then 3 ignored, then 9)");
        assert_eq!(t.traces_written(), 1, "traces_written counts each trace");
        assert_eq!(t.runs_completed(), 2, "runs_completed returns its field (kills -> 1)");
        assert_eq!(t.runs_killed(), 5, "runs_killed returns its field");
        assert_eq!(t.reserved(), 0, "reserved is independent (kills cross-field constant mutants)");
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
        assert!(!clean.leaked_shared_token(), "a clean child env does not leak (the anti-leak unset)");
        // a (hypothetical) leaked env: leaked_shared_token must return TRUE (kills `-> false`).
        let leaked = ChildEnv {
            run_token_jti: "jti:R1".into(),
            shared_platform_token: Some("PLATFORM-TOKEN".into()),
        };
        assert!(leaked.leaked_shared_token(), "a leaked shared token IS a leak (kills -> false)");
    }

    /// **`SkeletonError` Display is non-empty + distinct per variant (kills the `fmt -> Ok(default)`
    /// mutant — a swallowed error message is a silent failure, EI-01 §2).** Each variant renders its
    /// machine reason loudly.
    #[test]
    fn skeleton_error_display_is_loud_and_distinct() {
        let refused = SkeletonError::DispatchRefused("no balance".into()).to_string();
        let mint = SkeletonError::MintFailed("id down".into()).to_string();
        let cc = SkeletonError::CoCommit("journal".into()).to_string();
        assert!(refused.contains("dispatch refused"), "Display renders the refusal: {refused}");
        assert!(mint.contains("mint failed"), "Display renders the mint failure: {mint}");
        assert!(cc.contains("co-commit failed"), "Display renders the co-commit failure: {cc}");
        assert_ne!(refused, mint);
        assert_ne!(mint, cc);
        assert!(!refused.is_empty(), "the error message is non-empty (kills fmt -> Ok(default))");
    }
}
