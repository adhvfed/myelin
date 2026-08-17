use myelin_events::{Firehose, FirehoseError, FirehoseScope, FrameDraft, DEFAULT_INFLIGHT_CAP};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};

fn scope(s: &str) -> FirehoseScope {
    FirehoseScope::parse(s).expect("a bounded scope")
}

fn set_firehose_signals(
    src: &mut SignalSource,
    stream: &str,
    scope: &str,
    seq_gap: i64,
    resync: i64,
) {
    src.set_labelled(
        SignalName::FirehoseFrameLag,
        vec![Label::new("stream", stream), Label::new("scope", scope)],
        seq_gap,
    );
    src.set_scalar(SignalName::ResyncRequiredCount, resync);
}

#[test]
fn d10_reconnect_backfills_then_live_loses_zero_ops() {
    let mut fh = Firehose::new();
    let stream = "kn-ops";
    let s = scope("doc:hot-design");

    let breaker = DependencyBreaker::new();
    let sub = fh
        .subscribe(stream, &s, None)
        .expect("bounded scope subscribes");
    for _ in 0..3 {
        fh.publish(stream, &s, FrameDraft::new("op"))
            .expect("the fixture publishes a valid frame");
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(seen, vec![1, 2, 3], "the client saw 1,2,3 while connected");
    let last_seq = sub.last_seq();
    assert_eq!(last_seq, 3, "its resume cursor is the last delivered seq");

    breaker.break_dependency(Dependency::Firehose, Scope::Global);
    assert!(
        breaker.is_broken(&Dependency::Firehose, &Scope::Global),
        "the connection is down"
    );
    for _ in 0..4 {
        fh.publish(stream, &s, FrameDraft::new("op"))
            .expect("the fixture publishes a valid frame");
    }

    breaker.restore_dependency(Dependency::Firehose, Scope::Global);
    let resumed = fh
        .resume(stream, &s, last_seq)
        .expect("an in-window resume backfills the gap");
    let backfilled: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        backfilled,
        vec![4, 5, 6, 7],
        "the gap (last_seq, now] is replayed - 0 ops lost"
    );

    fh.publish(stream, &s, FrameDraft::new("op"))
        .expect("the fixture publishes a valid frame");
    let live: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        live,
        vec![8],
        "live continues contiguously - 0 duplicate across the boundary"
    );

    let mut total = seen;
    total.extend(backfilled);
    total.extend(live);
    assert_eq!(
        total,
        (1..=8).collect::<Vec<u64>>(),
        "every op delivered exactly once: 0 lost, 0 dup"
    );

    let remaining_gap = (fh.head_seq(stream, &s) - resumed.last_seq()) as i64;
    let mut src = SignalSource::new();
    set_firehose_signals(&mut src, stream, &s.selector(), remaining_gap, 0);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Eq(0))
        .expect_green();
    src.assert_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", stream),
            Label::new("scope", s.selector()),
        ],
        Predicate::Eq(0),
    )
    .expect_green();
}

#[test]
fn d10_out_of_window_resume_raises_resync_required() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let stream = "chat-live";
    let s = scope("channel:town-hall");

    let last_seq = 2u64;
    for _ in 0..8 {
        fh.publish(stream, &s, FrameDraft::new("msg"))
            .expect("the fixture publishes a valid frame");
    }
    assert_eq!(
        fh.window_len(stream, &s),
        3,
        "the retention window is bounded"
    );

    let err = fh
        .resume(stream, &s, last_seq)
        .expect_err("out-of-window resume cannot backfill");
    assert!(
        err.is_resync_required(),
        "an out-of-window cursor RAISES resync_required"
    );
    let resync_fired = if let FirehoseError::ResyncRequired { window_floor, .. } = err {
        assert_eq!(
            window_floor, 6,
            "the window floor is the oldest held seq (6)"
        );
        1
    } else {
        0
    };

    let mut src = SignalSource::new();
    set_firehose_signals(&mut src, stream, &s.selector(), 0, resync_fired);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Gte(1))
        .expect_green();
}

#[test]
fn d10_transport_rejects_an_over_broad_scope() {
    let mut fh = Firehose::new();
    let err = fh
        .subscribe_raw("chat-live", "*", None)
        .expect_err("the transport rejects an over-broad scope");
    assert!(
        err.is_over_broad_scope(),
        "scope = * is rejected (BUS-3 generalised)"
    );
    assert!(
        fh.subscribe_raw("chat-live", "channel:eng", None).is_ok(),
        "a bounded scope subscribes"
    );
}

#[test]
fn d10_board_scope_reconnect_loses_zero_ops() {
    let mut fh = Firehose::new();
    let stream = "issues";
    let s = scope("board:proj-42");

    let sub = fh
        .subscribe(stream, &s, None)
        .expect("a board scope subscribes");
    for _ in 0..10 {
        fh.publish(stream, &s, FrameDraft::new("row-edit"))
            .expect("the fixture publishes a valid frame");
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(seen, (1..=10).collect::<Vec<u64>>());

    for _ in 0..10 {
        fh.publish(stream, &s, FrameDraft::new("row-edit"))
            .expect("the fixture publishes a valid frame");
    }
    let resumed = fh
        .resume(stream, &s, sub.last_seq())
        .expect("board resume backfills");
    let gap: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(
        gap,
        (11..=20).collect::<Vec<u64>>(),
        "the whole edit-storm gap is replayed - 0 lost"
    );

    let mut src = SignalSource::new();
    let remaining = (fh.head_seq(stream, &s) - resumed.last_seq()) as i64;
    set_firehose_signals(&mut src, stream, &s.selector(), remaining, 0);
    src.assert_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", stream),
            Label::new("scope", s.selector()),
        ],
        Predicate::Eq(0),
    )
    .expect_green();
}
