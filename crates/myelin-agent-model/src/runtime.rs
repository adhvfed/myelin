//! # `LlmAgentRuntime` — the productionized spike kernel on the real [`AgentRuntime`] seam.
//!
//! Wraps a [`ModelClient`] and implements [`AgentRuntime::step`]: it translates the platform-owned
//! [`Conversation`] (system + turns + tool results + the available tool specs) into a
//! [`ModelRequest`], calls the client, and maps the reply to [`StepOutcome::UseTools`] or
//! [`StepOutcome::Submit`]. This is the exact spike loop-body, one decision at a time — the
//! multi-turn DRIVING loop is a separate later slice in the service (it is NOT built here).
//!
//! ## Conversation → request mapping (the widened seam carries real ids/arguments/schemas)
//! The [`myelin_agent`] seam now carries the structure a real tool-calling loop needs, so the
//! mapping is faithful — no synthesized ids, no positional matching, no dropped arguments:
//!
//! - **`Conversation` has no distinct user-goal turn** — the task is folded into `SystemContext`
//!   (`{system}`), so the request's `system` carries the whole framing and (on the first step) there
//!   are no prior turns. This is faithful for Tier-0 single decisions.
//! - **`ToolSchema { name, description, input_schema }` carries the real tool spec** — each
//!   [`ToolSpec`] is built from the tool's real description + its JSON-schema string (mirroring
//!   `ToolDef.input_schema`), parsed into the normalized [`ToolSpec::input_schema`] object.
//! - **`ToolCall { id, name, arguments }` and `ToolOutcome { call_id, result }` carry the real
//!   linkage** — a reconstructed assistant turn replays the model's own call ids + chosen arguments,
//!   and each tool result is keyed back to its call by the real `call_id` (no positional matching).
//! - **On the way out, `StepOutcome::UseTools(Vec<ToolCall>)` carries the model's real ids +
//!   arguments** — a [`ModelReply::ToolCalls`] maps straight through, so the next turn can route each
//!   call and link its result back by id.
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
    AgentRuntime, Conversation, StepOutcome, Submission, ToolCall, ToolCallId, ToolName, Turn,
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

/// Translate the platform-owned [`Conversation`] into a normalized [`ModelRequest`]. The widened
/// seam carries the tool spec (name + description + schema), the model's real call ids/arguments, and
/// each result keyed back to its call — so the mapping is faithful (see the module docs).
pub(crate) fn build_request(conv: &Conversation, max_output_tokens: Option<u32>) -> ModelRequest {
    let tools = conv
        .tools
        .iter()
        .map(|schema| ToolSpec {
            name: schema.name.0.clone(),
            description: schema.description.clone(),
            // `ToolSchema.input_schema` mirrors `ToolDef.input_schema` (a JSON-schema string); parse
            // it to the normalized object carrier, falling back to a permissive object schema if the
            // string is empty/unparseable so the request stays wire-valid.
            input_schema: serde_json::from_str(&schema.input_schema)
                .unwrap_or_else(|_| serde_json::json!({"type": "object"})),
        })
        .collect();

    let mut turns = Vec::new();
    for turn in conv.turns.iter() {
        match turn {
            Turn::Model(StepOutcome::UseTools(calls)) => {
                let tool_calls = calls
                    .iter()
                    .map(|call| ToolCallRequest {
                        // The model's own call id — carried through the seam, not synthesized.
                        id: call.id.0.clone(),
                        name: call.name.0.clone(),
                        // The model's chosen arguments — carried through the seam.
                        arguments: call.arguments.clone(),
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
                // Each result is keyed back to its call by the real `call_id` (no positional match).
                let results = results
                    .iter()
                    .map(|outcome| ToolCallResult {
                        id: outcome.call_id.0.clone(),
                        content: outcome.result.0.clone(),
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

/// Map a normalized [`ModelReply`] to the seam's [`StepOutcome`]. Each tool call carries the model's
/// real `{id, name, arguments}` straight through the widened seam so the next turn can route it and
/// link its result back by id; a final answer becomes a [`Submission`].
pub(crate) fn map_reply(reply: ModelReply) -> StepOutcome {
    match reply {
        ModelReply::ToolCalls(calls) => StepOutcome::UseTools(
            calls
                .into_iter()
                .map(|c| ToolCall {
                    id: ToolCallId(c.id),
                    name: ToolName(c.name),
                    arguments: c.arguments,
                })
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
    use myelin_agent::{BudgetView, SystemContext, ToolOutcome, ToolResult, ToolSchema};

    fn conv_with_tools() -> Conversation {
        Conversation {
            system: SystemContext("you are labelled as an agent".into()),
            turns: vec![],
            tools: vec![
                ToolSchema {
                    name: ToolName("search".into()),
                    description: "full-text search".into(),
                    input_schema: r#"{"type":"object","properties":{"q":{"type":"string"}}}"#
                        .into(),
                },
                ToolSchema {
                    name: ToolName("read_file".into()),
                    description: "read a file".into(),
                    input_schema: "{}".into(),
                },
            ],
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
        // The model's real id + chosen arguments ride straight through the widened seam.
        assert_eq!(
            report.outcome,
            StepOutcome::UseTools(vec![ToolCall {
                id: ToolCallId("call_x".into()),
                name: ToolName("search".into()),
                arguments: serde_json::json!({"q": "panic"}),
            }])
        );
        assert!(matches!(report.usage, Usage::Reported { .. }));
        // The trait step projects out the same outcome.
        assert_eq!(
            runtime.step(&conv_with_tools()),
            StepOutcome::UseTools(vec![ToolCall {
                id: ToolCallId("call_x".into()),
                name: ToolName("search".into()),
                arguments: serde_json::json!({"q": "panic"}),
            }])
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
        conv.turns
            .push(Turn::Model(StepOutcome::UseTools(vec![ToolCall {
                id: ToolCallId("call_abc".into()),
                name: ToolName("search".into()),
                arguments: serde_json::json!({"q": "panic"}),
            }])));
        conv.turns.push(Turn::ToolResults(vec![ToolOutcome {
            call_id: ToolCallId("call_abc".into()),
            result: ToolResult("match at foo.rs:10".into()),
        }]));

        let request = build_request(&conv, Some(128));
        assert_eq!(request.system, "you are labelled as an agent");
        assert_eq!(request.tools.len(), 2);
        assert_eq!(request.tools[0].name, "search");
        // The real tool description + parsed JSON-schema ride through (no longer name-only).
        assert_eq!(request.tools[0].description, "full-text search");
        assert_eq!(request.tools[0].input_schema["type"], "object");
        assert_eq!(request.max_output_tokens, Some(128));

        // The assistant turn's real call id matches the tool-result's linked id (wire-valid).
        match (&request.turns[0], &request.turns[1]) {
            (ModelTurn::Assistant { tool_calls, .. }, ModelTurn::ToolResults(results)) => {
                assert_eq!(tool_calls[0].id, "call_abc");
                assert_eq!(tool_calls[0].id, results[0].id);
                // The model's chosen arguments ride through too (not dropped/nulled).
                assert_eq!(tool_calls[0].arguments, serde_json::json!({"q": "panic"}));
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
