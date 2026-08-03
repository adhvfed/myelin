//! # The CDC pair for contract 8.3 — `AgentRuntime::step(&Conversation) -> StepOutcome`
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.3
//! (`AgentRuntime::step(&Conversation) → UseTools | Submit` — the stateless brain; strategy seam
//! (skeleton/mock/llm); platform owns history; `--use-mock` is a real runtime flag). Owning
//! architecture: `agent-fabric.md` §2.1. AG-P1 / P-130 ships the SIGNATURE half; the runtimes are
//! SKELETON (AG-P4 → P-216) and `MockAgentRuntime` (AG-P5 → P-217); `LlmAgentRuntime` is
//! designed-not-built (AG-P25) — the only place an LLM SDK/prompt/model-name may ever appear.
//!
//! ## What this pair pins (the signature half of 8.3)
//! - the **PROVIDER** is a runtime (here a deterministic scripted brain on the `--use-mock` code
//!   path): `step` is a pure function of the whole `Conversation`, returning a single decision; the
//!   platform owns history (the runtime is stateless). NO LLM SDK appears (no-llm-in-platform, 1.6).
//! - the **CONSUMER** is the platform loop (`Agent::handle`): it builds the `Conversation` and reads
//!   the `StepOutcome` — `UseTools` (call these tools, step me again) or `Submit` (final answer).

use myelin_agent::{
    AgentRuntime, Conversation, StepOutcome, Submission, ToolCall, ToolCallId, ToolName,
};

/// A tool call with a deterministic id and null arguments (this scripted brain chooses no real
/// arguments); the id links its later result back at the widened seam.
fn call(name: &str) -> ToolCall {
    ToolCall {
        id: ToolCallId(format!("call:{name}")),
        name: ToolName(name.into()),
        arguments: serde_json::Value::Null,
    }
}

/// **PROVIDER side of 8.3 (a runtime).** A deterministic scripted brain — the `--use-mock` shape:
/// `step` is a pure function of the conversation; the platform owns history, the runtime is
/// stateless. It submits on an empty conversation, asks for a tool otherwise. NO model SDK.
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

    // CONSUMER (the loop): an opening conversation → the brain proposes a tool call.
    let opening = Conversation::default();
    match provider.step(&opening) {
        StepOutcome::UseTools(calls) => {
            assert_eq!(calls, vec![call("search")]);
        }
        other => panic!("expected UseTools on the opening turn, got {other:?}"),
    }

    // CONSUMER (the loop): after a turn was appended → the brain submits (it only ever PROPOSES).
    let mut later = Conversation::default();
    later
        .turns
        .push(myelin_agent::Turn::Model(StepOutcome::Submit(Submission(
            "x".into(),
        ))));
    assert!(matches!(provider.step(&later), StepOutcome::Submit(_)));
}
