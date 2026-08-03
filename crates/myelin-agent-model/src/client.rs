//! # `ModelClient` — the vendor abstraction (the normalized model wire, NOT Luna-shaped).
//!
//! This is the durable seam between the platform's agent runtime and a concrete model provider. It
//! is deliberately shaped so BOTH vendor wire protocols normalize onto it — Luna is OpenAI
//! `/v1/chat/completions` (with `reasoning_effort: "none"`), and a future `AnthropicClient` is
//! native Messages tool-use — WITHOUT changing this trait:
//!
//! - [`ModelRequest`] carries a `system` framing + prior [`ModelTurn`]s (user text, prior assistant
//!   text/tool-calls, and tool results linked by id) + the available [`ToolSpec`]s. Anthropic maps
//!   `system` → the top-level `system`, turns → `messages` content blocks (`text` / `tool_use` /
//!   `tool_result`), and tools → `tools` with an `input_schema` — a pure re-serialization.
//! - [`ModelResponse`] is either [`ModelReply::ToolCalls`] or a final [`ModelReply::Final`], plus a
//!   [`Usage`] report. Anthropic's `stop_reason == "tool_use"` → `ToolCalls`; its
//!   `usage.{input_tokens, output_tokens, cache_read_input_tokens}` → [`Usage::Reported`].
//!
//! **Usage is never fabricated.** A provider that omits its usage block surfaces [`Usage::NotReported`]
//! so the caller (the future metering loop) can FAIL CLOSED rather than estimate a bill — the ovim
//! `AgentReported<T> = Reported | NotReported` discipline (product plan §2). Raw token COUNTS are
//! reported here; pricing (wholesale/markup in micro-units) lives in a later metering slice, not this
//! crate.

use serde::{Deserialize, Serialize};

/// One tool the model may call, as the client presents it to the provider (a normalized, vendor-
/// neutral tool spec). The Luna adapter renders this as an OpenAI `function` tool; an Anthropic
/// adapter would render the SAME fields as a Messages `tool` with an `input_schema`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// The tool name the provider will echo back on a call.
    pub name: String,
    /// A human/model-readable description of what the tool does.
    pub description: String,
    /// JSON Schema for the tool's arguments (an object schema). Both vendors take a JSON-Schema
    /// parameters block; this is the one normalized carrier.
    pub input_schema: serde_json::Value,
}

/// A tool call the MODEL requested (on the way out) or that a prior assistant turn made (on the way
/// in). Carries the provider's opaque call `id` (so a later tool result can be linked back to it),
/// the tool `name`, and the parsed `arguments` object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// The provider's opaque call id (OpenAI `tool_calls[].id` / Anthropic `tool_use.id`). Links a
    /// later [`ToolCallResult`] back to this call.
    pub id: String,
    /// The tool the model wants to call.
    pub name: String,
    /// The parsed arguments the model chose (an object). Best-effort JSON; `Null` if the provider
    /// emitted no/invalid arguments.
    pub arguments: serde_json::Value,
}

/// The result of executing a tool call, fed back into the next request. Linked to its
/// [`ToolCallRequest`] by `id` (both vendors require the linkage: OpenAI a `tool` message with a
/// `tool_call_id`; Anthropic a `tool_result` block with a `tool_use_id`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// The [`ToolCallRequest::id`] this result answers.
    pub id: String,
    /// The tool's output text (already bounded/summarized by the platform before it reaches here).
    pub content: String,
}

/// One turn of the normalized conversation the client sends. The platform owns history; this is the
/// vendor-neutral projection of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelTurn {
    /// A user/task message (or a platform note surfaced to the model, e.g. an approval outcome).
    User { content: String },
    /// A prior assistant step: free text and/or one or more tool-call requests.
    Assistant {
        /// The assistant's free-text content, if any.
        content: Option<String>,
        /// The tool calls the assistant made this turn (empty for a pure-text turn).
        tool_calls: Vec<ToolCallRequest>,
    },
    /// The results of the tool calls the immediately-preceding assistant turn requested.
    ToolResults(Vec<ToolCallResult>),
}

/// The normalized request handed to a [`ModelClient`] (system + prior turns + tool results + the
/// available tool specs). Vendor-neutral: the concrete adapter serializes it to its own wire shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelRequest {
    /// The system framing (task, role, the labelled-as-agent notice). Both vendors carry a system.
    pub system: String,
    /// The platform-owned prior turns (empty on the first step).
    pub turns: Vec<ModelTurn>,
    /// The tools THIS step may call (already permission/delegation-scoped upstream).
    pub tools: Vec<ToolSpec>,
    /// A per-call output-token ceiling that bounds single-call overshoot (product plan §2: a
    /// per-call `max_tokens` ceiling). `None` leaves it to the provider default.
    pub max_output_tokens: Option<u32>,
}

/// The model's per-step decision: call tools, or answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelReply {
    /// The model wants to call these tools; step it again with their results.
    ToolCalls(Vec<ToolCallRequest>),
    /// The model produced a final answer (no tool call) — the run may submit.
    Final { content: String },
}

/// The provider's token accounting for one call. **Never fabricated** — a provider that omits it
/// surfaces [`Usage::NotReported`] so the caller fails closed rather than estimating a bill.
///
/// Counts are RAW provider token counts; pricing (micro-unit wholesale/markup) is a later slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Usage {
    /// The provider reported token counts. `input` is the prompt tokens charged at the standard
    /// input rate (i.e. NON-cached); `cached_input` is the tokens served from the prompt cache (a
    /// cheaper tier); `output` is the completion tokens.
    Reported {
        /// Non-cached prompt tokens (standard input tier).
        input: u64,
        /// Cached prompt tokens (cache-hit tier).
        cached_input: u64,
        /// Completion (output) tokens.
        output: u64,
    },
    /// The provider omitted usage. The caller MUST fail the run closed (never estimate).
    NotReported,
}

/// The normalized model response: the decision + the usage report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelResponse {
    /// The model's decision this step.
    pub reply: ModelReply,
    /// The provider's token accounting (or [`Usage::NotReported`]).
    pub usage: Usage,
}

/// A typed model-call error. **Never panics; never carries the API key** (the key rides only in the
/// `Authorization` header and is interpolated into no error).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    /// No API key was available (env var missing/blank) — fail closed before any egress.
    MissingApiKey,
    /// A connect / TLS / timeout / transport failure (the message never includes credentials).
    Transport(String),
    /// The provider returned a non-2xx status; `body` is the (bounded) provider error body.
    Http { status: u16, body: String },
    /// The response could not be parsed into the normalized shape.
    Parse(String),
}

impl core::fmt::Display for ModelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ModelError::MissingApiKey => {
                write!(f, "model API key is not set (fail-closed; no call made)")
            }
            ModelError::Transport(m) => write!(f, "model transport error: {m}"),
            ModelError::Http { status, body } => {
                write!(f, "model provider returned HTTP {status}: {body}")
            }
            ModelError::Parse(m) => write!(f, "model response parse error: {m}"),
        }
    }
}

impl std::error::Error for ModelError {}

/// **THE VENDOR SEAM.** A model provider the platform can call to advance one agent step. Sync (the
/// [`myelin_agent::AgentRuntime::step`] seam it serves is sync); a concrete adapter bridges to its
/// own async transport internally. Implementors MUST NOT panic and MUST NOT log the API key.
pub trait ModelClient {
    /// Run one completion: normalized request in, normalized decision + usage out (or a typed error).
    fn complete(&self, request: &ModelRequest) -> Result<ModelResponse, ModelError>;
}
