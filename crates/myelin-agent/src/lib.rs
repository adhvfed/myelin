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

// ───────────────────────── the brain — value types (the conversation) ───────────────────────────

/// The agent conversation the *stateless* brain reads: `{system, turns, tools, budget}`. **The
/// platform owns history** — the runtime is stateless. The concrete member types are opaque
/// newtypes here so the trait surface compiles and the brain seam is the point.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Conversation {
    /// Task framing, the agent's role, the labelled-as-agent notice.
    pub system: SystemContext,
    /// Prior model steps + tool results — platform-owned, the trace.
    pub turns: Vec<Turn>,
    /// The tools THIS run may call — already permission/delegation-scoped.
    pub tools: Vec<ToolSchema>,
    /// Remaining reserve, so the brain can choose to `Submit` early.
    pub budget: BudgetView,
}

/// The system context the conversation opens with. Opaque newtype; the field exists so
/// [`Conversation`] is the frozen shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SystemContext(pub String);

/// A tool the run may call, as the brain sees it — the delegation-scoped tool-list projection, so
/// the model can produce valid arguments: the `input_schema` mirrors [`ToolDef::input_schema`], the
/// JSON-schema the model's chosen [`ToolCall::arguments`] are validated against before dispatch.
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

/// The remaining-reserve view the brain reads to decide whether to `Submit` early. Opaque until the
/// reserve/settle cost gate reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BudgetView(pub u64);

/// One turn of the platform-owned conversation history:
/// `Model(StepOutcome) | ToolResults(Vec<ToolOutcome>) | Approval(ApprovalNote)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Turn {
    /// A model step (the brain's decision for this turn).
    Model(StepOutcome),
    /// The results of the tool calls the brain requested, each linked back to its call by id.
    ToolResults(Vec<ToolOutcome>),
    /// An HITL approval note appended to the trace (the card text is humanised for the reviewer).
    Approval(ApprovalNote),
}

/// An HITL approval note carried in the conversation. Opaque until the withhold→surface→resume loop
/// fills it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalNote(pub String);

/// The model-chosen identity of a single tool call, minted by the brain so each later
/// [`ToolOutcome`] can be linked back to the call it answers. Both vendors require this linkage
/// (OpenAI a `tool` message with a `tool_call_id`; Anthropic a `tool_result` block with a
/// `tool_use_id`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallId(pub String);

/// A proposed tool call the brain emits. Carries the call `id` (so its result can be linked back),
/// the tool `name`, and the model's chosen `arguments`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The model-minted call id; the matching [`ToolOutcome::call_id`] links the result back.
    pub id: ToolCallId,
    /// Which tool to call (the catalogue key half; resolves to a [`ToolDef`]).
    pub name: ToolName,
    /// The model's chosen JSON input for the call. **UNTRUSTED model output** — before a
    /// `ToolCall` is dispatched to a tool, these arguments MUST be validated against that tool's
    /// [`ToolDef::input_schema`]; the brain only ever *proposes* (plan-then-apply survives).
    pub arguments: serde_json::Value,
}

/// A final submission from the brain — its answer / proposed effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission(pub String);

/// The brain's per-step outcome: **use tools**, or **submit**. The brain only ever *proposes* — it
/// never acts (plan-then-apply survives).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepOutcome {
    /// "Call these tools, give me the results, step me again."
    UseTools(Vec<ToolCall>),
    /// "I'm done; here is my final answer / proposed effects."
    Submit(Submission),
}

/// **Raw provider token usage for ONE model step (NON-FINANCIAL — counts only).**
///
/// The *platform-side* projection of the vendor's per-call token accounting. It lives HERE, in the
/// seam crate, so the driving loop (`myelin-agent-service`) can observe a run's token usage WITHOUT
/// depending on the vendor crate `myelin-agent-model` (the `no-llm-in-platform` boundary). The
/// vendor→platform mapping (`myelin_agent_model::client::Usage` → this type) is the job of
/// `myelin-agent-model`'s `MeteredRuntime` override.
///
/// **Raw counts, never fabricated.** These are the provider's own token counts. A provider that
/// omits its usage block surfaces [`TokenUsage::NotReported`] — the runtime never estimates a count.
/// [`TokenUsage::NotReported`] is what the metering slice reads to **fail closed** (never price an
/// unmetered call). This carrier is *observability only*: it holds NO money, NO pricing, NO wallet —
/// pricing raw counts into a bill (wholesale/markup, micro-units) lives outside this crate.
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
    /// The provider omitted usage (or the brain has no usage source, e.g. the Mock). The metering
    /// slice MUST fail the run closed on this — never estimate a count.
    #[default]
    NotReported,
}

// ───────────────────────── the hands — value types (sandboxed exec) ──────────────────────────────

/// A sandboxed command for the hands. Carries only `compute`/`external` untrusted code — mutation
/// goes through [`EffectApi`], never here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command(pub String);

/// The result of a sandboxed [`ToolHands::exec`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult(pub String);

/// A tool [`ToolResult`] linked back to the [`ToolCall`] it answers. The platform records these into
/// [`Turn::ToolResults`] so the brain's next step sees each result keyed to the call it requested —
/// the linkage both vendors require (an OpenAI `tool` message / an Anthropic `tool_result` block
/// carrying the originating call id).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    /// The [`ToolCall::id`] this result answers.
    pub call_id: ToolCallId,
    /// The hands' execution result for that call.
    pub result: ToolResult,
}

// ───────────────────────── the tool surface — ToolDef (the frozen field list) ────────────────────

/// The lookup key into the one tool catalogue. `ToolDef` is versioned (forward-only) and keyed by
/// `(subsystem, name, version)`; this is the `name` half.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolName(pub String);

/// How an effect routes: the platform loop routes `UseTools` per `effect_kind` / `side_effecting` —
/// `read` direct, `compute`/`external` into the sandbox ([`ToolHands::exec`]), `mutate` through
/// [`EffectApi::apply`] (plan-then-apply).
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

/// A tool definition registered into the one permissioned catalogue — **the frozen field list**.
/// Every subsystem contributes its actions; the catalogue is MCP-exposable (the MCP surface is a
/// projection of `ToolDef`).
///
/// `requires_approval` is the COLUMN, not a seeded value: the per-subsystem defaults table (CI
/// deploy/secret = yes; Git merge = yes, open_pr = no; …) is seeded with the catalogue, not here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDef {
    /// The tool name (the catalogue key half; `(subsystem, name, version)`).
    pub name: ToolName,
    /// The contributing subsystem (an event-bus token).
    pub subsystem: String,
    /// `ToolDef` is versioned (forward-only).
    pub version: u32,
    /// JSON Schema for the tool's input, validated pre-apply (opaque-string carrier at this seam).
    pub input_schema: String,
    /// The `Permission`(s) the run must hold (the identity `check`).
    pub required_caps: Vec<String>,
    /// How the effect routes (`read | compute | mutate | external`).
    pub effect_kind: EffectKind,
    /// Whether applying the tool has a side effect.
    pub side_effecting: bool,
    /// Whether the tool is HITL-gated by default. The per-subsystem defaults are seeded with the
    /// catalogue; here the column exists, no value is seeded.
    pub requires_approval: bool,
    /// Whether the tool is exposed over the external MCP endpoint (the MCP surface is a projection
    /// of `ToolDef`; the external endpoint itself is a deferred floor).
    pub exposed_over_mcp: bool,
}

// ───────────────────────── plan-then-apply — EffectApi value types ───────────────────────────────

/// The run context an effect is applied under. Carries the per-run attenuated token + budget +
/// causality; opaque here (the full shape lands with the runtime) so the trait signature is the
/// frozen shape.
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

/// A proposed effect the brain wants the platform to apply. Agents NEVER mutate directly — a
/// `ProposedEffect` goes through the schema → capability → delegation → tenant → budget → HITL-gate
/// → apply → meter pipeline; opaque here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedEffect(pub String);

/// An opaque event id returned when an effect is applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventId(pub String);

/// An opaque HITL gate id returned when an effect is withheld pending approval. A withheld gated
/// tool does NOT mutate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateId(pub String);

/// The outcome of [`EffectApi::apply`]: **Applied**, **Gated** (withheld for HITL — does not
/// mutate), or **Denied** (an ordinary tool error).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectResult {
    /// The effect was applied; carries the emitted domain event id.
    Applied(EventId),
    /// The effect is withheld pending an HITL approval; carries the gate id. Does NOT mutate.
    Gated(GateId),
    /// The effect was denied (an ordinary tool error); carries the reason.
    Denied(String),
}

// ───────────────────────── the loop / delivery — value types ─────────────────────────────────────

/// An event the platform delivers into the agent inbox (the envelope + binding + token + budget);
/// agents don't poll. **Explicit-first dispatch:** a mention notifies, it does NOT auto-spawn a
/// costed run. Opaque here — the binding shape lands with the runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxEvent(pub String);

/// The outcome of a bounded multi-turn run. Opaque here — the shape lands with the runtime loop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutcome(pub String);

// ───────────────────────── the six traits (frozen signatures only) ──────────────────────────────

/// **THE BRAIN** — the *stateless* runtime. The **only** strategy-swappable seam alongside
/// [`ToolHands`]: the runtime behind which `MockAgentRuntime` lives now and `LlmAgentRuntime` lives
/// later. The platform owns the [`Conversation`] history; `step` is pure-ish (conversation in,
/// decision out) so it is trivially mockable + golden/mutation-testable. The real vendor brain is
/// the only place an LLM SDK / prompt / model-name may appear (`no-llm-in-platform`).
pub trait AgentRuntime {
    /// Take the whole conversation, return a single decision (use tools, or submit).
    fn step(&self, conv: &Conversation) -> StepOutcome;
}

/// **One brain step PLUS its raw token usage (NON-FINANCIAL).** The [`StepOutcome`] together with
/// the [`TokenUsage`] the model reported for it, so the driving loop can accumulate per-turn token
/// counts into a run's telemetry. Observability only — no pricing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeteredStep {
    /// The brain's decision this step (use tools, or submit).
    pub outcome: StepOutcome,
    /// The provider's raw token counts for this step ([`TokenUsage::NotReported`] if none/unmetered).
    pub usage: TokenUsage,
}

/// **THE METERED BRAIN SEAM** — one step *observably*: the decision AND its raw token usage
/// (NON-FINANCIAL — counts only, no pricing). A super-trait of [`AgentRuntime`] with a DEFAULT
/// [`step_metered`](MeteredRuntime::step_metered): a brain with no usage source (the Mock) reports
/// [`TokenUsage::NotReported`] for free; only the real vendor brain (`LlmAgentRuntime`) overrides it
/// to map the provider's usage → [`TokenUsage`].
///
/// **Why explicit per-type impls, NOT a blanket `impl<T: AgentRuntime> MeteredRuntime for T`.** A
/// blanket impl would collide (trait coherence) with the vendor crate's real override
/// `impl MeteredRuntime for LlmAgentRuntime` — the compiler cannot know the two don't overlap. So
/// each runtime gets an explicit `impl MeteredRuntime for <Type> {}` (the Mock inherits the default
/// for free), and `LlmAgentRuntime` gets a real body. The default method means an empty impl is
/// genuinely empty — the seam costs a usage-less brain one line.
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

/// **THE LOOP** — a platform-owned, **bounded, driven** multi-turn loop. NOT a single call, NOT the
/// runtime's responsibility, NOT a strategy seam (identical for mock and real). A run is a durable
/// workflow; nested causality is preserved. The body (build_conversation → reserve → repeatedly
/// `step` → route → settle) lands with the runtime.
pub trait Agent {
    /// Drive the bounded multi-turn loop for one delivered inbox event.
    fn handle(&self, inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome;
}

/// **THE HANDS** — sandboxed computation; the other strategy-swappable seam. **No host-execution
/// path bypasses `exec`** (the `no-host-exec` lint enforces it). `exec` is the CI runner's
/// `kind=agent` job on the one unified sandbox; it carries only `compute`/`external` untrusted code
/// — mutation goes through [`EffectApi`], never here (the routing split is the safety boundary).
pub trait ToolHands {
    /// Run untrusted code in the sandbox and return its result.
    fn exec(&self, cmd: Command) -> ToolResult;
}

/// **THE TOOL REGISTRY** — one permissioned catalogue, MCP-exposable. Platform-owned (identical for
/// mock and real). Every subsystem contributes its [`ToolDef`]s; `resolve` looks a tool up by name.
pub trait ToolSurface {
    /// Register a tool into the one catalogue.
    fn register_tool(&mut self, def: ToolDef);
    /// Resolve a tool by name.
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef>;
}

/// **DELIVERY** — the platform delivers matched events; agents don't poll. Platform-owned.
/// **Explicit-first dispatch:** a mention notifies, it does NOT auto-spawn a costed run (implicit
/// auto-dispatch is a counsel-gated policy question, see `docs/gaps/agent-fabric-floors.md`).
pub trait EventInbox {
    /// Deliver a matched event into the agent inbox (envelope + binding + token + budget).
    fn deliver(&self, ev: InboxEvent);
}

/// **PLAN-THEN-APPLY** — the platform-owned write-back path. Platform-owned (identical for mock and
/// real). Agents NEVER mutate directly: every `ProposedEffect` runs schema → capability →
/// delegation → tenant → budget → HITL-gate → apply via the public endpoint → meter, and returns
/// `Applied | Gated | Denied`.
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

/// **`run --dry-run`** — plan-then-apply testability. Returns the proposed effects WITHOUT applying
/// any. This is the signature seam; the CLI body lands with the runtime.
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

    /// A deterministic mock impl of every swappable + platform-owned trait — proves all six traits
    /// (+`DryRun`) compile against a real impl on the SAME code path users hit. No LLM SDK appears
    /// (the `no-llm-in-platform` ratchet).
    struct Mock {
        catalogue: Vec<ToolDef>,
    }

    impl AgentRuntime for Mock {
        fn step(&self, _conv: &Conversation) -> StepOutcome {
            // A deterministic submit is a valid decision; the body proper lands with the runtimes.
            StepOutcome::Submit(Submission("ok".into()))
        }
    }

    // The explicit (empty) MeteredRuntime impl — a usage-less brain inherits the default
    // `step_metered` (NotReported). Explicit, NOT blanket, so the vendor `LlmAgentRuntime` override
    // does not collide under coherence.
    impl MeteredRuntime for Mock {}

    impl Agent for Mock {
        fn handle(&self, _inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome {
            // Drive one `step` so the `&dyn AgentRuntime` seam is exercised (dynamic dispatch).
            let _ = runtime.step(&Conversation::default());
            RunOutcome("done".into())
        }
    }

    impl ToolHands for Mock {
        fn exec(&self, _cmd: Command) -> ToolResult {
            // SimHands marker — proves it went through the trait, not a host shell (no-host-exec).
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
            // A deterministic Applied exercises the EffectResult value type; the plan-then-apply
            // pipeline body lands with the runtime.
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
            // The column exists; a test value here is not the frozen default — it proves the field
            // round-trips.
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

    // ───────── value-type mutation-score floor: ToolDef / EffectResult / StepOutcome / EffectKind ─
    //
    // These value types are PURE and must be mutation-covered: every mutation of a variant / field /
    // tag is killed by a test. The tests below discharge that obligation directly (a full
    // `cargo-mutants` run over the crate is the substrate mutation-harness's job).

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
