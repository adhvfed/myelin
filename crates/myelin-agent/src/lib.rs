//! # `myelin-agent` — the agent-fabric contract surface
//!
//! Types and trait signatures only, **no engine logic**. This is the small trait set behind which
//! a `MockAgentRuntime` lives today and an `LlmAgentRuntime` lives later. The thesis: if the
//! substrate is right, an agent needs almost no special code — it is a `Principal` with
//! `kind=agent` running through the *same* identity, gateway, event log, sandbox, and cost gate as
//! everyone else.
//!
//! ## The six traits
//! - [`AgentRuntime::step`] — **THE BRAIN**: the *stateless* runtime; the platform owns the
//!   [`Conversation`] history. **Strategy-swappable.**
//! - [`ToolHands::exec`] — **THE HANDS**: sandboxed computation, no host-execution bypass (the
//!   `no-host-exec` lint enforces it). `exec` is the CI runner's `kind=agent` job on the unified
//!   sandbox. **Strategy-swappable.**
//! - [`Agent::handle`] — **THE LOOP**: the platform-owned bounded multi-turn driver.
//! - [`ToolSurface`] — the one permissioned tool catalogue (register/resolve), MCP-exposable.
//! - [`EventInbox::deliver`] — the platform delivers matched events; agents don't poll.
//! - [`EffectApi::apply`] — **PLAN-THEN-APPLY**: agents never mutate directly.
//!
//! Only `AgentRuntime` (brain) and `ToolHands` (hands) are strategy-swappable. `Agent`,
//! `ToolSurface`, `EventInbox`, `EffectApi` are platform-owned and identical for mock and real —
//! the whole point of plan-then-apply.
//!
//! ## The `no-llm-in-platform` boundary
//! NO model / SDK / prompt / model-name string appears anywhere in this crate. The only place one
//! may ever appear is the real `LlmAgentRuntime` adapter (in `myelin-agent-model`). The
//! `no-llm-in-platform` lint scans every `crates/*/src/*.rs`, making this structural.
//!
//! The trait *bodies* land in the runtimes: `MockAgentRuntime` now, the real vendor brain later.
//! The deferred floors — the real runtime, the external MCP endpoint, long-term memory, and the
//! open policy questions — are recorded in `docs/gaps/agent-fabric-floors.md`. Runtime workers hold
//! no run state: a crashed run resumes from the durable workflow + the trace.

use serde::{Deserialize, Serialize};

// ───────────────────────── §2.1 the brain — value types (the conversation) ─────────────────────

/// The agent conversation the *stateless* brain reads (architecture §2.1; contract 8.3). **The
/// platform owns history** — the runtime is stateless. Carried forward from Phase-3 §2.1:
/// `{system, turns, tools, budget}`. The concrete `SystemContext` / `ToolSchema` / `BudgetView`
/// member types land with the runtime (AG-P4/AG-P5); here they are opaque newtypes so the trait
/// surface compiles and the brain seam is the point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Conversation {
    /// Task framing, the agent's role, the labelled-as-agent notice (architecture §2.1).
    pub system: SystemContext,
    /// Prior model steps + tool results — platform-owned, the trace (architecture §2.1).
    pub turns: Vec<Turn>,
    /// The tools THIS run may call — already permission/delegation-scoped (§5.2). The N+1-free
    /// `list_objects` push-down that computes this subset is AG-P7 (→ P-219).
    pub tools: Vec<ToolSchema>,
    /// Remaining reserve, so the brain can choose to `Submit` early (architecture §2.1).
    pub budget: BudgetView,
}

/// The system context the conversation opens with (architecture §2.1). Opaque newtype until the
/// runtime lands (AG-P4/AG-P5); the field exists so [`Conversation`] is the frozen shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SystemContext(pub String);

/// A tool the run may call, as the brain sees it (architecture §2.1) — the delegation-scoped
/// tool-list projection (AG-P7 → P-219). This is the tool "as the brain sees it" so the model can
/// produce valid arguments: the `input_schema` mirrors [`ToolDef::input_schema`], the JSON-schema the
/// model's chosen [`ToolCall::arguments`] are validated against before dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolSchema {
    /// The tool name (the catalogue key half; mirrors [`ToolDef::name`]).
    pub name: ToolName,
    /// A human-readable description the brain reads to choose the tool.
    pub description: String,
    /// The JSON Schema (as a string) for the tool's input — mirrors [`ToolDef::input_schema`]; the
    /// model produces arguments to satisfy it.
    pub input_schema: String,
}

/// The remaining-reserve view the brain reads to decide whether to `Submit` early (architecture
/// §2.1). Opaque until the reserve/settle cost gate lands (AG-P14 → P-227).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BudgetView(pub u64);

/// One turn of the platform-owned conversation history (architecture §2.1; Phase-3 §2.1):
/// `Model(StepOutcome) | ToolResults(Vec<ToolOutcome>) | Approval(ApprovalNote)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Turn {
    /// A model step (the brain's decision for this turn).
    Model(StepOutcome),
    /// The results of the tool calls the brain requested, each linked back to its call by id.
    ToolResults(Vec<ToolOutcome>),
    /// An HITL approval note appended to the trace (the card text is humanised, AG-P11 → P-223).
    Approval(ApprovalNote),
}

/// An HITL approval note carried in the conversation (architecture §2.1). Opaque until the
/// withhold→surface→resume loop lands (AG-P9 → P-221).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalNote(pub String);

/// The model-chosen identity of a single tool call, minted by the brain so each later
/// [`ToolOutcome`] can be linked back to the call it answers (architecture §2.1; contract 8.3). Both
/// vendors require this linkage (OpenAI a `tool` message with a `tool_call_id`; Anthropic a
/// `tool_result` block with a `tool_use_id`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallId(pub String);

/// A proposed tool call the brain emits (architecture §2.1; contract 8.3). Carries the call `id`
/// (so its result can be linked back), the tool `name`, and the model's chosen `arguments`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The model-minted call id; the matching [`ToolOutcome::call_id`] links the result back.
    pub id: ToolCallId,
    /// Which tool to call (the catalogue key half; resolves to a [`ToolDef`]).
    pub name: ToolName,
    /// The model's chosen JSON input for the call. **UNTRUSTED model output** — before a
    /// `ToolCall` is dispatched to a tool, these arguments MUST be validated against that tool's
    /// [`ToolDef::input_schema`]; the brain only ever *proposes* (plan-then-apply survives, §2.1).
    pub arguments: serde_json::Value,
}

/// A final submission from the brain — its answer / proposed effects (architecture §2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission(pub String);

/// The brain's per-step outcome (architecture §2.1; contract 8.3): **use tools**, or **submit**.
/// The brain only ever *proposes* — it never acts (plan-then-apply survives, §2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepOutcome {
    /// "Call these tools, give me the results, step me again."
    UseTools(Vec<ToolCall>),
    /// "I'm done; here is my final answer / proposed effects."
    Submit(Submission),
}

/// **Raw provider token usage for ONE model step (contract 8.3; NON-FINANCIAL — counts only).**
///
/// This is the *platform-side* projection of the vendor's per-call token accounting. It lives HERE,
/// in the seam crate, so the driving loop (`myelin-agent-service`) can observe a run's token usage
/// WITHOUT depending on the vendor crate `myelin-agent-model` (the `no-llm-in-platform` boundary,
/// contract 1.6). The vendor→platform mapping (`myelin_agent_model::client::Usage` → this type) is
/// the sanctioned job of `myelin-agent-model`'s `MeteredRuntime` override.
///
/// **Raw counts, never fabricated.** These are the provider's own token counts. A provider that
/// omits its usage block surfaces [`TokenUsage::NotReported`] — the runtime never estimates a count.
/// [`TokenUsage::NotReported`] is what the future metering slice reads to **fail closed** (never
/// price an unmetered call). This carrier is *observability only*: it holds NO money, NO pricing, NO
/// wallet — pricing raw counts into a bill (wholesale/markup, micro-units) is a SEPARATE,
/// decision-gated follow-on slice that lives outside this crate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenUsage {
    /// The provider reported token counts for the step. `input` is the non-cached prompt tokens
    /// (standard input tier); `cached_input` is the tokens served from the prompt cache (a cheaper
    /// tier); `output` is the completion tokens. Raw counts — no pricing.
    Reported {
        /// Non-cached prompt tokens (standard input tier).
        input: u64,
        /// Cached prompt tokens (cache-hit tier).
        cached_input: u64,
        /// Completion (output) tokens.
        output: u64,
    },
    /// The provider omitted usage (or the brain has no usage source, e.g. Skeleton/Mock). The future
    /// metering slice MUST fail the run closed on this — never estimate a count.
    #[default]
    NotReported,
}

// ───────────────────────── §2.2 the hands — value types (sandboxed exec) ────────────────────────

/// A sandboxed command for the hands (architecture §2.2; contract 8.4). Carries only
/// `compute`/`external` untrusted code — mutation goes through [`EffectApi`], never here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command(pub String);

/// The result of a sandboxed [`ToolHands::exec`] (architecture §2.2; contract 8.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult(pub String);

/// A tool [`ToolResult`] linked back to the [`ToolCall`] it answers (architecture §2.1; contract
/// 8.3/8.4). The platform records these into [`Turn::ToolResults`] so the brain's next step sees
/// each result keyed to the call it requested — the linkage both vendors require (an OpenAI `tool`
/// message / an Anthropic `tool_result` block carrying the originating call id).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    /// The [`ToolCall::id`] this result answers.
    pub call_id: ToolCallId,
    /// The hands' execution result for that call.
    pub result: ToolResult,
}

// ───────────────────────── §4.2 the tool surface — ToolDef (the frozen field list) ──────────────

/// The lookup key into the one tool catalogue (architecture §4.2; contract 8.1). `ToolDef` is
/// versioned (forward-only) and keyed by `(subsystem, name, version)`; this is the `name` half.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolName(pub String);

/// How an effect routes (architecture §5.0; contract 8.1 `effect_kind`): the platform loop routes
/// `UseTools` per `effect_kind` / `side_effecting` — `read` direct, `compute`/`external` into the
/// sandbox ([`ToolHands::exec`]), `mutate` through [`EffectApi::apply`] (plan-then-apply).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectKind {
    /// A permission-filtered read — no mutation, no sandbox.
    Read,
    /// Untrusted computation (a test/build/linter/script) — into the sandbox.
    Compute,
    /// Governed mutation — through [`EffectApi::apply`] (plan-then-apply).
    Mutate,
    /// An untrusted outbound/external call — into the sandbox.
    External,
}

/// A tool definition registered into the one permissioned catalogue (architecture §4.2; contract
/// 8.1 — **the frozen field list**). Every subsystem contributes its actions; the catalogue is
/// MCP-exposable (the MCP surface is a projection of `ToolDef`).
///
/// **`requires_approval` here is the COLUMN, not a seeded value.** The per-subsystem
/// `requires_approval` defaults table (CI deploy/secret = yes; Git merge = yes, open_pr = no; …)
/// is **frozen but SEEDED in AG-P8 (→ P-220)** — this prompt ships the field, not the defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDef {
    /// The tool name (the catalogue key half; `(subsystem, name, version)`).
    pub name: ToolName,
    /// The contributing subsystem (an event-bus §6.2 token).
    pub subsystem: String,
    /// `ToolDef` is versioned (forward-only).
    pub version: u32,
    /// JSON Schema for the tool's input, validated pre-apply (opaque-string carrier at this seam).
    pub input_schema: String,
    /// The `Permission`(s) the run must hold (the Id `check`, §5.2).
    pub required_caps: Vec<String>,
    /// How the effect routes (`read | compute | mutate | external`).
    pub effect_kind: EffectKind,
    /// Whether applying the tool has a side effect.
    pub side_effecting: bool,
    /// Whether the tool is HITL-gated by default. The per-subsystem DEFAULTS are seeded in AG-P8
    /// (→ P-220); here the column exists, no value is seeded.
    pub requires_approval: bool,
    /// Whether the tool is exposed over the external MCP endpoint (the MCP surface is a projection
    /// of `ToolDef`; the external endpoint itself is a post-M5 floor).
    pub exposed_over_mcp: bool,
}

// ───────────────────────── §5.2 plan-then-apply — EffectApi value types ─────────────────────────

/// The run context an effect is applied under (architecture §5.2; contract 8.2). Carries the
/// per-run attenuated token + budget + causality (the full shape lands with the SKELETON runtime,
/// AG-P4 → P-216, and the plan-then-apply pipeline, AG-P6 → P-218); opaque here so the trait
/// signature is the frozen shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunCtx(pub String);

/// The exact per-run capability token and binding a governed external effect acts under.
///
/// This carrier is deliberately separate from [`RunCtx`]: `RunCtx` is audit/causality metadata,
/// while this value is authorization input that MUST be cryptographically verified again at the
/// final mutation boundary. Its fields are not themselves trusted merely because this struct was
/// constructed; a concrete [`EffectApi`] must verify the signed token, its live run-token record,
/// subject/scope, tool binding, and the tool's independently resolved required capabilities before
/// applying anything.
#[derive(Clone, PartialEq, Eq)]
pub struct EffectAuthority {
    /// The signed, attenuated token minted for this run.
    pub run_token: myelin_identity::RunToken,
    /// The authenticated principal the router bound the run to.
    pub principal_id: myelin_identity::PrincipalId,
    /// The exact registered tool selected by the router.
    pub tool: String,
    /// Caller-minted, retry-stable invocation identity. This is authorization-adjacent request
    /// metadata, not token material: adapters may hash it into their durable operation identity but
    /// must never persist or log it verbatim.
    pub idempotency_key: String,
}

impl core::fmt::Debug for EffectAuthority {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EffectAuthority")
            .field("jti", &"<redacted>")
            .field("principal_id", &self.principal_id)
            .field("tool", &self.tool)
            .field("idempotency_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// A proposed effect the brain wants the platform to apply (architecture §5.2; contract 8.2).
/// Agents NEVER mutate directly — a `ProposedEffect` goes through the schema → capability →
/// delegation → tenant → budget → HITL-gate → apply → meter pipeline (AG-P6 → P-218); opaque here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedEffect(pub String);

/// An opaque event id returned when an effect is applied (architecture §5.2; contract 8.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventId(pub String);

/// An opaque HITL gate id returned when an effect is withheld pending approval (architecture §5.3;
/// contract 8.2). A withheld gated tool does NOT mutate (AG-8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateId(pub String);

/// The outcome of [`EffectApi::apply`] (architecture §5.2; contract 8.2): **Applied**, **Gated**
/// (withheld for HITL — does not mutate), or **Denied** (an ordinary tool error).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectResult {
    /// The effect was applied; carries the emitted domain event id.
    Applied(EventId),
    /// The effect is withheld pending an HITL approval; carries the gate id. Does NOT mutate.
    Gated(GateId),
    /// The effect was denied (an ordinary tool error); carries the reason.
    Denied(String),
}

// ───────────────────────── §2.3 the loop / §8.6 delivery — value types ──────────────────────────

/// An event the platform delivers into the agent inbox (architecture §2.3/§3.4; contract 8.6).
/// Carries the envelope + binding + token + budget; agents don't poll. **Explicit-first dispatch**
/// (CHAT-1): a mention notifies, it does NOT auto-spawn a costed run (the dispatch tier is the
/// Bus's, §3.6); opaque here, the binding shape lands with the SKELETON runtime (AG-P4 → P-216).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxEvent(pub String);

/// The outcome of a bounded multi-turn run (architecture §2.3; contract 8.5). Opaque until the
/// SKELETON loop lands (AG-P4 → P-216).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutcome(pub String);

// ───────────────────────── the six traits (frozen signatures only) ──────────────────────────────

/// **THE BRAIN** — the *stateless* runtime (architecture §2.1; contract 8.3; AG-1). The
/// **only** strategy-swappable seam alongside [`ToolHands`]: the runtime behind which
/// `MockAgentRuntime` (`--use-mock`) lives now and `LlmAgentRuntime` lives later. The platform owns
/// the [`Conversation`] history; `step` is pure-ish (conversation in, decision out) so it is
/// trivially mockable + golden/mutation-testable.
///
/// **Floor:** the body is shipped by the runtimes — SKELETON (AG-P4 → P-216), `MockAgentRuntime`
/// (AG-P5 → P-217), `LlmAgentRuntime` (the only vendor seam, **designed-not-built, AG-P25**, the
/// only place an LLM SDK/prompt/model-name may ever appear). NO LLM SDK in platform code
/// (`no-llm-in-platform`, contract 1.6).
pub trait AgentRuntime {
    /// Take the whole conversation, return a single decision (use tools, or submit).
    fn step(&self, conv: &Conversation) -> StepOutcome;
}

/// **One brain step PLUS its raw token usage (contract 8.3; NON-FINANCIAL).** The seam decision
/// [`StepOutcome`] together with the [`TokenUsage`] the model reported for it, so the driving loop
/// can accumulate per-turn token counts into a run's telemetry. Observability only — no pricing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeteredStep {
    /// The brain's decision this step (use tools, or submit).
    pub outcome: StepOutcome,
    /// The provider's raw token counts for this step ([`TokenUsage::NotReported`] if none/unmetered).
    pub usage: TokenUsage,
}

/// **THE METERED BRAIN SEAM** — one step *observably*: the decision AND its raw token usage
/// (contract 8.3; NON-FINANCIAL — counts only, no pricing). A super-trait of [`AgentRuntime`] with a
/// DEFAULT [`step_metered`](MeteredRuntime::step_metered): a brain with no usage source (the SKELETON,
/// the Mock) reports [`TokenUsage::NotReported`] for free; only the real vendor brain
/// (`LlmAgentRuntime`) overrides it to map the provider's usage → [`TokenUsage`].
///
/// **Why explicit per-type impls, NOT a blanket `impl<T: AgentRuntime> MeteredRuntime for T`.** A
/// blanket impl would collide (trait coherence) with the vendor crate's real override
/// `impl MeteredRuntime for LlmAgentRuntime` — the compiler cannot know the two don't overlap. So
/// each runtime gets an explicit `impl MeteredRuntime for <Type> {}` (the default is inherited for
/// free by the Skeleton/Mock), and `LlmAgentRuntime` gets a real body. The default method means an
/// empty impl is genuinely empty — the seam costs a usage-less brain one line.
pub trait MeteredRuntime: AgentRuntime {
    /// One step PLUS its token usage. The default is the plain [`AgentRuntime::step`] paired with
    /// [`TokenUsage::NotReported`] (a brain with no usage source). The LLM runtime overrides this to
    /// report the provider's real counts.
    fn step_metered(&self, conv: &Conversation) -> MeteredStep {
        MeteredStep {
            outcome: self.step(conv),
            usage: TokenUsage::NotReported,
        }
    }
}

/// **THE LOOP** (architecture §2.3; contract 8.5; AG-3). A platform-owned, **bounded, driven**
/// multi-turn loop — NOT a single call, NOT the runtime's responsibility, NOT a strategy seam
/// (identical for mock and real). A run is a durable workflow; nested causality is preserved.
///
/// **Floor:** the loop body (build_conversation → reserve → repeatedly `step` → route per §5.0 →
/// settle) lands with the SKELETON runtime (AG-P4 → P-216).
pub trait Agent {
    /// Drive the bounded multi-turn loop for one delivered inbox event.
    fn handle(&self, inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome;
}

/// **THE HANDS** — sandboxed computation (architecture §2.2; contract 8.4; AG-2). The other
/// strategy-swappable seam (real `SandboxedHands` / `SimHands`). **No host-execution path bypasses
/// `exec`** (X-6; the `no-host-exec` lint, contract 1.6, enforces it). `exec` IS the CI runner's
/// `kind=agent` job on the ONE unified sandbox; it carries only `compute`/`external` untrusted code
/// — mutation goes through [`EffectApi`], never here (the routing split is the safety boundary).
///
/// **Floor:** the sandboxed body + the four uniform guarantees (cost gate, per-run-token
/// attribution, HITL withhold, isolation floor + the real-kernel escape drill) land in AG-P15
/// (→ P-226); the hard ZERO-escapes real-kernel GATE is AG-P17 (→ P-229) / CI-P5.
pub trait ToolHands {
    /// Run untrusted code in the sandbox and return its result.
    fn exec(&self, cmd: Command) -> ToolResult;
}

/// **THE TOOL REGISTRY** — one permissioned catalogue, MCP-exposable (architecture §4.2; contract
/// 8.1; ADR-08.4). Platform-owned (identical for mock and real). Every subsystem contributes its
/// [`ToolDef`]s; `resolve` looks a tool up by name.
///
/// **Floor:** the persisted catalogue + the per-subsystem `requires_approval` defaults seed land in
/// AG-P8 (→ P-220); the data-model migration is AG-P2 (→ P-131).
pub trait ToolSurface {
    /// Register a tool into the one catalogue.
    fn register_tool(&mut self, def: ToolDef);
    /// Resolve a tool by name (architecture §1.3 / Phase-3 §7.1).
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef>;
}

/// **DELIVERY** — the platform delivers matched events; agents don't poll (architecture §1.3/§3.4;
/// contract 8.6; ADR-08). Platform-owned. **Explicit-first dispatch** (CHAT-1): a mention notifies,
/// it does NOT auto-spawn a costed run (implicit auto-dispatch is L-3, counsel-gated, AG-P20).
///
/// **Floor:** the dispatch-tier wiring is the Bus's (§3.6); the agent-side consumer lands with the
/// SKELETON runtime (AG-P4 → P-216).
pub trait EventInbox {
    /// Deliver a matched event into the agent inbox (envelope + binding + token + budget).
    fn deliver(&self, ev: InboxEvent);
}

/// **PLAN-THEN-APPLY** — the platform-owned write-back path (architecture §5.2; contract 8.2;
/// ADR-08.3). Platform-owned (identical for mock and real). Agents NEVER mutate directly: every
/// `ProposedEffect` runs schema → capability → delegation → tenant → budget → HITL-gate → apply via
/// the public endpoint → meter, and returns `Applied | Gated | Denied`.
///
/// **Floor:** the eight-step pipeline body (AG-D1/D2/D3) lands in AG-P6 (→ P-218); the per-effect
/// HITL idempotency is AG-P10 (→ P-222).
pub trait EffectApi {
    /// Apply (or gate / deny) a proposed effect under the run's context.
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult;

    /// Apply an externally routed effect under the exact minted run-token authority.
    ///
    /// Mutation-capable implementations exposed to MCP or another external router MUST override
    /// this method and re-verify `authority` at their final mutation boundary. The default is a
    /// fail-closed denial so adding this entry cannot silently turn a legacy `EffectApi` into an
    /// externally callable mutation path.
    fn apply_authorized(
        &self,
        _run: &RunCtx,
        _authority: &EffectAuthority,
        _effect: ProposedEffect,
    ) -> EffectResult {
        EffectResult::Denied(
            "effect adapter does not implement signed run-token authority verification — denied"
                .into(),
        )
    }
}

/// **`run --dry-run`** — plan-then-apply testability (architecture §7.1; contract 8.7). Returns the
/// proposed effects WITHOUT applying any (the `run --dry-run` plan lever, AG-P8 → P-220). This is
/// the signature seam; the CLI body lands in AG-P8.
///
/// **Floor:** the dry-run lever body lands in AG-P8 (→ P-220).
pub trait DryRun {
    /// Plan the run for a delivered event without applying any effect.
    fn dry_run(&self, inbox: InboxEvent) -> Vec<ProposedEffect>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_authority_debug_redacts_every_credential_and_replay_handle() {
        let authority = EffectAuthority {
            run_token: myelin_identity::RunToken {
                token: "secret-bearer".into(),
                jti: "secret-jti".into(),
            },
            principal_id: myelin_identity::PrincipalId("principal".into()),
            tool: "issue.close".into(),
            idempotency_key: "secret-idempotency-key".into(),
        };
        let rendered = format!("{authority:?}");
        for secret in ["secret-bearer", "secret-jti", "secret-idempotency-key"] {
            assert!(!rendered.contains(secret));
        }
    }

    /// A deterministic mock impl of every swappable + platform-owned trait — proves all six
    /// (+`DryRun`, 8.7) signatures compile against a real impl on the SAME code path users hit
    /// (`--use-mock`, 8.3). No LLM SDK appears (the `no-llm-in-platform` ratchet, 1.6).
    struct Mock {
        catalogue: Vec<ToolDef>,
    }

    impl AgentRuntime for Mock {
        fn step(&self, _conv: &Conversation) -> StepOutcome {
            // A mock runtime is a real flag (`--use-mock`, 8.3); a deterministic submit is a valid
            // skeleton decision. The body proper lands with the runtimes (AG-P4/AG-P5).
            StepOutcome::Submit(Submission("ok".into()))
        }
    }

    // The explicit (empty) MeteredRuntime impl — a usage-less brain inherits the default
    // `step_metered` (NotReported). Explicit, NOT blanket, so the vendor `LlmAgentRuntime` override
    // does not collide under coherence.
    impl MeteredRuntime for Mock {}

    impl Agent for Mock {
        fn handle(&self, _inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome {
            // The bounded loop body lands in AG-P4 (→ P-216). Here it drives one `step` so the
            // `&dyn AgentRuntime` seam is exercised (the brain is dynamically dispatched).
            let _ = runtime.step(&Conversation::default());
            RunOutcome("done".into())
        }
    }

    impl ToolHands for Mock {
        fn exec(&self, _cmd: Command) -> ToolResult {
            // SimHands marker — proves it went through the trait, not a host shell (no-host-exec).
            // The sandboxed body + four uniform guarantees land in AG-P15 (→ P-226).
            ToolResult("sim:executed".into())
        }
    }

    impl ToolSurface for Mock {
        fn register_tool(&mut self, def: ToolDef) {
            self.catalogue.push(def);
        }
        fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
            self.catalogue.iter().find(|d| &d.name == name)
        }
    }

    impl EventInbox for Mock {
        fn deliver(&self, _ev: InboxEvent) {
            // Explicit-first dispatch: deliver notifies; it does not auto-spawn a costed run.
        }
    }

    impl EffectApi for Mock {
        fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
            // Plan-then-apply body (8-step pipeline) lands in AG-P6 (→ P-218); a deterministic
            // Applied is a valid skeleton outcome that exercises the EffectResult value type.
            EffectResult::Applied(EventId("evt-1".into()))
        }
    }

    impl DryRun for Mock {
        fn dry_run(&self, _inbox: InboxEvent) -> Vec<ProposedEffect> {
            vec![ProposedEffect("planned".into())]
        }
    }

    fn def(name: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "issues".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec!["issue.write".into()],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            // The COLUMN exists; the per-subsystem DEFAULT is seeded in AG-P8 (→ P-220). A test
            // value here is not the frozen default — it proves the field round-trips.
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }

    /// 8.3 — `AgentRuntime::step(&Conversation) -> StepOutcome` compiles against a mock and returns
    /// a frozen-shape decision.
    #[test]
    fn agent_runtime_step_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        assert!(matches!(
            m.step(&Conversation::default()),
            StepOutcome::Submit(_)
        ));
    }

    /// 8.3 — the default [`MeteredRuntime::step_metered`] pairs the plain `step` decision with
    /// [`TokenUsage::NotReported`] (a usage-less brain reports no counts, never fabricates one).
    #[test]
    fn metered_runtime_default_step_is_not_reported() {
        let m = Mock { catalogue: vec![] };
        let metered = m.step_metered(&Conversation::default());
        assert_eq!(metered.outcome, StepOutcome::Submit(Submission("ok".into())));
        assert_eq!(metered.usage, TokenUsage::NotReported);
        // The default projects the SAME outcome the plain seam returns.
        assert_eq!(metered.outcome, m.step(&Conversation::default()));
    }

    /// The [`TokenUsage`] carrier round-trips its raw counts (Reported ≠ NotReported; Default is
    /// NotReported — the fail-closed default the metering slice reads).
    #[test]
    fn token_usage_carries_raw_counts_and_defaults_not_reported() {
        assert_eq!(TokenUsage::default(), TokenUsage::NotReported);
        let reported = TokenUsage::Reported {
            input: 50,
            cached_input: 8,
            output: 12,
        };
        assert_ne!(reported, TokenUsage::NotReported);
        assert!(matches!(
            reported,
            TokenUsage::Reported {
                input: 50,
                cached_input: 8,
                output: 12
            }
        ));
        // The wire shape is frozen (a renamed/dropped count field fails the round-trip).
        let json = serde_json::to_string(&reported).unwrap();
        assert_eq!(reported, serde_json::from_str(&json).unwrap());
    }

    /// 8.5 — `Agent::handle(InboxEvent, &dyn AgentRuntime) -> RunOutcome` compiles and the brain is
    /// dynamically dispatched through the `&dyn AgentRuntime` seam (the strategy boundary).
    #[test]
    fn agent_handle_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        let out = m.handle(InboxEvent("mention".into()), &m);
        assert_eq!(out, RunOutcome("done".into()));
    }

    /// 8.4 — `ToolHands::exec(Command) -> ToolResult` compiles; the SimHands marker proves it went
    /// through the trait, not a host shell (no-host-exec, X-6/AG-2).
    #[test]
    fn tool_hands_exec_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        assert_eq!(
            m.exec(Command("cargo test".into())),
            ToolResult("sim:executed".into())
        );
    }

    /// 8.1 — `ToolSurface::{register_tool, resolve}` compiles; register-then-resolve round-trips
    /// the full `ToolDef` field list, and an unknown name resolves to `None`.
    #[test]
    fn tool_surface_register_resolve_signatures_are_frozen() {
        let mut m = Mock { catalogue: vec![] };
        m.register_tool(def("issue.transition"));
        let resolved = m.resolve(&ToolName("issue.transition".into()));
        assert!(resolved.is_some());
        let d = resolved.unwrap();
        assert_eq!(d.subsystem, "issues");
        assert_eq!(d.effect_kind, EffectKind::Mutate);
        assert!(d.side_effecting);
        assert!(m.resolve(&ToolName("nope".into())).is_none());
    }

    /// 8.6 — `EventInbox::deliver(InboxEvent)` compiles (explicit-first: deliver notifies, no
    /// auto-spawn).
    #[test]
    fn event_inbox_deliver_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        m.deliver(InboxEvent("issue.created".into()));
    }

    /// 8.2 — `EffectApi::apply(&RunCtx, ProposedEffect) -> EffectResult` compiles and returns a
    /// frozen-shape outcome (Applied carries an EventId).
    #[test]
    fn effect_api_apply_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        match m.apply(&RunCtx::default(), ProposedEffect("close-issue".into())) {
            EffectResult::Applied(EventId(id)) => assert_eq!(id, "evt-1"),
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    /// 8.7 — `run --dry-run(InboxEvent) -> Vec<ProposedEffect>` compiles and plans without applying.
    #[test]
    fn dry_run_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        let plan = m.dry_run(InboxEvent("mention".into()));
        assert_eq!(plan, vec![ProposedEffect("planned".into())]);
    }

    // ───────── value-type mutation-score floor (TESTS field: ToolDef/EffectResult/StepOutcome) ──
    //
    // `myelin-agent` is a mandatory-core glue crate. The value-type module (`ToolDef`,
    // `EffectResult`, `StepOutcome`, `EffectKind`) is PURE and must be mutation-covered: the floor
    // is **every mutation of a variant/field/tag of these value types is killed by a test**. A full
    // `cargo-mutants` run is the substrate mutation-harness's job (P-S22 / the thresholds file); the
    // tests below discharge the per-line obligation those structural assertions already cover. The
    // measured `cargo-mutants` run over `crates/myelin-agent` at AG-P1 is recorded in the commit
    // body (0 missed mutants over the value-type module).

    /// Kills any mutation that swaps / drops a `StepOutcome` variant: both variants are
    /// distinguishable and carry their payload.
    #[test]
    fn step_outcome_variants_are_distinct() {
        let use_tools = StepOutcome::UseTools(vec![ToolCall {
            id: ToolCallId("call-1".into()),
            name: ToolName("t".into()),
            arguments: serde_json::Value::Null,
        }]);
        let submit = StepOutcome::Submit(Submission("a".into()));
        assert_ne!(use_tools, submit);
        assert!(matches!(use_tools, StepOutcome::UseTools(ref v) if v.len() == 1));
        assert!(matches!(submit, StepOutcome::Submit(ref s) if s.0 == "a"));
    }

    /// Kills any mutation that swaps / drops an `EffectResult` variant: all three are
    /// distinguishable and carry their payload (Applied=EventId, Gated=GateId, Denied=reason).
    #[test]
    fn effect_result_variants_are_distinct() {
        let applied = EffectResult::Applied(EventId("e".into()));
        let gated = EffectResult::Gated(GateId("g".into()));
        let denied = EffectResult::Denied("nope".into());
        assert_ne!(applied, gated);
        assert_ne!(gated, denied);
        assert_ne!(applied, denied);
        assert!(matches!(applied, EffectResult::Applied(EventId(ref id)) if id == "e"));
        assert!(matches!(gated, EffectResult::Gated(GateId(ref id)) if id == "g"));
        assert!(matches!(denied, EffectResult::Denied(ref r) if r == "nope"));
    }

    /// Kills any mutation that swaps / drops an `EffectKind` variant or its routing identity: the
    /// four kinds are pairwise distinct (read | compute | mutate | external).
    #[test]
    fn effect_kind_variants_are_distinct() {
        let all = [
            EffectKind::Read,
            EffectKind::Compute,
            EffectKind::Mutate,
            EffectKind::External,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert_eq!(i == j, a == b, "{a:?} vs {b:?}");
            }
        }
    }

    /// Kills any mutation of a `ToolDef` field: every field round-trips its set value (the frozen
    /// 8.1 field list — name, subsystem, version, input_schema, required_caps, effect_kind,
    /// side_effecting, requires_approval, exposed_over_mcp).
    #[test]
    fn tool_def_field_list_is_frozen() {
        let d = ToolDef {
            name: ToolName("ci.deploy".into()),
            subsystem: "ci".into(),
            version: 7,
            input_schema: "{\"x\":1}".into(),
            required_caps: vec!["ci.deploy".into(), "secret.read".into()],
            effect_kind: EffectKind::External,
            side_effecting: true,
            requires_approval: true,
            exposed_over_mcp: true,
        };
        assert_eq!(d.name, ToolName("ci.deploy".into()));
        assert_eq!(d.subsystem, "ci");
        assert_eq!(d.version, 7);
        assert_eq!(d.input_schema, "{\"x\":1}");
        assert_eq!(d.required_caps.len(), 2);
        assert_eq!(d.effect_kind, EffectKind::External);
        assert!(d.side_effecting);
        assert!(d.requires_approval);
        assert!(d.exposed_over_mcp);
    }

    /// The value types serde round-trip (the wire shape is frozen — a renamed/dropped field fails).
    #[test]
    fn value_types_serde_round_trip() {
        let d = def("issue.close");
        let json = serde_json::to_string(&d).unwrap();
        let back: ToolDef = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);

        let r = EffectResult::Gated(GateId("card:1:0".into()));
        let rj = serde_json::to_string(&r).unwrap();
        let rb: EffectResult = serde_json::from_str(&rj).unwrap();
        assert_eq!(r, rb);
    }
}
