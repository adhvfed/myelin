//! # The CDC pair for contracts 8.3 + 3.5 — chat agent presence + streaming partials
//! (CHAT-P24 / P-418, M4-C9)
//!
//! **Contracts:**
//! - `contract-index.md` row **8.3** (`AgentRuntime::step --use-mock` — the stateless-brain strategy
//!   seam; a deterministic scripted brain on the `--use-mock` code path, NO LLM SDK).
//! - row **3.5** (the firehose transport + the resume-cursor protocol — scope is a BOUNDED selector,
//!   never `*`: board:/doc:/channel:).
//!
//! **Owning architecture:** chat `02-internals-and-algorithms.md` §7.2 (agent presence is its own
//! fabric-health-derived class) + §7.3 (streaming partials on the firehose; the FINAL durable
//! `chat.message.created` REPLACES the partial; a reconnect resumes the FINAL, never a half-message);
//! `03-events-contracts-and-glue.md` §1.2 (`agent.message.partial` is firehose-only).
//!
//! ## The two seams this pair pins
//!
//! **8.3** — chat is the CONSUMER of the `AgentRuntime::step` strategy seam: it drives a
//! `--use-mock` runtime ([`myelin_chat::MockStreamRuntime`]) that streams scripted partials then
//! SUBMITS. The PROVIDER (the frozen [`myelin_agent::AgentRuntime`] trait) admits chat's mock impl on
//! the SAME `step` code path the real `LlmAgentRuntime` will use — proven WITHOUT an LLM.
//!
//! **3.5** — chat is the PROVIDER of the presence/partial frame SHAPES; it declares its scopes as
//! bounded `channel:<id>` selectors. The CONSUMER (the Bus's `FirehoseScope` `*`-rejecting
//! chokepoint) ADMITS them as bounded selectors. Chat authors no second scope validator.

use myelin_agent::{AgentRuntime, Conversation, StepOutcome, Submission};
use myelin_chat::glue::chat_channel_scope;
use myelin_chat::presence::{
    run_streamed, AgentPresence, FabricHealth, MockStreamRuntime, PartialFrame, PartialPush,
    StreamState,
};
use myelin_events::{FirehoseScope, ScopeKind};

// ─────────────────────────── 8.3 — the --use-mock streaming runtime (CONSUMER) ─────────────────────

/// **8.3 PROVIDER ⇄ CONSUMER** — chat's `--use-mock` streaming runtime is a real
/// [`myelin_agent::AgentRuntime`]: `step` is a pure function of the conversation and SUBMITS the
/// scripted answer, with NO LLM SDK. The scripted answer is also the partial stream's terminal
/// cumulative text — so the FINAL submission body matches the last partial exactly (final replaces
/// partial). This is the seam the real `LlmAgentRuntime` swaps into post-M5.
#[test]
fn cdc_8_3_mock_runtime_streams_partials_then_submits_the_same_answer() {
    let runtime = MockStreamRuntime::new("run-1", "hello brave world");

    // the scripted partial stream (the firehose frames) — cumulative, monotonic, last-marked.
    let partials = runtime.partials();
    assert_eq!(partials.len(), 3, "one partial per scripted token");
    assert_eq!(
        partials.last().unwrap().cumulative_text,
        "hello brave world"
    );
    assert!(partials.last().unwrap().is_last);

    // the brain SUBMITS the same answer through the frozen `step` seam (the CONSUMER drives it).
    let outcome = runtime.step(&Conversation::default());
    match outcome {
        StepOutcome::Submit(Submission(body)) => {
            // the FINAL durable body == the last partial cumulative — the reconciliation is exact.
            assert_eq!(body, partials.last().unwrap().cumulative_text);
        }
        StepOutcome::UseTools(_) => panic!("the streaming mock must submit"),
    }
}

/// **8.3 — `step` is deterministic (the golden/mutation-testable property).** The same conversation
/// yields the same submission across calls (a pure function; no hidden state, no LLM).
#[test]
fn cdc_8_3_step_is_deterministic_no_llm() {
    let runtime = MockStreamRuntime::new("run-2", "deterministic answer");
    let a = runtime.step(&Conversation::default());
    let b = runtime.step(&Conversation::default());
    assert_eq!(a, b, "step must be a pure function of the conversation");
}

// ─────────────────────────── 3.5 — the firehose scope for presence + partials ──────────────────────

/// **3.5 PROVIDER (chat) ⇄ CONSUMER (Bus chokepoint)** — chat's presence + partial frames ride a
/// BOUNDED `channel:<id>` scope, admitted by the Bus's `*`-rejecting chokepoint as a `Channel`
/// selector. An unbounded `*` scope is REJECTED (chat never publishes on the tenant firehose).
#[test]
fn cdc_3_5_presence_and_partials_ride_a_bounded_channel_scope() {
    let scope = chat_channel_scope("eng").expect("chat's presence/partial scope is bounded");
    assert_eq!(scope.kind(), ScopeKind::Channel);

    // the Bus chokepoint REJECTS an unbounded scope — chat cannot publish presence on `*`.
    assert!(FirehoseScope::parse("*").is_err());
    assert!(FirehoseScope::parse("channel:*").is_err());
}

/// **3.5 — the partial push port publishes each frame on the bounded scope, in order.** A capturing
/// [`PartialPush`] proves chat hands the frames to the gateway's transport (it owns no socket) on a
/// bounded selector, with a monotonic resume cursor.
#[test]
fn cdc_3_5_partial_push_publishes_on_the_bounded_scope_with_a_monotonic_cursor() {
    #[derive(Default)]
    struct CaptureSeqs {
        seqs: std::cell::RefCell<Vec<u64>>,
    }
    impl PartialPush for CaptureSeqs {
        fn push_partial(&self, scope: &FirehoseScope, _frame: &PartialFrame) -> u64 {
            // the frame is published on a BOUNDED channel scope (never `*`).
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

    // the run finalized (the FINAL replaced the partial).
    assert!(matches!(session.state, StreamState::Finalized { .. }));
    // every partial was published on the bounded scope with a monotonic cursor.
    assert_eq!(*push.seqs.borrow(), vec![1, 2, 3]);
}

// ─────────────────────────── presence transitions over the consumed health signal ─────────────────

/// **8.3/3.5 — presence is derived from the consumed `agent.status_changed` health verdict.** A
/// healthy idle agent is `available`; starting a run takes it `busy`; a shed verdict mid-run
/// overrides to `rate-limited`; the run finishing on a healthy fabric returns to `available`.
#[test]
fn cdc_presence_class_tracks_fabric_health_and_run_lifecycle() {
    // idle + healthy ⇒ available.
    let p = FabricHealth::Healthy.idle_presence();
    assert_eq!(p, AgentPresence::Available);
    // dispatch ⇒ busy.
    let p = p.on_run_start();
    assert_eq!(p, AgentPresence::Busy);
    // shed mid-run ⇒ rate-limited (the protected-human-lane shed, OQ-K).
    let shed = p.on_status(FabricHealth::Shed);
    assert_eq!(shed, AgentPresence::RateLimited);
    // a run that finishes on a healthy fabric returns to available.
    let done = AgentPresence::Busy.on_run_finish(FabricHealth::Healthy);
    assert_eq!(done, AgentPresence::Available);
}
