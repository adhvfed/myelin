use myelin_substrate::{
    BoundedSelector, Frame, FrameClass, FrameOutcome, FrameSelector, ScopeWindow,
};

fn presence(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::Presence)
}
fn human(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::HumanDelivery)
}

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

#[test]
fn cdc_3_5_storm_frame_lag_stays_bounded_for_a_keeping_up_consumer() {
    let mut conn = connection("storm-fast", 4, 16);
    let mut seq = 0u64;
    for burst in 0..125u64 {
        for f in 0..8u64 {
            let frame = if f == 7 { human(seq) } else { presence(seq) };
            if conn.offer(frame, None) == FrameOutcome::Buffered {
                conn.deliver(frame);
            }
            seq += 1;
        }
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

#[test]
fn cdc_3_5_storm_slow_consumer_dropped_keeping_up_neighbour_holds() {
    let mut fast = connection("storm-fast", 4, 16);
    let mut slow = connection("storm-slow", 4, 16);

    let mut seq = 0u64;
    for _ in 0..200u64 {
        for f in 0..8u64 {
            let frame = if f == 7 { human(seq) } else { presence(seq) };
            if fast.offer(frame, None) == FrameOutcome::Buffered {
                fast.deliver(frame);
            }
            slow.offer(frame, None);
            seq += 1;
        }
    }

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
    assert!(
        !fast.buffer().resync_required(),
        "a slow consumer never drops a keeping-up neighbour (per-connection, never stream-wide)"
    );
}

#[test]
fn cdc_3_5_storm_presence_sheds_before_message_lane_holds() {
    let mut conn = connection("storm-classes", 8, 100_000);
    let mut seq = 0u64;
    let mut presence_offered = 0u64;
    let mut human_buffered = 0u64;
    for _ in 0..20u64 {
        for _ in 0..6u64 {
            conn.offer(presence(seq), None);
            presence_offered += 1;
            seq += 1;
        }
        if conn.offer(human(seq), None) == FrameOutcome::Buffered {
            human_buffered += 1;
        }
        seq += 1;
    }

    let presence_shed = conn.budget().shed_count(FrameClass::Presence);
    let human_shed = conn.budget().shed_count(FrameClass::HumanDelivery);

    assert!(presence_offered > 0);
    assert!(
        presence_shed >= 1,
        "presence frames shed under the storm (the ephemeral lane absorbs pressure first)"
    );
    assert_eq!(
        human_shed, 0,
        "message (human) frames are shed LAST - the protected lane holds under the storm"
    );
    assert!(
        human_buffered >= 1,
        "message frames still buffered while presence shed (the lane held)"
    );
}
