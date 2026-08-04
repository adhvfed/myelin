use myelin_agent::{AgentRuntime, Conversation, StepOutcome, Submission};
use myelin_chat::glue::chat_channel_scope;
use myelin_chat::presence::{
    run_streamed, AgentPresence, FabricHealth, MockStreamRuntime, PartialFrame, PartialPush,
    StreamState,
};
use myelin_events::{FirehoseScope, ScopeKind};

#[test]
fn cdc_8_3_mock_runtime_streams_partials_then_submits_the_same_answer() {
    let runtime = MockStreamRuntime::new("run-1", "hello brave world");

    let partials = runtime.partials();
    assert_eq!(partials.len(), 3, "one partial per scripted token");
    assert_eq!(
        partials.last().unwrap().cumulative_text,
        "hello brave world"
    );
    assert!(partials.last().unwrap().is_last);

    let outcome = runtime.step(&Conversation::default());
    match outcome {
        StepOutcome::Submit(Submission(body)) => {
            assert_eq!(body, partials.last().unwrap().cumulative_text);
        }
        StepOutcome::UseTools(_) => panic!("the streaming mock must submit"),
    }
}

#[test]
fn cdc_8_3_step_is_deterministic_no_llm() {
    let runtime = MockStreamRuntime::new("run-2", "deterministic answer");
    let a = runtime.step(&Conversation::default());
    let b = runtime.step(&Conversation::default());
    assert_eq!(a, b, "step must be a pure function of the conversation");
}

#[test]
fn cdc_3_5_presence_and_partials_ride_a_bounded_channel_scope() {
    let scope = chat_channel_scope("eng").expect("chat's presence/partial scope is bounded");
    assert_eq!(scope.kind(), ScopeKind::Channel);

    assert!(FirehoseScope::parse("*").is_err());
    assert!(FirehoseScope::parse("channel:*").is_err());
}

#[test]
fn cdc_3_5_partial_push_publishes_on_the_bounded_scope_with_a_monotonic_cursor() {
    #[derive(Default)]
    struct CaptureSeqs {
        seqs: std::cell::RefCell<Vec<u64>>,
    }
    impl PartialPush for CaptureSeqs {
        fn push_partial(&self, scope: &FirehoseScope, _frame: &PartialFrame) -> u64 {
            assert_eq!(scope.kind(), ScopeKind::Channel);
            let mut s = self.seqs.borrow_mut();
            let next = s.len() as u64 + 1;
            s.push(next);
            next
        }
    }

    let runtime = MockStreamRuntime::new("run-3", "alpha beta gamma");
    let push = CaptureSeqs::default();
    let scope = chat_channel_scope("c-1").expect("bounded");
    let session = run_streamed(&runtime, "msg-3", Some((&push, &scope)));

    assert!(matches!(session.state, StreamState::Finalized { .. }));
    assert_eq!(*push.seqs.borrow(), vec![1, 2, 3]);
}

#[test]
fn cdc_presence_class_tracks_fabric_health_and_run_lifecycle() {
    let p = FabricHealth::Healthy.idle_presence();
    assert_eq!(p, AgentPresence::Available);
    let p = p.on_run_start();
    assert_eq!(p, AgentPresence::Busy);
    let shed = p.on_status(FabricHealth::Shed);
    assert_eq!(shed, AgentPresence::RateLimited);
    let done = AgentPresence::Busy.on_run_finish(FabricHealth::Healthy);
    assert_eq!(done, AgentPresence::Available);
}
