use myelin_agent::{
    AgentRuntime, BudgetView, Conversation, MeteredRuntime, StepOutcome, Submission, SystemContext,
    ToolCall, ToolOutcome, ToolResult, ToolSchema, Turn,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockScript {
    system: SystemContext,
    tools: Vec<ToolSchema>,
    budget: BudgetView,
    steps: Vec<StepOutcome>,
}

impl MockScript {
    pub fn new(
        system: SystemContext,
        tools: Vec<ToolSchema>,
        budget: BudgetView,
        steps: Vec<StepOutcome>,
    ) -> MockScript {
        MockScript {
            system,
            tools,
            budget,
            steps,
        }
    }

    pub fn submit_only(system: impl Into<String>, answer: impl Into<String>) -> MockScript {
        MockScript {
            system: SystemContext(system.into()),
            tools: Vec::new(),
            budget: BudgetView(0),
            steps: vec![StepOutcome::Submit(Submission(answer.into()))],
        }
    }

    fn step_at(&self, n: usize) -> StepOutcome {
        match self.steps.get(n) {
            Some(step) => step.clone(),
            None => StepOutcome::Submit(Submission(
                "mock: script exhausted - defensive terminal submit (script not well-formed)"
                    .into(),
            )),
        }
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn is_well_formed(&self) -> bool {
        matches!(self.steps.last(), Some(StepOutcome::Submit(_)))
    }

    pub fn system(&self) -> &SystemContext {
        &self.system
    }

    pub fn tools(&self) -> &[ToolSchema] {
        &self.tools
    }

    pub fn budget(&self) -> &BudgetView {
        &self.budget
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockAgentRuntime {
    script: MockScript,
}

impl MockAgentRuntime {
    pub fn new(script: MockScript) -> MockAgentRuntime {
        MockAgentRuntime { script }
    }

    pub fn script(&self) -> &MockScript {
        &self.script
    }
}

pub fn model_turns_taken(conv: &Conversation) -> usize {
    conv.turns
        .iter()
        .filter(|t| matches!(t, Turn::Model(_)))
        .count()
}

impl AgentRuntime for MockAgentRuntime {
    fn step(&self, conv: &Conversation) -> StepOutcome {
        let n = model_turns_taken(conv);
        self.script.step_at(n)
    }
}

impl MeteredRuntime for MockAgentRuntime {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceHistory {
    entries: Vec<HistoryEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryEntry {
    Model(StepOutcome),
    ToolResults(Vec<ToolOutcome>),
}

impl TraceHistory {
    pub fn new() -> TraceHistory {
        TraceHistory::default()
    }

    pub fn push_model(&mut self, step: StepOutcome) {
        self.entries.push(HistoryEntry::Model(step));
    }

    pub fn push_tool_results(&mut self, results: Vec<ToolOutcome>) {
        self.entries.push(HistoryEntry::ToolResults(results));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub fn build_conversation(script: &MockScript, history: &TraceHistory) -> Conversation {
    let turns = history
        .entries
        .iter()
        .map(|e| match e {
            HistoryEntry::Model(step) => Turn::Model(step.clone()),
            HistoryEntry::ToolResults(results) => Turn::ToolResults(results.clone()),
        })
        .collect();
    Conversation {
        system: script.system.clone(),
        turns,
        tools: script.tools.clone(),
        budget: script.budget.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayRecord {
    pub outcomes: Vec<StepOutcome>,
    pub conversations: Vec<Conversation>,
    pub submission: Option<Submission>,
    pub terminated: bool,
}

pub const MOCK_MAX_STEPS: usize = 64;

pub fn replay(script: &MockScript) -> ReplayRecord {
    let brain = MockAgentRuntime::new(script.clone());
    replay_bounded(&brain, script, MOCK_MAX_STEPS)
}

pub fn replay_bounded(
    brain: &dyn AgentRuntime,
    framing: &MockScript,
    max_steps: usize,
) -> ReplayRecord {
    let script = framing;
    let mut history = TraceHistory::new();
    let mut outcomes = Vec::new();
    let mut conversations = Vec::new();
    let mut submission = None;
    let mut terminated = false;

    for _ in 0..max_steps {
        let conv = build_conversation(script, &history);
        let outcome = brain.step(&conv);
        outcomes.push(outcome.clone());
        conversations.push(conv);
        match &outcome {
            StepOutcome::Submit(s) => {
                history.push_model(outcome.clone());
                submission = Some(s.clone());
                terminated = true;
                break;
            }
            StepOutcome::UseTools(calls) => {
                history.push_model(outcome.clone());
                let results = scripted_tool_results(calls);
                history.push_tool_results(results);
            }
        }
    }

    ReplayRecord {
        outcomes,
        conversations,
        submission,
        terminated,
    }
}

fn scripted_tool_results(calls: &[ToolCall]) -> Vec<ToolOutcome> {
    calls
        .iter()
        .map(|call| ToolOutcome {
            call_id: call.id.clone(),
            result: ToolResult::Succeeded(format!("tool:{}:result", call.name.0)),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFlag {
    Skeleton,
    UseMock,
}

impl RuntimeFlag {
    pub fn from_args<I, S>(args: I) -> RuntimeFlag
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if args.into_iter().any(|a| a.as_ref() == "--use-mock") {
            RuntimeFlag::UseMock
        } else {
            RuntimeFlag::Skeleton
        }
    }

    pub fn is_mock(self) -> bool {
        matches!(self, RuntimeFlag::UseMock)
    }
}

pub fn select_runtime(
    flag: RuntimeFlag,
    script: MockScript,
) -> Box<dyn AgentRuntime + Send + Sync> {
    match flag {
        RuntimeFlag::UseMock => Box::new(MockAgentRuntime::new(script)),
        RuntimeFlag::Skeleton => Box::new(crate::skeleton::SkeletonAgentRuntime::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{Submission, ToolCallId, ToolName};

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

    fn search_then_read_then_submit() -> MockScript {
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
    fn mock_step_is_stateless_pure_function_of_the_conversation() {
        let brain = MockAgentRuntime::new(search_then_read_then_submit());

        let opening = Conversation::default();
        let a = brain.step(&opening);
        let b = brain.step(&opening);
        assert_eq!(
            a, b,
            "the brain holds no cursor - same conversation, same outcome"
        );
        assert_eq!(
            a,
            StepOutcome::UseTools(vec![call("search")]),
            "the opening turn replays step[0] (search)"
        );

        let mut later = Conversation::default();
        later.turns.push(Turn::Model(a.clone()));
        assert_eq!(
            brain.step(&later),
            StepOutcome::UseTools(vec![call("read")]),
            "one model turn taken → replay step[1] (read)"
        );
    }

    #[test]
    fn model_turns_taken_counts_only_model_turns() {
        let mut conv = Conversation::default();
        assert_eq!(
            model_turns_taken(&conv),
            0,
            "an opening conversation is at position 0"
        );
        conv.turns
            .push(Turn::Model(StepOutcome::Submit(Submission("a".into()))));
        assert_eq!(model_turns_taken(&conv), 1, "one model turn → position 1");
        conv.turns.push(Turn::ToolResults(vec![ToolOutcome {
            call_id: ToolCallId("call:r".into()),
            result: ToolResult::Succeeded("r".into()),
        }]));
        assert_eq!(
            model_turns_taken(&conv),
            1,
            "a tool-result turn does NOT advance the position"
        );
        conv.turns
            .push(Turn::Model(StepOutcome::Submit(Submission("b".into()))));
        assert_eq!(
            model_turns_taken(&conv),
            2,
            "a second model turn → position 2"
        );
    }

    #[test]
    fn ag_d9_replay_is_byte_identical_across_two_runs() {
        let script = search_then_read_then_submit();
        let first = replay(&script);
        let second = replay(&script);
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
            "the replayed StepOutcome stream IS the script, in order"
        );
        assert!(
            first.terminated,
            "a well-formed script terminates the bounded loop"
        );
        assert_eq!(
            first.submission,
            Some(Submission("the answer".into())),
            "the terminal submission is the script's final answer"
        );
    }

    #[test]
    fn ag_d9_conversation_reconstruction_grows_the_transcript_deterministically() {
        let script = search_then_read_then_submit();
        let rec = replay(&script);

        assert_eq!(
            rec.conversations.len(),
            3,
            "three turns → three reconstructed conversations"
        );

        let c0 = &rec.conversations[0];
        assert!(c0.turns.is_empty(), "turn 0 opens with an empty transcript");
        assert_eq!(
            c0.system,
            SystemContext("you are agent-7; you are labelled as an agent".into())
        );
        assert_eq!(
            c0.tools.len(),
            2,
            "the scoped tool list is rebuilt from the script"
        );
        assert_eq!(
            c0.budget,
            BudgetView(100),
            "the budget view is rebuilt from the script"
        );

        let c1 = &rec.conversations[1];
        assert_eq!(
            c1.turns.len(),
            2,
            "turn 1 sees the search step + its routed tool result"
        );
        assert_eq!(
            c1.turns[0],
            Turn::Model(StepOutcome::UseTools(vec![call("search")]))
        );
        assert_eq!(
            c1.turns[1],
            Turn::ToolResults(vec![outcome("search")]),
            "the routed tool result is DETERMINISTIC (tool:<name>:result), linked to its call id"
        );

        let c2 = &rec.conversations[2];
        assert_eq!(c2.turns.len(), 4, "turn 2 sees both prior tool round-trips");
    }

    #[test]
    fn non_terminating_brain_trips_the_bounded_ceiling() {
        struct NeverSubmits;
        impl AgentRuntime for NeverSubmits {
            fn step(&self, _conv: &Conversation) -> StepOutcome {
                StepOutcome::UseTools(vec![call("loop")])
            }
        }
        let framing = MockScript::new(SystemContext("sys".into()), vec![], BudgetView(0), vec![]);
        let rec = replay_bounded(&NeverSubmits, &framing, 5);
        assert!(
            !rec.terminated,
            "a non-terminating brain trips the ceiling (never hangs)"
        );
        assert_eq!(
            rec.outcomes.len(),
            5,
            "it ran exactly max_steps turns, then stopped (bounded)"
        );
        assert_eq!(
            rec.submission, None,
            "no terminal submission (it never submitted)"
        );

        let good = search_then_read_then_submit();
        assert!(
            good.is_well_formed(),
            "search→read→submit ends in a terminal Submit"
        );
        let good_brain = MockAgentRuntime::new(good.clone());
        let good_rec = replay_bounded(&good_brain, &good, 64);
        assert!(
            good_rec.terminated,
            "a well-formed scripted brain terminates the bounded loop"
        );
    }

    #[test]
    fn script_exhaustion_is_a_defensive_terminal_submit() {
        let script = MockScript::submit_only("sys", "done");
        assert!(matches!(
            script.step_at(0),
            StepOutcome::Submit(Submission(ref s)) if s == "done"
        ));
        match script.step_at(1) {
            StepOutcome::Submit(Submission(s)) => {
                assert!(
                    s.contains("script exhausted"),
                    "past-the-end is a LOUD defensive submit: {s}"
                );
            }
            other => panic!("expected a defensive terminal submit, got {other:?}"),
        }
        assert!(
            script.is_well_formed(),
            "submit_only IS well-formed (a single terminal Submit)"
        );
    }

    #[test]
    fn use_mock_flag_selects_the_mock_brain_on_the_same_seam() {
        let flag = RuntimeFlag::from_args(["myelin-agent", "serve"]);
        assert_eq!(
            flag,
            RuntimeFlag::Skeleton,
            "no --use-mock → the SKELETON brain (default)"
        );
        assert!(!flag.is_mock());

        let flag = RuntimeFlag::from_args(["myelin-agent", "serve", "--use-mock"]);
        assert_eq!(
            flag,
            RuntimeFlag::UseMock,
            "--use-mock → the deterministic mock brain"
        );
        assert!(flag.is_mock());

        let brain = select_runtime(RuntimeFlag::UseMock, MockScript::submit_only("sys", "ok"));
        assert_eq!(
            brain.step(&Conversation::default()),
            StepOutcome::Submit(Submission("ok".into())),
            "the selected mock brain replays its script through the &dyn seam"
        );

        let skel = select_runtime(
            RuntimeFlag::Skeleton,
            MockScript::submit_only("sys", "ignored"),
        );
        assert!(
            matches!(skel.step(&Conversation::default()), StepOutcome::Submit(_)),
            "the SKELETON arm submits immediately (no model, no tools)"
        );
    }

    #[test]
    fn mock_brain_drives_the_skeleton_loop_seam_unchanged() {
        use crate::skeleton::SkeletonAgent;
        use myelin_agent::{Agent, InboxEvent};
        let loop_body = SkeletonAgent::new();
        let brain = MockAgentRuntime::new(MockScript::submit_only("sys", "answer"));
        let out = loop_body.handle(InboxEvent("issue.created".into()), &brain);
        assert!(
            out.0.contains("skeleton handle"),
            "the mock brain drives the SAME platform-owned loop seam (only the brain swapped): {out:?}"
        );
    }

    #[test]
    fn build_conversation_is_deterministic() {
        let script = search_then_read_then_submit();
        let mut history = TraceHistory::new();
        history.push_model(StepOutcome::UseTools(vec![call("search")]));
        history.push_tool_results(vec![outcome("search")]);

        let a = build_conversation(&script, &history);
        let b = build_conversation(&script, &history);
        assert_eq!(
            a, b,
            "the same (script, history) rebuilds a byte-identical conversation"
        );
        assert_eq!(
            a.turns.len(),
            2,
            "the transcript carries the model step + the tool result"
        );
        assert_eq!(
            a.system,
            *script.system(),
            "the system framing is rebuilt from the script"
        );
        assert_eq!(
            a.tools,
            script.tools(),
            "the scoped tool list is rebuilt from the script"
        );
    }

    #[test]
    fn is_well_formed_predicate_is_exact() {
        assert!(
            !MockScript::new(SystemContext("s".into()), vec![], BudgetView(0), vec![])
                .is_well_formed(),
            "an empty script is NOT well-formed (it never terminates)"
        );
        assert!(
            MockScript::submit_only("s", "x").is_well_formed(),
            "a single Submit IS well-formed"
        );
        let trailing_tools = MockScript::new(
            SystemContext("s".into()),
            vec![],
            BudgetView(0),
            vec![
                StepOutcome::Submit(Submission("early".into())),
                StepOutcome::UseTools(vec![call("t")]),
            ],
        );
        assert!(
            !trailing_tools.is_well_formed(),
            "a script ending in UseTools is NOT well-formed"
        );
    }

    #[test]
    fn script_and_history_accessors_are_exact() {
        let empty = MockScript::new(SystemContext("s".into()), vec![], BudgetView(0), vec![]);
        assert_eq!(empty.len(), 0, "an empty script has len 0 (kills -> 1)");
        assert!(
            empty.is_empty(),
            "an empty script is_empty (kills -> false)"
        );
        let three = search_then_read_then_submit();
        assert_eq!(
            three.len(),
            3,
            "a three-step script has len 3 (kills -> 0 / -> 1)"
        );
        assert!(
            !three.is_empty(),
            "a non-empty script is NOT is_empty (kills -> true)"
        );

        assert_eq!(
            *three.budget(),
            BudgetView(100),
            "budget() returns its field (kills -> default)"
        );
        assert_ne!(
            *three.budget(),
            BudgetView::default(),
            "the budget is non-default (kills -> default)"
        );

        let mut h = TraceHistory::new();
        assert_eq!(h.len(), 0, "a fresh history has len 0 (kills -> 1)");
        assert!(h.is_empty(), "a fresh history is_empty (kills -> false)");
        h.push_model(StepOutcome::Submit(Submission("a".into())));
        h.push_tool_results(vec![outcome("r")]);
        assert_eq!(h.len(), 2, "two entries → len 2 (kills -> 0 / -> 1)");
        assert!(
            !h.is_empty(),
            "a non-empty history is NOT is_empty (kills -> true)"
        );
    }
}
