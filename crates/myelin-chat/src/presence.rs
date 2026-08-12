use std::collections::BTreeMap;

use myelin_events::firehose::FirehoseScope;

use crate::events::{delivery_class, DeliveryClass, CHAT_PRESENCE_CHANGED};

pub const AGENT_MESSAGE_PARTIAL: &str = "agent.message.partial";

pub const AGENT_STATUS_CHANGED: &str = "agent.status_changed";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentPresence {
    Available,
    Busy,
    RateLimited,
    Offline,
}

impl AgentPresence {
    pub fn key(self) -> &'static str {
        match self {
            AgentPresence::Available => "available",
            AgentPresence::Busy => "busy",
            AgentPresence::RateLimited => "rate-limited",
            AgentPresence::Offline => "offline",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            AgentPresence::Available => "●",
            AgentPresence::Busy => "◐",
            AgentPresence::RateLimited => "⏸",
            AgentPresence::Offline => "○",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AgentPresence::Available => "Available",
            AgentPresence::Busy => "Working…",
            AgentPresence::RateLimited => "Rate-limited",
            AgentPresence::Offline => "Offline",
        }
    }

    pub fn dispatchable(self) -> bool {
        matches!(self, AgentPresence::Available)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FabricHealth {
    Healthy,
    Shed,
    Down,
}

impl FabricHealth {
    pub fn idle_presence(self) -> AgentPresence {
        match self {
            FabricHealth::Healthy => AgentPresence::Available,
            FabricHealth::Shed => AgentPresence::RateLimited,
            FabricHealth::Down => AgentPresence::Offline,
        }
    }
}

impl AgentPresence {
    pub fn on_status(self, health: FabricHealth) -> AgentPresence {
        match health {
            FabricHealth::Shed => AgentPresence::RateLimited,
            FabricHealth::Down => AgentPresence::Offline,
            FabricHealth::Healthy => {
                if self == AgentPresence::Busy {
                    AgentPresence::Busy
                } else {
                    AgentPresence::Available
                }
            }
        }
    }

    pub fn on_run_start(self) -> AgentPresence {
        match self {
            AgentPresence::Available => AgentPresence::Busy,
            other => other,
        }
    }

    pub fn on_run_finish(self, still_healthy: FabricHealth) -> AgentPresence {
        if self == AgentPresence::Busy {
            still_healthy.idle_presence()
        } else {
            self
        }
    }
}

pub trait PresencePush {
    fn push_presence(&self, scope: &FirehoseScope, agent: &str, class: AgentPresence) -> u64;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialFrame {
    pub correlation_id: String,
    pub seq: u64,
    pub cumulative_text: String,
    pub is_last: bool,
}

pub trait PartialPush {
    fn push_partial(&self, scope: &FirehoseScope, frame: &PartialFrame) -> u64;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamState {
    Streaming {
        last_seq: u64,
        cumulative_text: String,
    },
    Finalized {
        message_id: String,
        final_text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamSession {
    pub correlation_id: String,
    pub state: StreamState,
}

impl StreamSession {
    pub fn open(correlation_id: impl Into<String>) -> StreamSession {
        StreamSession {
            correlation_id: correlation_id.into(),
            state: StreamState::Streaming {
                last_seq: 0,
                cumulative_text: String::new(),
            },
        }
    }

    pub fn apply_partial(&mut self, frame: &PartialFrame) -> bool {
        match &mut self.state {
            StreamState::Streaming {
                last_seq,
                cumulative_text,
            } => {
                if frame.seq != *last_seq + 1 {
                    return false;
                }
                *last_seq = frame.seq;
                cumulative_text.clone_from(&frame.cumulative_text);
                true
            }
            StreamState::Finalized { .. } => false,
        }
    }

    pub fn finalize(&mut self, message_id: impl Into<String>, final_text: impl Into<String>) {
        if let StreamState::Streaming { .. } = self.state {
            self.state = StreamState::Finalized {
                message_id: message_id.into(),
                final_text: final_text.into(),
            };
        }
    }

    pub fn is_finalized(&self) -> bool {
        matches!(self.state, StreamState::Finalized { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeView {
    Final {
        message_id: String,
        final_text: String,
    },
    InProgress {
        resume_from_seq: u64,
    },
}

pub fn resume_view(session: &StreamSession) -> ResumeView {
    match &session.state {
        StreamState::Finalized {
            message_id,
            final_text,
        } => ResumeView::Final {
            message_id: message_id.clone(),
            final_text: final_text.clone(),
        },
        StreamState::Streaming { last_seq, .. } => ResumeView::InProgress {
            resume_from_seq: *last_seq,
        },
    }
}

#[derive(Clone, Debug, Default)]
pub struct StreamSessions {
    by_correlation: BTreeMap<String, StreamSession>,
}

impl StreamSessions {
    pub fn new() -> StreamSessions {
        StreamSessions {
            by_correlation: BTreeMap::new(),
        }
    }

    pub fn open(&mut self, correlation_id: impl Into<String>) -> &mut StreamSession {
        let id = correlation_id.into();
        self.by_correlation
            .entry(id.clone())
            .or_insert_with(|| StreamSession::open(id))
    }

    pub fn get(&self, correlation_id: &str) -> Option<&StreamSession> {
        self.by_correlation.get(correlation_id)
    }

    pub fn resume(&self, correlation_id: &str) -> Option<ResumeView> {
        self.by_correlation.get(correlation_id).map(resume_view)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockStreamRuntime {
    answer: String,
    correlation_id: String,
}

impl MockStreamRuntime {
    pub fn new(correlation_id: impl Into<String>, answer: impl Into<String>) -> MockStreamRuntime {
        MockStreamRuntime {
            answer: answer.into(),
            correlation_id: correlation_id.into(),
        }
    }

    pub fn answer(&self) -> &str {
        &self.answer
    }

    pub fn partials(&self) -> Vec<PartialFrame> {
        let tokens: Vec<&str> = self.answer.split_whitespace().collect();
        if tokens.is_empty() {
            return vec![PartialFrame {
                correlation_id: self.correlation_id.clone(),
                seq: 1,
                cumulative_text: String::new(),
                is_last: true,
            }];
        }
        let mut frames = Vec::with_capacity(tokens.len());
        let mut cumulative = String::new();
        let last = tokens.len() - 1;
        for (i, tok) in tokens.iter().enumerate() {
            if !cumulative.is_empty() {
                cumulative.push(' ');
            }
            cumulative.push_str(tok);
            frames.push(PartialFrame {
                correlation_id: self.correlation_id.clone(),
                seq: (i as u64) + 1,
                cumulative_text: cumulative.clone(),
                is_last: i == last,
            });
        }
        frames
    }

    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }
}

impl myelin_agent::AgentRuntime for MockStreamRuntime {
    fn step(&self, _conv: &myelin_agent::Conversation) -> myelin_agent::StepOutcome {
        myelin_agent::StepOutcome::Submit(myelin_agent::Submission(self.answer.clone()))
    }
}

pub fn run_streamed(
    runtime: &MockStreamRuntime,
    message_id: impl Into<String>,
    mut push: Option<(&dyn PartialPush, &FirehoseScope)>,
) -> StreamSession {
    use myelin_agent::AgentRuntime;

    let mut session = StreamSession::open(runtime.correlation_id());
    for frame in runtime.partials() {
        if let Some((port, scope)) = push.as_mut() {
            port.push_partial(scope, &frame);
        }
        let _ = session.apply_partial(&frame);
    }
    let outcome = runtime.step(&myelin_agent::Conversation::default());
    let final_text = match outcome {
        myelin_agent::StepOutcome::Submit(myelin_agent::Submission(s)) => s,
        myelin_agent::StepOutcome::UseTools(_) => runtime.answer().to_string(),
    };
    session.finalize(message_id, final_text);
    session
}

pub trait AgD4Attestation {
    fn artifact_tag(&self) -> &str;
    fn drill_id(&self) -> &str;
    fn total_escapes(&self) -> u32;
}

pub fn ag_d4_attestation_is_green<A: AgD4Attestation>(attestation: Option<&A>) -> bool {
    match attestation {
        None => false,
        Some(att) => {
            att.artifact_tag() == "ag-d4-green-escape-attestation"
                && att.drill_id() == "AG-D4 / CI-T1"
                && att.total_escapes() == 0
        }
    }
}

pub fn presence_and_partials_are_firehose_only() -> bool {
    let presence_firehose = delivery_class(CHAT_PRESENCE_CHANGED) == Some(DeliveryClass::Firehose);
    let partial_is_agent_owned = AGENT_MESSAGE_PARTIAL.starts_with("agent.")
        && delivery_class(AGENT_MESSAGE_PARTIAL).is_none();
    presence_firehose && partial_is_agent_owned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_maps_to_the_idle_presence_class() {
        assert_eq!(
            FabricHealth::Healthy.idle_presence(),
            AgentPresence::Available
        );
        assert_eq!(
            FabricHealth::Shed.idle_presence(),
            AgentPresence::RateLimited
        );
        assert_eq!(FabricHealth::Down.idle_presence(), AgentPresence::Offline);
    }

    #[test]
    fn run_start_takes_an_available_agent_to_busy() {
        assert_eq!(AgentPresence::Available.on_run_start(), AgentPresence::Busy);
        assert_eq!(
            AgentPresence::RateLimited.on_run_start(),
            AgentPresence::RateLimited
        );
        assert_eq!(
            AgentPresence::Offline.on_run_start(),
            AgentPresence::Offline
        );
        assert_eq!(AgentPresence::Busy.on_run_start(), AgentPresence::Busy);
    }

    #[test]
    fn run_finish_returns_busy_to_the_current_idle_class() {
        assert_eq!(
            AgentPresence::Busy.on_run_finish(FabricHealth::Healthy),
            AgentPresence::Available
        );
        assert_eq!(
            AgentPresence::Busy.on_run_finish(FabricHealth::Shed),
            AgentPresence::RateLimited
        );
        assert_eq!(
            AgentPresence::Busy.on_run_finish(FabricHealth::Down),
            AgentPresence::Offline
        );
    }

    #[test]
    fn a_shed_verdict_overrides_an_in_flight_run() {
        assert_eq!(
            AgentPresence::Busy.on_status(FabricHealth::Shed),
            AgentPresence::RateLimited
        );
        assert_eq!(
            AgentPresence::Busy.on_status(FabricHealth::Down),
            AgentPresence::Offline
        );
        assert_eq!(
            AgentPresence::Busy.on_status(FabricHealth::Healthy),
            AgentPresence::Busy
        );
        assert_eq!(
            AgentPresence::Offline.on_status(FabricHealth::Healthy),
            AgentPresence::Available
        );
    }

    #[test]
    fn only_available_is_dispatchable() {
        assert!(AgentPresence::Available.dispatchable());
        assert!(!AgentPresence::Busy.dispatchable());
        assert!(!AgentPresence::RateLimited.dispatchable());
        assert!(!AgentPresence::Offline.dispatchable());
    }

    #[test]
    fn presence_is_glyph_plus_label_never_colour_only() {
        let classes = [
            AgentPresence::Available,
            AgentPresence::Busy,
            AgentPresence::RateLimited,
            AgentPresence::Offline,
        ];
        let mut glyphs = std::collections::BTreeSet::new();
        for c in classes {
            assert!(!c.label().is_empty());
            assert!(
                glyphs.insert(c.glyph()),
                "glyph for {:?} is not shape-distinct",
                c
            );
        }
        assert_eq!(glyphs.len(), classes.len());
    }

    #[test]
    fn partials_advance_the_cursor_monotonically() {
        let mut s = StreamSession::open("run-1");
        for seq in 1..=3 {
            let f = PartialFrame {
                correlation_id: "run-1".into(),
                seq,
                cumulative_text: format!("tok{seq}"),
                is_last: seq == 3,
            };
            assert!(s.apply_partial(&f), "in-order partial seq={seq} must apply");
        }
        match &s.state {
            StreamState::Streaming {
                last_seq,
                cumulative_text,
            } => {
                assert_eq!(*last_seq, 3);
                assert_eq!(cumulative_text, "tok3");
            }
            _ => panic!("must still be streaming"),
        }
    }

    #[test]
    fn an_out_of_order_partial_is_rejected_the_cursor_never_rewinds() {
        let mut s = StreamSession::open("run-1");
        let f1 = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "a".into(),
            is_last: false,
        };
        assert!(s.apply_partial(&f1));
        let replay = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "a".into(),
            is_last: false,
        };
        assert!(!s.apply_partial(&replay));
        let skip = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 3,
            cumulative_text: "abc".into(),
            is_last: false,
        };
        assert!(!s.apply_partial(&skip));
    }

    #[test]
    fn finalize_replaces_the_partial_with_the_final_durable_message() {
        let mut s = StreamSession::open("run-1");
        let f = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "hel".into(),
            is_last: true,
        };
        assert!(s.apply_partial(&f));
        assert!(!s.is_finalized());
        s.finalize("msg-99", "hello world");
        assert!(s.is_finalized());
        match &s.state {
            StreamState::Finalized {
                message_id,
                final_text,
            } => {
                assert_eq!(message_id, "msg-99");
                assert_eq!(final_text, "hello world");
            }
            _ => panic!("must be finalized"),
        }
    }

    #[test]
    fn a_late_partial_after_finalize_is_dropped_the_final_is_the_truth() {
        let mut s = StreamSession::open("run-1");
        s.finalize("msg-1", "final");
        let late = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "half".into(),
            is_last: false,
        };
        assert!(
            !s.apply_partial(&late),
            "a late partial must NOT un-finalize"
        );
        assert!(s.is_finalized());
    }

    #[test]
    fn finalize_is_idempotent_the_durable_id_is_immutable() {
        let mut s = StreamSession::open("run-1");
        s.finalize("msg-1", "first");
        s.finalize("msg-2", "second");
        match &s.state {
            StreamState::Finalized {
                message_id,
                final_text,
            } => {
                assert_eq!(message_id, "msg-1");
                assert_eq!(final_text, "first");
            }
            _ => panic!("must be finalized"),
        }
    }

    #[test]
    fn resume_mid_stream_returns_the_working_marker_never_a_half_message() {
        let mut s = StreamSession::open("run-1");
        let f = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 2,
            cumulative_text: "half a".into(),
            is_last: false,
        };
        let f1 = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "half".into(),
            is_last: false,
        };
        assert!(s.apply_partial(&f1));
        assert!(s.apply_partial(&f));
        let view = resume_view(&s);
        match view {
            ResumeView::InProgress { resume_from_seq } => assert_eq!(resume_from_seq, 2),
            ResumeView::Final { .. } => panic!("must NOT return a final mid-stream"),
        }
    }

    #[test]
    fn resume_after_finalize_returns_the_final_durable_message() {
        let mut s = StreamSession::open("run-1");
        let f = PartialFrame {
            correlation_id: "run-1".into(),
            seq: 1,
            cumulative_text: "hel".into(),
            is_last: true,
        };
        assert!(s.apply_partial(&f));
        s.finalize("msg-7", "hello");
        match resume_view(&s) {
            ResumeView::Final {
                message_id,
                final_text,
            } => {
                assert_eq!(message_id, "msg-7");
                assert_eq!(final_text, "hello");
            }
            ResumeView::InProgress { .. } => panic!("a finalized run must resume the FINAL"),
        }
    }

    #[test]
    fn reconnect_at_every_token_boundary_never_yields_a_half_message() {
        let runtime = MockStreamRuntime::new("run-42", "the quick brown fox");
        let partials = runtime.partials();
        for k in 0..=partials.len() {
            let mut s = StreamSession::open(runtime.correlation_id());
            for frame in partials.iter().take(k) {
                assert!(s.apply_partial(frame));
            }
            let final_submitted = k == partials.len();
            if final_submitted {
                let outcome = {
                    use myelin_agent::AgentRuntime;
                    runtime.step(&myelin_agent::Conversation::default())
                };
                if let myelin_agent::StepOutcome::Submit(myelin_agent::Submission(body)) = outcome {
                    s.finalize("msg-42", body);
                }
            }
            match resume_view(&s) {
                ResumeView::Final { final_text, .. } => {
                    assert!(
                        final_submitted,
                        "a Final at k={k} but the run had not submitted"
                    );
                    assert_eq!(final_text, "the quick brown fox");
                }
                ResumeView::InProgress { resume_from_seq } => {
                    assert!(
                        !final_submitted,
                        "an InProgress at k={k} but the run HAD submitted"
                    );
                    assert_eq!(resume_from_seq, k as u64);
                }
            }
        }
    }

    #[test]
    fn the_mock_streams_cumulative_partials_then_submits_the_same_answer() {
        let runtime = MockStreamRuntime::new("run-1", "hello brave world");
        let partials = runtime.partials();
        assert_eq!(partials.len(), 3);
        assert_eq!(partials[0].cumulative_text, "hello");
        assert_eq!(partials[1].cumulative_text, "hello brave");
        assert_eq!(partials[2].cumulative_text, "hello brave world");
        assert!(partials[2].is_last);
        assert_eq!(
            partials.iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        use myelin_agent::AgentRuntime;
        let outcome = runtime.step(&myelin_agent::Conversation::default());
        match outcome {
            myelin_agent::StepOutcome::Submit(myelin_agent::Submission(s)) => {
                assert_eq!(s, partials[2].cumulative_text);
            }
            _ => panic!("the mock must submit"),
        }
    }

    #[test]
    fn run_streamed_drives_partials_then_finalizes_to_the_submission() {
        let runtime = MockStreamRuntime::new("run-9", "alpha beta");
        let session = run_streamed(&runtime, "msg-9", None);
        assert!(session.is_finalized());
        match &session.state {
            StreamState::Finalized {
                message_id,
                final_text,
            } => {
                assert_eq!(message_id, "msg-9");
                assert_eq!(final_text, "alpha beta");
            }
            _ => panic!("run_streamed must finalize"),
        }
    }

    #[test]
    fn run_streamed_publishes_each_partial_on_the_firehose_port() {
        #[derive(Default)]
        struct CapturePush {
            frames: std::cell::RefCell<Vec<PartialFrame>>,
        }
        impl PartialPush for CapturePush {
            fn push_partial(&self, _scope: &FirehoseScope, frame: &PartialFrame) -> u64 {
                self.frames.borrow_mut().push(frame.clone());
                self.frames.borrow().len() as u64
            }
        }
        let runtime = MockStreamRuntime::new("run-3", "a b c");
        let push = CapturePush::default();
        let scope = FirehoseScope::parse("channel:c-1").expect("bounded scope");
        let session = run_streamed(&runtime, "msg-3", Some((&push, &scope)));
        assert!(session.is_finalized());
        let frames = push.frames.borrow();
        assert_eq!(frames.len(), 3);
        assert_eq!(
            frames.iter().map(|f| f.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn presence_and_partials_ride_the_firehose_only() {
        assert!(presence_and_partials_are_firehose_only());
        assert_eq!(
            delivery_class(CHAT_PRESENCE_CHANGED),
            Some(DeliveryClass::Firehose)
        );
        assert!(AGENT_MESSAGE_PARTIAL.starts_with("agent."));
        assert!(delivery_class(AGENT_MESSAGE_PARTIAL).is_none());
        assert!(AGENT_STATUS_CHANGED.starts_with("agent."));
    }

    #[test]
    fn sessions_registry_opens_and_resumes_by_correlation() {
        let mut sessions = StreamSessions::new();
        {
            let s = sessions.open("run-1");
            let f = PartialFrame {
                correlation_id: "run-1".into(),
                seq: 1,
                cumulative_text: "x".into(),
                is_last: true,
            };
            s.apply_partial(&f);
            s.finalize("msg-1", "done");
        }
        match sessions.resume("run-1") {
            Some(ResumeView::Final { message_id, .. }) => assert_eq!(message_id, "msg-1"),
            other => panic!("expected a final resume, got {other:?}"),
        }
        assert!(sessions.resume("run-unknown").is_none());
    }

    struct FakeAtt {
        artifact: String,
        drill: String,
        escapes: u32,
    }
    impl AgD4Attestation for FakeAtt {
        fn artifact_tag(&self) -> &str {
            &self.artifact
        }
        fn drill_id(&self) -> &str {
            &self.drill
        }
        fn total_escapes(&self) -> u32 {
            self.escapes
        }
    }

    #[test]
    fn ag_d4_assertion_is_fail_closed_without_an_attestation() {
        assert!(!ag_d4_attestation_is_green::<FakeAtt>(None));
    }

    #[test]
    fn ag_d4_assertion_admits_a_green_attestation_refuses_a_red_one() {
        let green = FakeAtt {
            artifact: "ag-d4-green-escape-attestation".into(),
            drill: "AG-D4 / CI-T1".into(),
            escapes: 0,
        };
        assert!(ag_d4_attestation_is_green(Some(&green)));
        let red = FakeAtt {
            artifact: "ag-d4-green-escape-attestation".into(),
            drill: "AG-D4 / CI-T1".into(),
            escapes: 1,
        };
        assert!(!ag_d4_attestation_is_green(Some(&red)));
        let wrong = FakeAtt {
            artifact: "some-other-artifact".into(),
            drill: "AG-D4 / CI-T1".into(),
            escapes: 0,
        };
        assert!(!ag_d4_attestation_is_green(Some(&wrong)));
    }
}
