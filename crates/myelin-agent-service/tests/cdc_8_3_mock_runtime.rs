//! # The CDC pair for contract 8.3 — the `--use-mock` `MockAgentRuntime` (AG-P5 → P-217)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.3
//! (`AgentRuntime::step(&Conversation) → UseTools | Submit` — the stateless brain; strategy seam
//! (skeleton/mock/llm); platform owns history; `--use-mock` is a real runtime flag). Owning
//! architecture: `agent-fabric.md` §3.2 (the MockAgentRuntime — deterministic scripted
//! `StepOutcome`s on the real `--use-mock` path; the golden + cargo-mutants lever, AG-4), §2.1 (the
//! brain is stateless; the platform owns the Conversation history).
//!
//! AG-P1 (→ P-130) shipped the SIGNATURE-half CDC (`myelin-agent/tests/cdc_8_3_agent_runtime.rs`).
//! THIS pair pins the `--use-mock` runtime half AG-P5 owns: the PROVIDER is the deterministic
//! scripted `MockAgentRuntime`; the CONSUMER is the platform loop, which builds the `Conversation`
//! and reads the `StepOutcome` stream. It is distinct from (and extends) the AG-P1 signature CDC —
//! no duplication.
//!
//! It also carries the **AG-D9 step-determinism GOLDEN artifact**: the same script replays to a
//! byte-identical `ReplayRecord` across two runs (the step-sequence half AG-D9 asserts; the
//! proposed-effect-sequence determinism completes in AG-P8 once apply produces effects).

use myelin_agent::{
    AgentRuntime, BudgetView, Conversation, StepOutcome, Submission, SystemContext, ToolCall,
    ToolName, ToolResult, ToolSchema, Turn,
};
use myelin_agent_service::{
    build_conversation, model_turns_taken, replay, select_runtime, MockAgentRuntime, MockScript,
    RuntimeFlag, TraceHistory,
};

/// A canonical multi-turn fixture: search → read → submit (two tool turns, then the terminal answer).
fn script() -> MockScript {
    MockScript::new(
        SystemContext("you are agent-7; you are labelled as an agent".into()),
        vec![ToolSchema("search".into()), ToolSchema("read".into())],
        BudgetView(100),
        vec![
            StepOutcome::UseTools(vec![ToolCall(ToolName("search".into()))]),
            StepOutcome::UseTools(vec![ToolCall(ToolName("read".into()))]),
            StepOutcome::Submit(Submission("the answer".into())),
        ],
    )
}

/// **PROVIDER + CONSUMER of 8.3 (the `--use-mock` half).** The PROVIDER is the scripted
/// `MockAgentRuntime` — `step` is a pure function of the conversation (stateless; the platform owns
/// history). The CONSUMER is the platform loop: it builds the `Conversation` from the running
/// history and reads the `StepOutcome` (UseTools → route + append; Submit → terminate).
#[test]
fn cdc_8_3_mock_runtime_is_a_pure_function_of_the_conversation() {
    let provider = MockAgentRuntime::new(script());

    // CONSUMER (the loop): an opening conversation → the FIRST scripted decision (search).
    let opening = Conversation::default();
    match provider.step(&opening) {
        StepOutcome::UseTools(calls) => {
            assert_eq!(calls, vec![ToolCall(ToolName("search".into()))]);
        }
        other => panic!("expected UseTools (search) on the opening turn, got {other:?}"),
    }

    // CONSUMER (the loop): after the search step was appended (one model turn) → step[1] (read).
    let mut conv = Conversation::default();
    conv.turns
        .push(Turn::Model(StepOutcome::UseTools(vec![ToolCall(
            ToolName("search".into()),
        )])));
    conv.turns.push(Turn::ToolResults(vec![ToolResult(
        "tool:search:result".into(),
    )]));
    assert_eq!(
        model_turns_taken(&conv),
        1,
        "one model turn taken (the tool-result turn does not count)"
    );
    assert_eq!(
        provider.step(&conv),
        StepOutcome::UseTools(vec![ToolCall(ToolName("read".into()))]),
        "after one model turn the brain replays step[1] (read)"
    );

    // CONSUMER (the loop): after the read step → step[2] (the terminal Submit).
    conv.turns
        .push(Turn::Model(StepOutcome::UseTools(vec![ToolCall(
            ToolName("read".into()),
        )])));
    conv.turns.push(Turn::ToolResults(vec![ToolResult(
        "tool:read:result".into(),
    )]));
    assert!(
        matches!(provider.step(&conv), StepOutcome::Submit(_)),
        "after two model turns the brain submits (it only ever PROPOSES — plan-then-apply survives)"
    );
}

/// **AG-D9 (the step-determinism leg) — the GOLDEN artifact: the same script replays to a
/// byte-identical `ReplayRecord` across two runs.** The recorded `StepOutcome` stream + the
/// reconstructed `Conversation`s are identical run-to-run (the AG-4 lever). This is the green
/// artifact the drill scorecard reads.
#[test]
fn ag_d9_golden_replay_is_byte_identical_across_runs() {
    let s = script();
    let first = replay(&s);
    let second = replay(&s);
    assert_eq!(
        first, second,
        "AG-D9: two replays of the same script are byte-identical"
    );

    // the golden StepOutcome stream IS the script, in order, terminated by the Submit.
    assert_eq!(
        first.outcomes,
        vec![
            StepOutcome::UseTools(vec![ToolCall(ToolName("search".into()))]),
            StepOutcome::UseTools(vec![ToolCall(ToolName("read".into()))]),
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

/// **8.3 — the real `--use-mock` flag drives the mock brain through the SAME `&dyn AgentRuntime`
/// seam (NOT a test-only stub).** `select_runtime(UseMock, script)` returns the mock behind the
/// frozen seam; the platform loop drives it exactly as it drives the SKELETON brain.
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

/// **`build_conversation` reconstructs the conversation from the platform-owned history (§2.1) —
/// deterministic.** The brain is stateless; the platform rebuilds the `Conversation` (system +
/// transcript + tools + budget) from the trace history. Same `(script, history)` → identical rebuild.
#[test]
fn cdc_8_3_build_conversation_reconstructs_from_platform_history() {
    let s = script();
    let mut history = TraceHistory::new();
    history.push_model(StepOutcome::UseTools(vec![ToolCall(ToolName(
        "search".into(),
    ))]));
    history.push_tool_results(vec![ToolResult("tool:search:result".into())]);

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
