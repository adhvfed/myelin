use crate::client::{
    ModelClient, ModelError, ModelReply, ModelRequest, ModelTurn, ToolCallRequest, ToolCallResult,
    ToolSpec, Usage,
};
use myelin_agent::{
    AgentRuntime, Conversation, MeteredRuntime, MeteredStep, RuntimeStepError, StepOutcome,
    Submission, TokenUsage, ToolCall, ToolCallId, ToolName, Turn,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepReport {
    pub outcome: StepOutcome,
    pub usage: Usage,
}

pub struct LlmAgentRuntime {
    client: Box<dyn ModelClient + Send + Sync>,
    max_output_tokens: Option<u32>,
}

impl LlmAgentRuntime {
    pub fn new(client: Box<dyn ModelClient + Send + Sync>) -> LlmAgentRuntime {
        LlmAgentRuntime {
            client,
            max_output_tokens: None,
        }
    }

    pub fn with_max_output_tokens(mut self, max: u32) -> LlmAgentRuntime {
        self.max_output_tokens = Some(max);
        self
    }

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
            Err(e) => StepOutcome::Submit(Submission(format!(
                "agent runtime error (fail-closed, run aborted): {}",
                e.runtime_step_error(),
            ))),
        }
    }
}

impl MeteredRuntime for LlmAgentRuntime {
    fn step_metered(&self, conv: &Conversation) -> Result<MeteredStep, RuntimeStepError> {
        match self.try_step(conv) {
            Ok(report) => Ok(MeteredStep {
                outcome: report.outcome,
                usage: map_usage(report.usage),
            }),
            Err(error) => Err(error.runtime_step_error()),
        }
    }
}

pub(crate) fn map_usage(usage: Usage) -> TokenUsage {
    match usage {
        Usage::Reported {
            input,
            cached_input,
            output,
        } => TokenUsage::Reported {
            input,
            cached_input,
            output,
        },
        Usage::NotReported => TokenUsage::NotReported,
    }
}

pub(crate) fn build_request(conv: &Conversation, max_output_tokens: Option<u32>) -> ModelRequest {
    let tools = conv
        .tools
        .iter()
        .map(|schema| ToolSpec {
            name: schema.name.0.clone(),
            description: schema.description.clone(),
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
                        id: call.id.0.clone(),
                        name: call.name.0.clone(),
                        arguments: call.arguments.clone(),
                    })
                    .collect();
                turns.push(ModelTurn::Assistant {
                    content: None,
                    tool_calls,
                });
            }
            Turn::Model(StepOutcome::Submit(Submission(text))) => {
                turns.push(ModelTurn::Assistant {
                    content: Some(text.clone()),
                    tool_calls: Vec::new(),
                });
            }
            Turn::ToolResults(results) => {
                let results = results
                    .iter()
                    .map(|outcome| ToolCallResult {
                        id: outcome.call_id.0.clone(),
                        content: outcome.result.content().to_string(),
                        is_error: outcome.result.is_refused(),
                    })
                    .collect();
                turns.push(ModelTurn::ToolResults(results));
            }
            Turn::Approval(note) => {
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
        assert_eq!(
            report.outcome,
            StepOutcome::UseTools(vec![ToolCall {
                id: ToolCallId("call_x".into()),
                name: ToolName("search".into()),
                arguments: serde_json::json!({"q": "panic"}),
            }])
        );
        assert!(matches!(report.usage, Usage::Reported { .. }));
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
        assert_eq!(report.usage, Usage::NotReported);
    }

    #[test]
    fn transport_error_try_step_errors_and_step_fails_closed() {
        let client = MockModelClient::err(ModelError::Http {
            status: 500,
            body: "upstream boom".into(),
        });
        let runtime = LlmAgentRuntime::new(Box::new(client));

        assert!(matches!(
            runtime.try_step(&conv_with_tools()),
            Err(ModelError::Http { status: 500, .. })
        ));
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
            result: ToolResult::Succeeded("match at foo.rs:10".into()),
        }]));

        let request = build_request(&conv, Some(128));
        assert_eq!(request.system, "you are labelled as an agent");
        assert_eq!(request.tools.len(), 2);
        assert_eq!(request.tools[0].name, "search");
        assert_eq!(request.tools[0].description, "full-text search");
        assert_eq!(request.tools[0].input_schema["type"], "object");
        assert_eq!(request.max_output_tokens, Some(128));

        match (&request.turns[0], &request.turns[1]) {
            (ModelTurn::Assistant { tool_calls, .. }, ModelTurn::ToolResults(results)) => {
                assert_eq!(tool_calls[0].id, "call_abc");
                assert_eq!(tool_calls[0].id, results[0].id);
                assert_eq!(tool_calls[0].arguments, serde_json::json!({"q": "panic"}));
                assert!(!results[0].is_error);
            }
            other => panic!("unexpected reconstruction: {other:?}"),
        }
    }

    #[test]
    fn step_metered_reports_the_mapped_provider_counts() {
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
        let metered = runtime
            .step_metered(&conv_with_tools())
            .expect("the provider completes the metered step");
        assert_eq!(
            metered.outcome,
            StepOutcome::Submit(Submission("the bug is at foo.rs:10".into()))
        );
        assert_eq!(
            metered.usage,
            TokenUsage::Reported {
                input: 60,
                cached_input: 10,
                output: 12,
            }
        );
    }

    #[test]
    fn step_metered_maps_not_reported_usage_to_token_usage_not_reported() {
        let client = MockModelClient::ok(ModelResponse {
            reply: ModelReply::Final {
                content: "done".into(),
            },
            usage: Usage::NotReported,
        });
        let runtime = LlmAgentRuntime::new(Box::new(client));
        assert_eq!(
            runtime
                .step_metered(&conv_with_tools())
                .expect("the provider completes without reporting usage")
                .usage,
            TokenUsage::NotReported
        );
    }

    #[test]
    fn step_metered_preserves_provider_failure_without_exposing_its_body() {
        let client = MockModelClient::err(ModelError::Http {
            status: 500,
            body: "secret upstream diagnostics".into(),
        });
        let runtime = LlmAgentRuntime::new(Box::new(client));
        let error = runtime
            .step_metered(&conv_with_tools())
            .expect_err("a provider failure is not a model submission");
        assert_eq!(error, RuntimeStepError::Rejected { status: Some(500) });
        assert_eq!(error.code(), "runtime_rejected");
        assert!(!error.to_string().contains("secret upstream diagnostics"));
    }

    #[test]
    fn runtime_is_boxable_as_the_select_runtime_return_type() {
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
