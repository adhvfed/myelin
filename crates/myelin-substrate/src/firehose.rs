use crate::shed::BoundedQueue;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FrameClass {
    Presence,
    AgentDelivery,
    HumanDelivery,
}

impl FrameClass {
    pub fn label(self) -> &'static str {
        match self {
            FrameClass::Presence => "presence",
            FrameClass::AgentDelivery => "agent",
            FrameClass::HumanDelivery => "human",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FirehoseScope(pub String);

impl FirehoseScope {
    pub fn selector(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub seq: u64,
    pub class: FrameClass,
}

impl Frame {
    pub fn new(seq: u64, class: FrameClass) -> Frame {
        Frame { seq, class }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    Buffered,
    Shed,
    ResyncRequired,
}

impl PushOutcome {
    pub fn is_buffered(self) -> bool {
        matches!(self, PushOutcome::Buffered)
    }

    pub fn is_resync_required(self) -> bool {
        matches!(self, PushOutcome::ResyncRequired)
    }
}

#[derive(Clone, Debug)]
pub struct FrameBuffer {
    stream: String,
    scope: FirehoseScope,
    queue: BoundedQueue,
    slow_consumer_lag_ceiling: u64,
    offered_seq: u64,
    delivered_seq: u64,
    resync_required: bool,
    resync_required_count: u64,
}

impl FrameBuffer {
    pub fn new(
        stream: impl Into<String>,
        scope: FirehoseScope,
        capacity: u32,
        slow_consumer_lag_ceiling: u64,
    ) -> FrameBuffer {
        let capacity = capacity.max(1);
        FrameBuffer {
            stream: stream.into(),
            scope,
            queue: BoundedQueue::new(capacity),
            slow_consumer_lag_ceiling: slow_consumer_lag_ceiling.max(capacity as u64),
            offered_seq: 0,
            delivered_seq: 0,
            resync_required: false,
            resync_required_count: 0,
        }
    }

    pub fn offer(&mut self, frame: Frame) -> PushOutcome {
        if self.resync_required {
            return PushOutcome::ResyncRequired;
        }
        self.offered_seq = self.offered_seq.max(frame.seq);

        if self.frame_lag() >= self.slow_consumer_lag_ceiling {
            self.drop_to_resync();
            return PushOutcome::ResyncRequired;
        }

        if self.queue.try_acquire() {
            PushOutcome::Buffered
        } else {
            PushOutcome::Shed
        }
    }

    pub fn note_shed_offer(&mut self, frame: Frame) -> PushOutcome {
        if self.resync_required {
            return PushOutcome::ResyncRequired;
        }
        self.offered_seq = self.offered_seq.max(frame.seq);
        if self.frame_lag() >= self.slow_consumer_lag_ceiling {
            self.drop_to_resync();
            return PushOutcome::ResyncRequired;
        }
        PushOutcome::Shed
    }

    pub fn deliver(&mut self, frame: Frame) {
        if self.resync_required {
            return;
        }
        self.queue.release();
        self.delivered_seq = self.delivered_seq.max(frame.seq);
    }

    pub fn frame_lag(&self) -> u64 {
        if self.resync_required {
            return 0;
        }
        self.offered_seq.saturating_sub(self.delivered_seq)
    }

    pub fn resync_required(&self) -> bool {
        self.resync_required
    }

    pub fn resync_required_count(&self) -> u64 {
        self.resync_required_count
    }

    pub fn buffered_frames(&self) -> u32 {
        self.queue.in_flight()
    }

    pub fn capacity(&self) -> u32 {
        self.queue.capacity()
    }

    pub fn shed_count(&self) -> u64 {
        self.queue.shed_count()
    }

    pub fn stream(&self) -> &str {
        &self.stream
    }

    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }

    fn drop_to_resync(&mut self) {
        if self.resync_required {
            return;
        }
        for _ in 0..self.queue.in_flight() {
            self.queue.release();
        }
        self.delivered_seq = self.offered_seq;
        self.resync_required = true;
        self.resync_required_count += 1;
    }
}

#[derive(Clone, Debug, Default)]
pub struct FirehoseSignals {
    pub frame_lag: Vec<FrameLagSample>,
    pub resync_required_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameLagSample {
    pub stream: String,
    pub scope: String,
    pub lag: u64,
}

impl FirehoseSignals {
    pub fn snapshot<'a>(buffers: impl IntoIterator<Item = &'a FrameBuffer>) -> FirehoseSignals {
        let mut frame_lag = Vec::new();
        let mut resync_required_count = 0u64;
        for b in buffers {
            frame_lag.push(FrameLagSample {
                stream: b.stream().to_string(),
                scope: b.scope().selector().to_string(),
                lag: b.frame_lag(),
            });
            resync_required_count += b.resync_required_count();
        }
        FirehoseSignals {
            frame_lag,
            resync_required_count,
        }
    }

    pub fn max_frame_lag(&self) -> u64 {
        self.frame_lag.iter().map(|s| s.lag).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(s: &str) -> FirehoseScope {
        FirehoseScope(s.to_string())
    }

    fn human(seq: u64) -> Frame {
        Frame::new(seq, FrameClass::HumanDelivery)
    }

    #[test]
    fn over_cap_subscription_sheds_rather_than_growing_memory() {
        let mut buf = FrameBuffer::new("chat-live", scope("channel:eng"), 3, 1_000);
        assert_eq!(buf.offer(human(1)), PushOutcome::Buffered);
        assert_eq!(buf.offer(human(2)), PushOutcome::Buffered);
        assert_eq!(buf.offer(human(3)), PushOutcome::Buffered);
        assert_eq!(buf.buffered_frames(), 3, "the buffer is at its cap");
        assert_eq!(
            buf.offer(human(4)),
            PushOutcome::Shed,
            "an over-cap frame sheds, never buffers"
        );
        assert_eq!(
            buf.buffered_frames(),
            3,
            "buffered frames NEVER exceed the cap (Little's Law)"
        );
        assert_eq!(
            buf.shed_count(),
            1,
            "the shed is counted (the bounded-streaming signal)"
        );
        buf.deliver(human(1));
        assert_eq!(buf.buffered_frames(), 2, "delivery freed a slot");
        assert_eq!(
            buf.offer(human(5)),
            PushOutcome::Buffered,
            "a freed slot is reusable"
        );
    }

    #[test]
    fn slow_consumer_is_dropped_to_resync_required_with_bounded_memory() {
        let mut buf = FrameBuffer::new("kn-ops", scope("doc:design"), 4, 8);
        for seq in 1..=7u64 {
            let out = buf.offer(human(seq));
            assert!(
                matches!(out, PushOutcome::Buffered | PushOutcome::Shed),
                "below the slow-consumer ceiling a frame buffers or sheds, never drops yet: seq={seq} {out:?}"
            );
        }
        assert!(
            !buf.resync_required(),
            "not dropped yet (lag 7 < ceiling 8)"
        );
        assert_eq!(
            buf.frame_lag(),
            7,
            "the frame-lag tracks the producer-vs-consumer gap"
        );
        assert_eq!(
            buf.buffered_frames(),
            4,
            "memory bounded at the cap even as the lag climbs"
        );

        let out = buf.offer(human(8));
        assert_eq!(
            out,
            PushOutcome::ResyncRequired,
            "a slow consumer is dropped to resync_required"
        );
        assert!(
            buf.resync_required(),
            "the connection is dropped (the cold-rebuild path, NAMED)"
        );
        assert_eq!(
            buf.buffered_frames(),
            0,
            "a dropped connection releases its buffer (bounded memory)"
        );
        assert_eq!(
            buf.frame_lag(),
            0,
            "a dropped connection holds no gap (it is in *.snapshot replay)"
        );
        assert_eq!(
            buf.resync_required_count(),
            1,
            "the resync_required count is accurate (one drop)"
        );
    }

    #[test]
    fn a_dropped_connection_stays_dropped_and_counts_the_drop_once() {
        let mut buf = FrameBuffer::new("ci-logs", scope("board:42"), 2, 3);
        assert_eq!(buf.offer(human(1)), PushOutcome::Buffered);
        assert_eq!(buf.offer(human(2)), PushOutcome::Buffered);
        assert_eq!(buf.offer(human(3)), PushOutcome::ResyncRequired);
        assert_eq!(buf.resync_required_count(), 1);
        assert_eq!(buf.offer(human(4)), PushOutcome::ResyncRequired);
        assert_eq!(buf.offer(human(5)), PushOutcome::ResyncRequired);
        assert_eq!(
            buf.resync_required_count(),
            1,
            "the drop is counted EXACTLY once per connection"
        );
        assert_eq!(buf.buffered_frames(), 0, "memory stays released");
        buf.deliver(human(2));
        assert_eq!(buf.buffered_frames(), 0);
    }

    #[test]
    fn a_keeping_up_consumer_is_never_dropped_and_lag_stays_bounded() {
        let mut buf = FrameBuffer::new("chat-live", scope("channel:eng"), 4, 8);
        for seq in 1..=100u64 {
            assert_eq!(
                buf.offer(human(seq)),
                PushOutcome::Buffered,
                "a keeping-up consumer never sheds"
            );
            buf.deliver(human(seq));
            assert!(
                buf.frame_lag() <= 1,
                "lag stays bounded (~0) for a keeping-up consumer"
            );
        }
        assert!(
            !buf.resync_required(),
            "a keeping-up consumer is never dropped"
        );
        assert_eq!(buf.resync_required_count(), 0);
        assert_eq!(buf.shed_count(), 0, "no shed on the happy path");
    }

    #[test]
    fn slow_consumer_ceiling_is_never_below_the_cap() {
        let mut buf = FrameBuffer::new("kn-ops", scope("doc:x"), 5, 1);
        for seq in 1..=4u64 {
            assert_eq!(
                buf.offer(human(seq)),
                PushOutcome::Buffered,
                "seq {seq} must buffer, not pre-drop"
            );
        }
        assert!(
            !buf.resync_required(),
            "a healthy connection filling its cap is NOT 'slow'"
        );
        assert_eq!(
            buf.offer(human(5)),
            PushOutcome::ResyncRequired,
            "the drop fires once the lag reaches the cap-raised ceiling, never before the cap"
        );
        assert!(buf.resync_required());
    }

    #[test]
    fn firehose_signals_export_frame_lag_and_resync_required_count() {
        let mut fast = FrameBuffer::new("chat-live", scope("channel:fast"), 4, 8);
        let mut slow = FrameBuffer::new("chat-live", scope("channel:slow"), 4, 8);

        for seq in 1..=3u64 {
            fast.offer(human(seq));
            fast.deliver(human(seq));
        }
        for seq in 1..=8u64 {
            slow.offer(human(seq));
        }
        assert!(slow.resync_required());

        let sig = FirehoseSignals::snapshot([&fast, &slow]);
        assert_eq!(sig.frame_lag.len(), 2);
        assert!(
            sig.max_frame_lag() <= 8,
            "every (stream,scope) frame-lag is BOUNDED by the ceiling"
        );
        let fast_row = sig
            .frame_lag
            .iter()
            .find(|r| r.scope == "channel:fast")
            .unwrap();
        assert!(fast_row.lag <= 1, "the keeping-up scope's lag is ~0");
        assert_eq!(
            sig.resync_required_count, 1,
            "the resync_required count is accurate + NAMED"
        );
    }

    #[test]
    fn accessors_read_back_the_buffer_state_exactly() {
        let mut buf = FrameBuffer::new("ci-logs", scope("board:7"), 5, 9);
        assert_eq!(buf.stream(), "ci-logs", "the stream key reads back exactly");
        assert_eq!(
            buf.scope().selector(),
            "board:7",
            "the scope selector reads back exactly"
        );
        assert_eq!(
            buf.capacity(),
            5,
            "the per-connection cap reads back exactly"
        );

        let buffered = buf.offer(human(1));
        assert!(
            buffered.is_buffered(),
            "a buffered frame reads is_buffered() == true"
        );
        assert!(
            !buffered.is_resync_required(),
            "a buffered frame is NOT resync_required"
        );

        for seq in 2..=9u64 {
            buf.offer(human(seq));
        }
        assert!(buf.resync_required(), "the buffer dropped to resync");
        let dropped = buf.offer(human(10));
        assert!(
            dropped.is_resync_required(),
            "a dropped offer reads is_resync_required() == true"
        );
        assert!(!dropped.is_buffered(), "a dropped offer is NOT buffered");

        assert_eq!(
            FirehoseSignals::default().max_frame_lag(),
            0,
            "an empty signal set has 0 max lag"
        );
        let mut a = FrameBuffer::new("s", scope("doc:a"), 8, 16);
        let mut b = FrameBuffer::new("s", scope("doc:b"), 8, 16);
        for seq in 1..=3u64 {
            a.offer(human(seq));
        }
        b.offer(human(1));
        b.deliver(human(1));
        let sig = FirehoseSignals::snapshot([&a, &b]);
        assert_eq!(
            sig.max_frame_lag(),
            3,
            "max_frame_lag is the LARGEST (stream,scope) lag, not 0/1"
        );
    }

    #[test]
    fn frame_class_shed_order_is_presence_then_agent_then_human() {
        assert!(FrameClass::Presence < FrameClass::AgentDelivery);
        assert!(FrameClass::AgentDelivery < FrameClass::HumanDelivery);
        assert_eq!(FrameClass::Presence.label(), "presence");
        assert_eq!(FrameClass::HumanDelivery.label(), "human");
    }
}
