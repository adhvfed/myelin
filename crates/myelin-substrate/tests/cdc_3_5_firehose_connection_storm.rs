//! # CDC 3.5 (substrate half) under CONNECTION-STORM load (P-S31 → global P-326)
//!
//! **Contract-index:** row 3.5 (`Firehose transport + the resume-cursor subscription protocol`). The
//! P-S28/P-S29 CDC pairs (`cdc_3_5_firehose_backpressure.rs`, `cdc_3_5_firehose_scope_selector.rs`)
//! pin the substrate's bounded-and-sheds half at **unit scale**. THIS CDC pair re-confirms the SAME
//! seam **under the real connection-storm load shape** (the P-S31 M4 deliverable): the connection-tier
//! consumer (Chat M4) and the substrate agree that, under a sustained frame storm, the survival-signal
//! shape the consumer reads off the substrate's firehose layer HOLDS —
//!
//!   - the per-`(stream,scope)` frame-lag is BOUNDED by the slow-consumer ceiling (memory never grows
//!     unboundedly, even under a 30× storm — §7.7 / Little's Law);
//!   - a slow consumer is DROPPED to `resync_required` (NAMED) and holds 0 frames, while a keeping-up
//!     consumer on the SAME stream is untouched (per-connection isolation, never stream-wide);
//!   - presence/speculative frames shed BEFORE message (human) frames (§7.6 connection-tier row) — the
//!     protected message lane holds under the storm.
//!
//! The provider side is [`myelin_substrate::firehose_selector`] / [`myelin_substrate::firehose`] (the
//! `FrameSelector` + the survival signals). This is the consumer (the connection tier offering a
//! storm of frames + reading the survival signals). The full SUB-D11 connection-storm drill (the dated
//! green artifact) is `tests/drill_sub_d11_connection_storm.rs`; this is the seam-level CDC pair under
//! storm load. The **zero-loss-replay** half of D-11 (zero ops lost across a reconnect) needs the Bus
//! impl — **P-141**; Chat owns the end-to-end CHAT-D1/D13/D14 resume-0-lost/0-dup drill (this CDC pair
//! proves the substrate's bounded/shed precondition holds under the storm Chat's drill runs over).

use myelin_substrate::{
    BoundedSelector, Frame, FrameClass, FrameOutcome, FrameSelector, ScopeWindow,
};

fn presence(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::Presence)
}
fn human(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::HumanDelivery)
}

/// Open a connection-tier [`FrameSelector`] on a bounded `channel:` scope (the connection-storm surface).
fn connection(id: &str, cap: u32, ceiling: u64) -> FrameSelector {
    let sel = BoundedSelector::parse(&format!("channel:{id}")).expect("a bounded channel selector");
    FrameSelector::new(
        "chat-live",
        &sel,
        cap,
        ceiling,
        ScopeWindow::new(0, 1, u64::MAX),
    )
}

/// **CDC 3.5 under storm — the per-`(stream,scope)` frame-lag stays BOUNDED under a sustained frame
/// flood (never grows unboundedly).** The connection tier floods a keeping-up consumer with a long storm
/// of mixed-class frames; the consumer drains each buffered frame; the lag oscillates near 0, far below
/// the slow-consumer ceiling — memory is bounded by the cap, not the storm length (§7.7).
#[test]
fn cdc_3_5_storm_frame_lag_stays_bounded_for_a_keeping_up_consumer() {
    let mut conn = connection("storm-fast", 4, 16);
    let mut seq = 0u64;
    // a long storm: 1000 frames, 7 presence chatter + 1 human message per "burst" (the connection-storm
    // fan-out shape). The keeping-up consumer delivers each buffered frame.
    for burst in 0..125u64 {
        for f in 0..8u64 {
            let frame = if f == 7 { human(seq) } else { presence(seq) };
            if conn.offer(frame, None) == FrameOutcome::Buffered {
                conn.deliver(frame);
            }
            seq += 1;
        }
        // the lag stays BOUNDED throughout the storm — never grows with the storm length.
        assert!(
            conn.buffer().frame_lag() <= 16,
            "frame-lag must stay bounded by the ceiling under the storm (burst {burst})"
        );
        assert!(
            conn.buffer().buffered_frames() <= conn.buffer().capacity(),
            "buffered frames never exceed the cap (Little's Law) under the storm"
        );
    }
    assert!(
        !conn.buffer().resync_required(),
        "a keeping-up consumer is NEVER dropped, no matter how long the storm"
    );
}

/// **CDC 3.5 under storm — a slow consumer is DROPPED to `resync_required` while a keeping-up neighbour
/// on the same stream HOLDS (per-connection isolation, never stream-wide).** The seam contract the
/// connection tier relies on under the storm: one stalled client cannot take down its neighbours.
#[test]
fn cdc_3_5_storm_slow_consumer_dropped_keeping_up_neighbour_holds() {
    let mut fast = connection("storm-fast", 4, 16);
    let mut slow = connection("storm-slow", 4, 16);

    let mut seq = 0u64;
    for _ in 0..200u64 {
        for f in 0..8u64 {
            let frame = if f == 7 { human(seq) } else { presence(seq) };
            // the fast consumer keeps up (delivers buffered frames); the slow one never delivers.
            if fast.offer(frame, None) == FrameOutcome::Buffered {
                fast.deliver(frame);
            }
            slow.offer(frame, None);
            seq += 1;
        }
    }

    // the slow consumer was DROPPED (NAMED), holds 0 frames, lag 0 (it is in *.snapshot replay).
    assert!(
        slow.buffer().resync_required(),
        "the slow consumer must be dropped to resync_required under the storm"
    );
    assert_eq!(
        slow.buffer().buffered_frames(),
        0,
        "a dropped connection releases its buffer (bounded memory)"
    );
    assert_eq!(
        slow.buffer().resync_required_count(),
        1,
        "dropped exactly once"
    );
    // the keeping-up neighbour on the SAME stream is UNTOUCHED — per-connection isolation.
    assert!(
        !fast.buffer().resync_required(),
        "a slow consumer never drops a keeping-up neighbour (per-connection, never stream-wide)"
    );
}

/// **CDC 3.5 under storm — presence/speculative frames shed BEFORE message (human) frames; the
/// protected message lane HOLDS.** Under the storm, the §7.6 frame-shed order is the seam contract: the
/// connection tier relies on the substrate shedding ephemeral presence chatter first so message delivery
/// survives. A presence-heavy storm on a stalled-enough buffer sheds presence by class while sparse human
/// (message) frames still buffer.
#[test]
fn cdc_3_5_storm_presence_sheds_before_message_lane_holds() {
    // cap 8 → v1 floor: presence 2, agent 4, human 8. A high ceiling so the CLASS budget fires (not the
    // slow-consumer drop). The consumer does NOT deliver, so the class budgets fill and shed.
    let mut conn = connection("storm-classes", 8, 100_000);
    let mut seq = 0u64;
    // a presence-heavy storm: many presence frames, a few human (message) frames interleaved.
    let mut presence_offered = 0u64;
    let mut human_buffered = 0u64;
    for _ in 0..20u64 {
        // 6 presence chatter frames...
        for _ in 0..6u64 {
            conn.offer(presence(seq), None);
            presence_offered += 1;
            seq += 1;
        }
        // ...then 1 human message frame.
        if conn.offer(human(seq), None) == FrameOutcome::Buffered {
            human_buffered += 1;
        }
        seq += 1;
    }

    let presence_shed = conn.budget().shed_count(FrameClass::Presence);
    let human_shed = conn.budget().shed_count(FrameClass::HumanDelivery);

    assert!(presence_offered > 0);
    // presence/speculative frames shed (the lowest class budget fills first).
    assert!(
        presence_shed >= 1,
        "presence frames shed under the storm (the ephemeral lane absorbs pressure first)"
    );
    // the protected message lane HOLDS — human (message) frames never class-shed (shed LAST).
    assert_eq!(
        human_shed, 0,
        "message (human) frames are shed LAST — the protected lane holds under the storm"
    );
    assert!(
        human_buffered >= 1,
        "message frames still buffered while presence shed (the lane held)"
    );
}
