use myelin_agent::{
    AgentRuntime, Conversation, StepOutcome, Submission, ToolCall, ToolCallId, ToolName,
};

fn call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId(format!("call:{name}")),
        name: ToolName(name.into()),
        arguments: serde_json::Value::Null,
    }
}

struct ProviderRuntime;

impl AgentRuntime for ProviderRuntime {
    fn step(&self, conv: &Conversation) -> StepOutcome {
        if conv.turns.is_empty() {
            StepOutcome::UseTools(vec![call("search")])
        } else {
            StepOutcome::Submit(Submission("final".into()))
        }
    }
}

#[test]
fn cdc_8_3_step_is_a_pure_function_of_the_conversation() {
    let provider = ProviderRuntime;

    let opening = Conversation::default();
    match provider.step(&opening) {
        StepOutcome::UseTools(calls) => {
            assert_eq!(calls, vec![call("search")]);
        }
        other => panic!("expected UseTools on the opening turn, got {other:?}"),
    }

    let mut later = Conversation::default();
    later
        .turns
        .push(myelin_agent::Turn::Model(StepOutcome::Submit(Submission(
            "x".into(),
        ))));
    assert!(matches!(provider.step(&later), StepOutcome::Submit(_)));
}
