//! # `LlmAgentRuntime` — the productionized spike kernel on the real [`AgentRuntime`] seam.
//!
//! Wraps a [`ModelClient`] and implements [`AgentRuntime::step`]: it translates the platform-owned
//! [`Conversation`] (system + turns + tool results + the available tool specs) into a
//! [`ModelRequest`], calls the client, and maps the reply to [`StepOutcome::UseTools`] or
//! [`StepOutcome::Submit`]. This is the exact spike loop-body, one decision at a time — the
//! multi-turn DRIVING loop is a separate later slice in the service (it is NOT built here).
//!
//! ## Conversation → request mapping, and the seam limits it exposes (READ THIS)
//! The frozen [`myelin_agent`] seam carries LESS structure than either vendor wire needs. The
//! mapping is therefore best-effort and LOSSY in named places — the honest slice-1 state:
//!
//! - **`Conversation` has no distinct user-goal turn** — the task is folded into `SystemContext`
//!   (`{system}`), so the request's `system` carries the whole framing and (on the first step) there
//!   are no prior turns. This is faithful for Tier-0 single decisions.
//! - **`ToolSchema(String)` is an opaque name-only newtype** — it carries no description and no JSON
//!   Schema, so each [`ToolSpec`] is built with an empty description + a permissive object schema.
//!   The rich schema lives on `ToolDef.input_schema`, which the `Conversation` does NOT carry (it
//!   carries `ToolSchema`); wiring the real schema through is a seam-widening follow-on.
//! - **`ToolCall(ToolName)` and `ToolResult(String)` carry no call id and no arguments** — so a
//!   reconstructed prior turn cannot recover the provider's real call ids/arguments. We synthesize
//!   deterministic ids (`call_<turn>_<i>`) and match tool results positionally to the preceding
//!   assistant turn, which keeps the reconstructed OpenAI request WIRE-VALID for a well-formed
//!   (assistant-tools → results) history, but it is NOT the provider's original id/argument data.
//! - **On the way out, `StepOutcome::UseTools(Vec<ToolCall>)` can hold only tool NAMES** — the
//!   model's chosen call ids and arguments are DROPPED when we map a [`ModelReply::ToolCalls`] into
//!   the seam. The real multi-turn loop needs the seam widened (or a platform-side side-channel) to
//!   carry ids+arguments so a tool result can be routed and linked back. Flagged, not hidden.
//!
//! ## Fail-closed at a sync, non-`Result` seam
//! [`AgentRuntime::step`] returns a bare [`StepOutcome`] (no `Result`) and cannot panic, so a model
//! error cannot be surfaced THROUGH it. [`LlmAgentRuntime`] therefore also exposes
//! [`LlmAgentRuntime::try_step`] returning a [`StepReport`] (`{outcome, usage}`) or a
//! [`ModelError`] — the future metering loop calls THAT to observe usage ([`Usage::NotReported`] →
//! fail the run closed) and the error. The trait `step` degrades an error to a terminal fail-closed
//! `Submit` (no further paid calls), which safely ends the bounded loop.

use crate::client::{
    ModelClient, ModelError, ModelReply, ModelRequest, ModelTurn, ToolCallRequest, ToolCallResult,
    ToolSpec, Usage,
};
use myelin_agent::{
    AgentRuntime, Conversation, StepOutcome, Submission, ToolCall, ToolName, ToolSchema, Turn,
};

/// One step's full result: the seam decision PLUS the provider usage report the metering slice
/// needs. Returned by [`LlmAgentRuntime::try_step`]; the trait `step` projects out `outcome`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepReport {
    /// The seam decision (use tools, or submit).
    pub outcome: StepOutcome,
    /// The provider's token accounting for this step ([`Usage::NotReported`] ⇒ caller fails closed).
    pub usage: Usage,
}

/// **THE REAL BRAIN** — an [`AgentRuntime`] backed by a [`ModelClient`]. Constructible into the
/// `Box<dyn AgentRuntime + Send + Sync>` the service's `select_runtime` returns, so it slots in as a
/// third arm behind the frozen seam with no caller change (see the wiring note in the crate docs).
pub struct LlmAgentRuntime {
    client: Box<dyn ModelClient + Send + Sync>,
    max_output_tokens: Option<u32>,
}

impl LlmAgentRuntime {
    /// Wrap a model client (the default per-call output ceiling is the provider default).
    pub fn new(client: Box<dyn ModelClient + Send + Sync>) -> LlmAgentRuntime {
        LlmAgentRuntime {
            client,
            max_output_tokens: None,
        }
    }

    /// Set a per-call output-token ceiling that bounds single-call overshoot (product plan §2).
    pub fn with_max_output_tokens(mut self, max: u32) -> LlmAgentRuntime {
        self.max_output_tokens = Some(max);
        self
    }

    /// **The observable step** — the decision AND the usage report (or a typed error). The future
    /// metering loop calls this so it can meter usage and fail closed on [`Usage::NotReported`] or a
    /// [`ModelError`]; the trait [`AgentRuntime::step`] is a fail-closed projection of it.
    pub fn try_step(&self, conv: &Conversation) -> Result<StepReport, ModelError> {
        let request = build_request(conv, self.max_output_tokens);
        let response = self.client.complete(&request)?;
        let outcome = map_reply(response.reply);
        Ok(StepReport {
            outcome,
            usage: response.usage,
        })
    }
}

impl AgentRuntime for LlmAgentRuntime {
    fn step(&self, conv: &Conversation) -> StepOutcome {
        match self.try_step(conv) {
            Ok(report) => report.outcome,
            // Fail closed: a terminal Submit ends the bounded loop with no further paid call. The
            // error detail is preserved for the trace; the run is marked failed by the loop that
            // calls try_step (which sees the real ModelError).
            Err(e) => StepOutcome::Submit(Submission(format!(
                "agent runtime error (fail-closed, run aborted): {e}"
            ))),
        }
    }
}

/// Translate the platform-owned [`Conversation`] into a normalized [`ModelRequest`]. See the module
/// docs for the seam limits this mapping exposes (name-only tools, synthesized call ids, dropped
/// arguments).
pub(crate) fn build_request(conv: &Conversation, max_output_tokens: Option<u32>) -> ModelRequest {
    let tools = conv
        .tools
        .iter()
        .map(|ToolSchema(name)| ToolSpec {
            name: name.clone(),
            // The opaque `ToolSchema` newtype carries no description/schema; the rich schema lives on
            // `ToolDef.input_schema`, which the Conversation does not carry (seam limit — see docs).
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
        })
        .collect();

    let mut turns = Vec::new();
    for (index, turn) in conv.turns.iter().enumerate() {
        match turn {
            Turn::Model(StepOutcome::UseTools(calls)) => {
                let tool_calls = calls
                    .iter()
                    .enumerate()
                    .map(|(i, ToolCall(ToolName(name)))| ToolCallRequest {
                        // Synthesized, deterministic id — the seam does not carry the real one.
                        id: format!("call_{index}_{i}"),
                        name: name.clone(),
                        // The seam does not carry the model's chosen arguments.
                        arguments: serde_json::Value::Null,
                    })
                    .collect();
                turns.push(ModelTurn::Assistant {
                    content: None,
                    tool_calls,
                });
            }
            // A terminal Submit inside history is not a mid-conversation turn to replay; surface any
            // text as a prior assistant message so the model sees its own earlier answer.
            Turn::Model(StepOutcome::Submit(Submission(text))) => {
                turns.push(ModelTurn::Assistant {
                    content: Some(text.clone()),
                    tool_calls: Vec::new(),
                });
            }
            Turn::ToolResults(results) => {
                // Match each result positionally to the preceding assistant turn's synthesized ids.
                let results = results
                    .iter()
                    .enumerate()
                    .map(|(i, myelin_agent::ToolResult(content))| ToolCallResult {
                        id: format!("call_{}_{i}", index.saturating_sub(1)),
                        content: content.clone(),
                    })
                    .collect();
                turns.push(ModelTurn::ToolResults(results));
            }
            Turn::Approval(note) => {
                // An HITL approval outcome is context the model should see; surface it as a user note.
                turns.push(ModelTurn::User {
                    content: format!("[human approval] {}", note.0),
                });
            }
        }
    }

    ModelRequest {
        system: conv.system.0.clone(),
        turns,
        tools,
        max_output_tokens,
    }
}

/// Map a normalized [`ModelReply`] to the seam's [`StepOutcome`]. Tool calls collapse to tool NAMES
/// (the seam's `ToolCall(ToolName)` carries no id/arguments — see the module docs); a final answer
/// becomes a [`Submission`].
pub(crate) fn map_reply(reply: ModelReply) -> StepOutcome {
    match reply {
        ModelReply::ToolCalls(calls) => StepOutcome::UseTools(
            calls
                .into_iter()
                .map(|c| ToolCall(ToolName(c.name)))
                .collect(),
        ),
        ModelReply::Final { content } => StepOutcome::Submit(Submission(content)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ModelReply, ModelResponse, Usage};
    use crate::mock::MockModelClient;
    use myelin_agent::{BudgetView, SystemContext};

    fn conv_with_tools() -> Conversation {
        Conversation {
            system: SystemContext("you are labelled as an agent".into()),
            turns: vec![],
            tools: vec![ToolSchema("search".into()), ToolSchema("read_file".into())],
            budget: BudgetView(1000),
        }
    }

    #[test]
    fn tool_call_reply_maps_to_use_tools() {
        let client = MockModelClient::ok(ModelResponse {
            reply: ModelReply::ToolCalls(vec![ToolCallRequest {
                id: "call_x".into(),
                name: "search".into(),
                arguments: serde_json::json!({"q": "panic"}),
            }]),
            usage: Usage::Reported {
                input: 50,
                cached_input: 0,
                output: 8,
            },
        });
        let runtime = LlmAgentRuntime::new(Box::new(client));

        let report = runtime.try_step(&conv_with_tools()).unwrap();
        assert_eq!(
            report.outcome,
            StepOutcome::UseTools(vec![ToolCall(ToolName("search".into()))])
        );
        assert!(matches!(report.usage, Usage::Reported { .. }));
        // The trait step projects out the same outcome.
        assert_eq!(
            runtime.step(&conv_with_tools()),
            StepOutcome::UseTools(vec![ToolCall(ToolName("search".into()))])
        );
    }

    #[test]
    fn final_reply_maps_to_submit() {
        let client = MockModelClient::ok(ModelResponse {
            reply: ModelReply::Final {
                content: "the bug is at foo.rs:10".into(),
            },
            usage: Usage::Reported {
                input: 60,
                cached_input: 10,
                output: 12,
            },
        });
        let runtime = LlmAgentRuntime::new(Box::new(client));
        let report = runtime.try_step(&conv_with_tools()).unwrap();
        assert_eq!(
            report.outcome,
            StepOutcome::Submit(Submission("the bug is at foo.rs:10".into()))
        );
    }

    #[test]
    fn not_reported_usage_is_surfaced_for_the_caller_to_fail_closed() {
        let client = MockModelClient::ok(ModelResponse {
            reply: ModelReply::Final {
                content: "done".into(),
            },
            usage: Usage::NotReported,
        });
        let runtime = LlmAgentRuntime::new(Box::new(client));
        let report = runtime.try_step(&conv_with_tools()).unwrap();
        // The runtime does NOT fabricate a count — it hands NotReported up so the metering loop can
        // fail the run closed.
        assert_eq!(report.usage, Usage::NotReported);
    }

    #[test]
    fn transport_error_try_step_errors_and_step_fails_closed() {
        let client = MockModelClient::err(ModelError::Http {
            status: 500,
            body: "upstream boom".into(),
        });
        let runtime = LlmAgentRuntime::new(Box::new(client));

        // try_step surfaces the typed error (the loop marks the run failed).
        assert!(matches!(
            runtime.try_step(&conv_with_tools()),
            Err(ModelError::Http { status: 500, .. })
        ));
        // The trait step never panics: it degrades to a terminal fail-closed Submit.
        match runtime.step(&conv_with_tools()) {
            StepOutcome::Submit(Submission(text)) => {
                assert!(text.contains("fail-closed"));
                assert!(!text.contains("500") || text.contains("aborted"));
            }
            other => panic!("expected a fail-closed Submit, got {other:?}"),
        }
    }

    #[test]
    fn build_request_maps_system_and_tools_and_reconstructs_history() {
        let mut conv = conv_with_tools();
        conv.turns.push(Turn::Model(StepOutcome::UseTools(vec![
            ToolCall(ToolName("search".into())),
        ])));
        conv.turns.push(Turn::ToolResults(vec![myelin_agent::ToolResult(
            "match at foo.rs:10".into(),
        )]));

        let request = build_request(&conv, Some(128));
        assert_eq!(request.system, "you are labelled as an agent");
        assert_eq!(request.tools.len(), 2);
        assert_eq!(request.tools[0].name, "search");
        assert_eq!(request.max_output_tokens, Some(128));

        // The assistant turn's synthesized id must match the tool-result's linked id (wire-valid).
        match (&request.turns[0], &request.turns[1]) {
            (
                ModelTurn::Assistant { tool_calls, .. },
                ModelTurn::ToolResults(results),
            ) => {
                assert_eq!(tool_calls[0].id, results[0].id);
            }
            other => panic!("unexpected reconstruction: {other:?}"),
        }
    }

    #[test]
    fn runtime_is_boxable_as_the_select_runtime_return_type() {
        // Proves LlmAgentRuntime slots into `Box<dyn AgentRuntime + Send + Sync>` — the exact type
        // `myelin_agent_service::mock::select_runtime` returns (the third-arm wiring; see crate docs).
        let client = MockModelClient::ok(ModelResponse {
            reply: ModelReply::Final {
                content: "ok".into(),
            },
            usage: Usage::NotReported,
        });
        let boxed: Box<dyn AgentRuntime + Send + Sync> =
            Box::new(LlmAgentRuntime::new(Box::new(client)));
        assert!(matches!(
            boxed.step(&Conversation::default()),
            StepOutcome::Submit(_)
        ));
    }
}
