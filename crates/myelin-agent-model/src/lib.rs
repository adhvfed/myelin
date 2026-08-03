//! # `myelin-agent-model` — the vendor brain (the ONE sanctioned model-SDK home).
//!
//! This crate productionizes the proven Luna spike (`scripts/agent-spike/agent.py`) onto the real
//! agent-fabric seam ([`myelin_agent::AgentRuntime`]). It is the slice-1 kernel of the hosted-agent
//! product (`planning/08-release/04-hosted-agent-product-plan.md` §4.1). Three pieces:
//!
//! - [`ModelClient`] — the **vendor abstraction**. A normalized request (system + prior turns +
//!   tool results + tool specs) → a normalized response (tool-call intents or a final submission) +
//!   a [`Usage`] report. Shaped so BOTH wire protocols fit WITHOUT changing it: Luna is OpenAI
//!   `/v1/chat/completions` (`reasoning_effort: "none"`); a future `AnthropicClient` is native
//!   Messages tool-use. Usage is [`Usage::Reported`] `{input, cached_input, output}` or
//!   [`Usage::NotReported`] — never fabricated (fail closed on omission).
//! - [`LunaClient`] — the **real Luna call** over hyper+rustls (the workspace's edge HTTP stack, no
//!   reqwest). Model `gpt-5.6-luna`, `tool_choice: "auto"`, key from `OPENAI_API_KEY` (never logged),
//!   bounded timeout, typed errors ([`ModelError`], never a panic).
//! - [`LlmAgentRuntime`] — the **real brain** on the seam: an [`AgentRuntime`] wrapping a
//!   `Box<dyn ModelClient>`. Its `step(&Conversation)` builds the request, calls the client, and maps
//!   the reply to [`myelin_agent::StepOutcome`]. It ALSO exposes [`LlmAgentRuntime::try_step`]
//!   (`{outcome, usage}` or error) for the future metering loop, since the frozen `step` seam is a
//!   bare, non-`Result` value. **This is a single decision — the multi-turn driving loop is a later
//!   service slice, NOT built here.**
//!
//! ## Why this crate is the `no-llm-in-platform` exception (contract 1.6) — and why that is SAFE
//! The `no-llm-in-platform` lint forbids any model SDK / prompt / model-name string
//! (`openai`, `anthropic`, a `gpt-*`/`claude-*` id, `reasoning_effort`, endpoint URLs, …) everywhere
//! in `crates/*/src` EXCEPT here. That is not a loophole — it is the WHOLE point of the
//! [`AgentRuntime`] strategy seam: the entire rest of the platform stays provider-agnostic (it only
//! ever sees the seam's value types), and every provider-specific string is quarantined in this one
//! crate behind that seam. Swapping Luna → Anthropic, or Anthropic → an EU-hosted model, is a change
//! to THIS crate only; no platform code moves. This crate is therefore named — LOUD, not silent — in
//! the lint's exclusion list (`myelin-lints/tests/workspace_clean.rs` +
//! `myelin-lints/src/bin/lint-gate.rs`), a whole-crate boundary.
//!
//! ## Wiring into the service (`select_runtime`, the third arm)
//! [`LlmAgentRuntime`] is constructible into the exact `Box<dyn AgentRuntime + Send + Sync>` that
//! `myelin_agent_service::mock::select_runtime` returns (proven by a crate test). To wire it as the
//! third `RuntimeFlag` arm, the service adds a `myelin-agent-model` dependency and:
//! `RuntimeFlag::Llm => Box::new(LlmAgentRuntime::new(Box::new(LunaClient::from_env()?)))`.
//! (Left as a one-liner for the orchestrator so this slice does not churn the doc-locked service
//! enum; the fit is verified here.)

mod client;
mod luna;
mod runtime;

#[cfg(any(test, feature = "test-support"))]
pub mod mock;

pub use client::{
    ModelClient, ModelError, ModelReply, ModelRequest, ModelResponse, ModelTurn, ToolCallRequest,
    ToolCallResult, ToolSpec, Usage,
};
pub use luna::LunaClient;
pub use runtime::{LlmAgentRuntime, StepReport};

// Re-export the seam trait so a downstream `use myelin_agent_model::AgentRuntime` reaches the brain
// it implements without a second import of the glue crate.
pub use myelin_agent::AgentRuntime;
