use myelin_substrate::{
    FirehoseScope, FirehoseSignals, Frame, FrameBuffer, FrameClass, PushOutcome,
};

fn scope(s: &str) -> FirehoseScope {
    FirehoseScope(s.to_string())
}

fn frame(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::HumanDelivery)
}

#[test]
fn cdc_3_5_over_cap_subscription_sheds_in_the_firehose_bounded_queue() {
    let mut buf = FrameBuffer::new("chat-live", scope("channel:eng"), 3, 1_000);
    assert!(buf.offer(frame(1)).is_buffered());
    assert!(buf.offer(frame(2)).is_buffered());
    assert!(buf.offer(frame(3)).is_buffered());
    assert_eq!(
        buf.buffered_frames(),
        3,
        "the buffer is at its per-connection cap"
    );
    assert_eq!(buf.offer(frame(4)), PushOutcome::Shed);
    assert_eq!(
        buf.buffered_frames(),
        3,
        "buffered frames NEVER exceed the cap (bounded memory)"
    );
    assert_eq!(
        buf.shed_count(),
        1,
        "the over-cap shed is counted (the bounded-streaming signal)"
    );
}

#[test]
fn cdc_3_5_slow_consumer_is_dropped_to_resync_required_not_buffered() {
    let mut buf = FrameBuffer::new("kn-ops", scope("doc:design"), 4, 8);
    let mut last = PushOutcome::Buffered;
    for seq in 1..=8u64 {
        last = buf.offer(frame(seq));
    }
    assert_eq!(
        last,
        PushOutcome::ResyncRequired,
        "a slow consumer is dropped (not buffered)"
    );
    assert!(
        buf.resync_required(),
        "the connection is dropped to the *.snapshot cold-rebuild path"
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
        "the resync_required count is accurate + NAMED"
    );
}

#[test]
fn cdc_3_5_transport_reads_frame_lag_and_resync_required_signals() {
    let mut fast = FrameBuffer::new("chat-live", scope("channel:fast"), 4, 8);
    let mut slow = FrameBuffer::new("chat-live", scope("channel:slow"), 4, 8);
    for seq in 1..=5u64 {
        fast.offer(frame(seq));
        fast.deliver(frame(seq));
    }
    for seq in 1..=8u64 {
        slow.offer(frame(seq));
    }
    let sig = FirehoseSignals::snapshot([&fast, &slow]);
    assert_eq!(
        sig.frame_lag.len(),
        2,
        "one (stream,scope) frame-lag row per open buffer"
    );
    assert!(
        sig.max_frame_lag() <= 8,
        "every (stream,scope) frame-lag is BOUNDED by the ceiling"
    );
    assert_eq!(
        sig.resync_required_count, 1,
        "the resync_required count is accurate across buffers"
    );
}

#[test]
fn cdc_3_5_keeping_up_consumer_is_never_dropped() {
    let mut buf = FrameBuffer::new("ci-logs", scope("board:42"), 4, 16);
    for seq in 1..=200u64 {
        assert!(
            buf.offer(frame(seq)).is_buffered(),
            "a keeping-up consumer never sheds"
        );
        buf.deliver(frame(seq));
    }
    assert!(
        !buf.resync_required(),
        "a keeping-up consumer is never dropped to resync_required"
    );
    assert_eq!(buf.resync_required_count(), 0);
    assert!(buf.frame_lag() <= 1, "the lag stays bounded near 0");
}
