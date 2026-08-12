use myelin_agent::{
    AgentRuntime, BudgetView, Conversation, StepOutcome, Submission, SystemContext, ToolCall,
    ToolCallId, ToolName, ToolOutcome, ToolResult, ToolSchema, Turn,
};
use myelin_agent_service::{
    build_conversation, model_turns_taken, replay, select_runtime, MockAgentRuntime, MockScript,
    RuntimeFlag, TraceHistory,
};

fn schema(name: &str) -> ToolSchema {
    ToolSchema {
        name: ToolName(name.into()),
        description: String::new(),
        input_schema: "{}".into(),
    }
}

fn call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId(format!("call:{name}")),
        name: ToolName(name.into()),
        arguments: serde_json::Value::Null,
    }
}

fn outcome(name: &str) -> ToolOutcome {
    ToolOutcome {
        call_id: ToolCallId(format!("call:{name}")),
        result: ToolResult::Succeeded(format!("tool:{name}:result")),
    }
}

fn script() -> MockScript {
    MockScript::new(
        SystemContext("you are agent-7; you are labelled as an agent".into()),
        vec![schema("search"), schema("read")],
        BudgetView(100),
        vec![
            StepOutcome::UseTools(vec![call("search")]),
            StepOutcome::UseTools(vec![call("read")]),
            StepOutcome::Submit(Submission("the answer".into())),
        ],
    )
}

#[test]
fn cdc_8_3_mock_runtime_is_a_pure_function_of_the_conversation() {
    let provider = MockAgentRuntime::new(script());

    let opening = Conversation::default();
    match provider.step(&opening) {
        StepOutcome::UseTools(calls) => {
            assert_eq!(calls, vec![call("search")]);
        }
        other => panic!("expected UseTools (search) on the opening turn, got {other:?}"),
    }

    let mut conv = Conversation::default();
    conv.turns
        .push(Turn::Model(StepOutcome::UseTools(vec![call("search")])));
    conv.turns.push(Turn::ToolResults(vec![outcome("search")]));
    assert_eq!(
        model_turns_taken(&conv),
        1,
        "one model turn taken (the tool-result turn does not count)"
    );
    assert_eq!(
        provider.step(&conv),
        StepOutcome::UseTools(vec![call("read")]),
        "after one model turn the brain replays step[1] (read)"
    );

    conv.turns
        .push(Turn::Model(StepOutcome::UseTools(vec![call("read")])));
    conv.turns.push(Turn::ToolResults(vec![outcome("read")]));
    assert!(
        matches!(provider.step(&conv), StepOutcome::Submit(_)),
        "after two model turns the brain submits (it only ever PROPOSES - plan-then-apply survives)"
    );
}

#[test]
fn ag_d9_golden_replay_is_byte_identical_across_runs() {
    let s = script();
    let first = replay(&s);
    let second = replay(&s);
    assert_eq!(
        first, second,
        "AG-D9: two replays of the same script are byte-identical"
    );

    assert_eq!(
        first.outcomes,
        vec![
            StepOutcome::UseTools(vec![call("search")]),
            StepOutcome::UseTools(vec![call("read")]),
            StepOutcome::Submit(Submission("the answer".into())),
        ],
        "the golden StepOutcome stream is the scripted queue, in order"
    );
    assert!(
        first.terminated,
        "a well-formed script terminates the bounded loop"
    );
    assert_eq!(first.submission, Some(Submission("the answer".into())));
}

#[test]
fn cdc_8_3_use_mock_flag_is_a_real_flag_on_the_same_seam() {
    let flag = RuntimeFlag::from_args(["myelin-agent", "serve", "--use-mock"]);
    assert!(flag.is_mock(), "--use-mock parses to the mock runtime");

    let brain: Box<dyn AgentRuntime + Send + Sync> =
        select_runtime(flag, MockScript::submit_only("sys", "ok"));
    assert_eq!(
        brain.step(&Conversation::default()),
        StepOutcome::Submit(Submission("ok".into())),
        "the selected mock brain replays its script through the frozen &dyn seam"
    );
}

#[test]
fn cdc_8_3_build_conversation_reconstructs_from_platform_history() {
    let s = script();
    let mut history = TraceHistory::new();
    history.push_model(StepOutcome::UseTools(vec![call("search")]));
    history.push_tool_results(vec![outcome("search")]);

    let conv = build_conversation(&s, &history);
    assert_eq!(
        conv.system,
        SystemContext("you are agent-7; you are labelled as an agent".into())
    );
    assert_eq!(
        conv.tools.len(),
        2,
        "the scoped tool list is rebuilt from the script"
    );
    assert_eq!(
        conv.budget,
        BudgetView(100),
        "the budget view is rebuilt from the script"
    );
    assert_eq!(
        conv.turns.len(),
        2,
        "the transcript is the model step + its routed tool result"
    );
    assert_eq!(
        build_conversation(&s, &history),
        conv,
        "the reconstruction is deterministic"
    );
}
