//! # CDC 3.5 (substrate half) — the firehose per-connection caps + slow-consumer drop (P-S28 → P-135)
//!
//! **Contract-index:** row 3.5 (`Firehose transport + the resume-cursor subscription protocol`). The
//! protocol is **Bus-owned** (`subscribe`/`resume`/`scope` + the zero-loss-replay half, landing in
//! P-141/EB-21); THIS consumer-driven contract test exercises the **substrate's bounded-and-sheds
//! half** from OUTSIDE the crate — the consumer is the firehose transport / connection tier (Chat M4)
//! that opens a per-connection [`FrameBuffer`] and offers frames to it. It pins the two halves the Bus
//! and the substrate agree on at the seam (§7.7):
//!
//! - **(a) per-connection in-flight frame caps** — a frame offered over-cap SHEDS in the firehose's
//!   own bounded queue; buffered frames never exceed the cap (memory bounded, Little's Law).
//! - **(b) slow-consumer drop to `resync_required`** — a consumer whose lag crosses the slow-consumer
//!   ceiling is DROPPED to `resync_required` (released, memory bounded), to fall back to a `*.snapshot`
//!   replay (NAMED, not silent).
//!
//! The provider side is [`myelin_substrate::firehose`] (the `FrameBuffer` + the `FirehoseSignals`).
//! This is the consumer (the transport offering frames + reading the survival signals). It is the
//! dated green artifact's CDC half (the unit half is `firehose::tests`; the SUB-D11 hot-stream drill
//! is `tests/drill_sub_d11_firehose_slow_consumer.rs`). The **zero-loss-replay** half of D-11 (zero
//! ops lost across a reconnect) needs the Bus impl — **P-141**, named, not asserted here.

use myelin_substrate::{
    FirehoseScope, FirehoseSignals, Frame, FrameBuffer, FrameClass, PushOutcome,
};

fn scope(s: &str) -> FirehoseScope {
    FirehoseScope(s.to_string())
}

fn frame(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::HumanDelivery)
}

/// **CDC 3.5 (a) — the transport offers frames; an over-cap subscription sheds in the firehose's own
/// bounded queue (never grows memory).** The connection-tier consumer agrees with the substrate that
/// a buffer at its per-connection cap sheds the next frame rather than buffering it unboundedly.
#[test]
fn cdc_3_5_over_cap_subscription_sheds_in_the_firehose_bounded_queue() {
    let mut buf = FrameBuffer::new("chat-live", scope("channel:eng"), 3, 1_000);
    // the transport pushes frames the consumer has not yet drained → they stay in flight up to the cap.
    assert!(buf.offer(frame(1)).is_buffered());
    assert!(buf.offer(frame(2)).is_buffered());
    assert!(buf.offer(frame(3)).is_buffered());
    assert_eq!(buf.buffered_frames(), 3, "the buffer is at its per-connection cap");
    // the next frame is OVER cap → it sheds in the firehose's own bounded queue (§7.7 (a) / §7.1).
    assert_eq!(buf.offer(frame(4)), PushOutcome::Shed);
    assert_eq!(buf.buffered_frames(), 3, "buffered frames NEVER exceed the cap (bounded memory)");
    assert_eq!(buf.shed_count(), 1, "the over-cap shed is counted (the bounded-streaming signal)");
}

/// **CDC 3.5 (b) — a slow consumer is dropped to `resync_required` (not buffered unboundedly).** The
/// transport and the substrate agree: a consumer whose lag crosses the slow-consumer ceiling is
/// dropped (its buffer released, memory bounded) to fall back to a `*.snapshot` replay — the
/// cold-rebuild path NAMED, not a silent memory growth.
#[test]
fn cdc_3_5_slow_consumer_is_dropped_to_resync_required_not_buffered() {
    let mut buf = FrameBuffer::new("kn-ops", scope("doc:design"), 4, 8);
    // a fast producer races ahead of a fully-stalled consumer (it never delivers). The lag climbs.
    let mut last = PushOutcome::Buffered;
    for seq in 1..=8u64 {
        last = buf.offer(frame(seq));
    }
    // the lag reached the ceiling → the slow consumer is dropped to resync_required.
    assert_eq!(last, PushOutcome::ResyncRequired, "a slow consumer is dropped (not buffered)");
    assert!(buf.resync_required(), "the connection is dropped to the *.snapshot cold-rebuild path");
    // MEMORY IS BOUNDED: the dropped buffer holds nothing (it did not buffer the gap).
    assert_eq!(buf.buffered_frames(), 0, "a dropped connection releases its buffer (bounded memory)");
    assert_eq!(buf.frame_lag(), 0, "a dropped connection holds no gap (it is in *.snapshot replay)");
    assert_eq!(buf.resync_required_count(), 1, "the resync_required count is accurate + NAMED");
}

/// **CDC 3.5 — the transport reads the per-`(stream,scope)` frame-lag + `resync_required` count
/// survival signals (§10.2 last row).** The consumer (the connection tier's metrics scrape) snapshots
/// the firehose signals off its open buffers; the frame-lag is bounded and the resync count accurate.
#[test]
fn cdc_3_5_transport_reads_frame_lag_and_resync_required_signals() {
    let mut fast = FrameBuffer::new("chat-live", scope("channel:fast"), 4, 8);
    let mut slow = FrameBuffer::new("chat-live", scope("channel:slow"), 4, 8);
    // a keeping-up consumer (lag ~0) + a stalled one (dropped to resync).
    for seq in 1..=5u64 {
        fast.offer(frame(seq));
        fast.deliver(frame(seq));
    }
    for seq in 1..=8u64 {
        slow.offer(frame(seq));
    }
    let sig = FirehoseSignals::snapshot([&fast, &slow]);
    assert_eq!(sig.frame_lag.len(), 2, "one (stream,scope) frame-lag row per open buffer");
    assert!(sig.max_frame_lag() <= 8, "every (stream,scope) frame-lag is BOUNDED by the ceiling");
    assert_eq!(sig.resync_required_count, 1, "the resync_required count is accurate across buffers");
}

/// **CDC 3.5 — a keeping-up consumer is never dropped; the lag stays bounded near 0 and no resync
/// fires.** The happy path the transport relies on: delivery keeps pace with offers.
#[test]
fn cdc_3_5_keeping_up_consumer_is_never_dropped() {
    let mut buf = FrameBuffer::new("ci-logs", scope("board:42"), 4, 16);
    for seq in 1..=200u64 {
        assert!(buf.offer(frame(seq)).is_buffered(), "a keeping-up consumer never sheds");
        buf.deliver(frame(seq));
    }
    assert!(!buf.resync_required(), "a keeping-up consumer is never dropped to resync_required");
    assert_eq!(buf.resync_required_count(), 0);
    assert!(buf.frame_lag() <= 1, "the lag stays bounded near 0");
}
