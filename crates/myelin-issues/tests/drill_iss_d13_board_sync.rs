use myelin_events::{Firehose, DEFAULT_INFLIGHT_CAP};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};
use myelin_issues::{BoardCard, BoardOp, BoardSync};
use myelin_substrate::firehose_selector::ScopeWindow;

fn open_board(fh: &mut Firehose, board: &str) -> BoardSync {
    let mut bs = BoardSync::open("fan.acme.web", board, ScopeWindow::new(0, 200, 50))
        .expect("a bounded board:<id> scope");
    bs.subscribe(fh, None).expect("a bounded scope subscribes");
    bs
}

fn set_board_signals(src: &mut SignalSource, scope: &str, seq_gap: i64, resync: i64) {
    src.set_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", "fan.acme.web"),
            Label::new("scope", scope),
        ],
        seq_gap,
    );
    src.set_scalar(SignalName::ResyncRequiredCount, resync);
}

#[test]
fn iss_d13_board_reconnect_loses_zero_ops() {
    let mut fh = Firehose::new();
    let board = "board:proj-42";
    let mut bs = open_board(&mut fh, board);

    let breaker = DependencyBreaker::new();
    for i in 1..=10u64 {
        fh.publish(
            bs.stream(),
            bs.scope(),
            BoardOp::Upsert(BoardCard::new(format!("I-{i}"), "todo", "m")).to_draft(),
        )
        .expect("the fixture publishes a valid frame");
    }
    let applied = bs.pump();
    assert_eq!(applied, 10, "the viewer saw 10 ops while connected");
    assert_eq!(
        bs.last_seq(),
        10,
        "its resume cursor is the last delivered seq"
    );
    assert_eq!(bs.cache().len(), 10);

    breaker.break_dependency(Dependency::Firehose, Scope::Global);
    assert!(
        breaker.is_broken(&Dependency::Firehose, &Scope::Global),
        "the connection is down"
    );
    for i in 11..=25u64 {
        let op = if i % 5 == 0 {
            BoardOp::Move {
                issue_id: format!("I-{}", i - 10),
                state_category: "in_progress".into(),
            }
        } else {
            BoardOp::Upsert(BoardCard::new(format!("I-{i}"), "todo", "m"))
        };
        fh.publish(bs.stream(), bs.scope(), op.to_draft())
            .expect("the fixture publishes a valid frame");
    }
    assert_eq!(
        fh.head_seq(bs.stream(), bs.scope()),
        25,
        "the storm reached seq 25"
    );

    breaker.restore_dependency(Dependency::Firehose, Scope::Global);
    let gap = bs
        .reconnect(&mut fh)
        .expect("in-window resume backfills the edit-storm gap");
    assert_eq!(
        gap, 15,
        "the whole 15-op edit-storm gap was replayed - 0 ops lost"
    );
    assert_eq!(bs.last_seq(), 25, "the cursor advanced to the head");
    assert_eq!(
        bs.resync_required_count(),
        0,
        "no resync needed (the gap was in-window)"
    );

    let remaining = (fh.head_seq(bs.stream(), bs.scope()) - bs.last_seq()) as i64;
    assert_eq!(remaining, 0, "0 ops outstanding after the reconnect");
    let mut src = SignalSource::new();
    set_board_signals(&mut src, board, remaining, 0);
    src.assert_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", "fan.acme.web"),
            Label::new("scope", board),
        ],
        Predicate::Eq(0),
    )
    .expect_green();
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Eq(0))
        .expect_green();

    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-final", "todo", "m")).to_draft(),
    )
    .expect("the fixture publishes a valid frame");
    assert_eq!(bs.pump(), 1, "live continues gap-free");
    assert!(bs.cache().card("I-final").is_some());
}

#[test]
fn iss_d13_past_window_cursor_resyncs_to_snapshot() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let board = "board:proj-42";
    let mut bs = open_board(&mut fh, board);

    for i in 1..=8u64 {
        fh.publish(
            bs.stream(),
            bs.scope(),
            BoardOp::Upsert(BoardCard::new(format!("I-{i}"), "todo", "m")).to_draft(),
        )
        .expect("the fixture publishes a valid frame");
        if i == 2 {
            bs.pump();
        }
    }
    assert_eq!(
        bs.last_seq(),
        2,
        "the viewer's cursor is at 2 (now far past the window floor)"
    );

    let err = bs
        .reconnect(&mut fh)
        .expect_err("an out-of-window cursor cannot backfill");
    assert!(
        err.is_resync_required(),
        "the over-window cursor RAISES resync_required"
    );

    let snapshot: Vec<BoardCard> = (1..=8u64)
        .map(|i| BoardCard::new(format!("I-{i}"), "todo", "m"))
        .collect();
    bs.resync_from_snapshot(&mut fh, snapshot, 8)
        .expect("the snapshot resync re-subscribes live from seq 8");
    assert_eq!(
        bs.cache().len(),
        8,
        "the cache was rebuilt from the full *.snapshot"
    );
    assert_eq!(
        bs.last_seq(),
        8,
        "the cursor re-pinned to the snapshot's as_of_seq"
    );
    assert_eq!(
        bs.resync_required_count(),
        1,
        "the cold-rebuild path fired exactly once"
    );

    let mut src = SignalSource::new();
    set_board_signals(&mut src, board, 0, bs.resync_required_count() as i64);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Gte(1))
        .expect_green();

    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-9", "todo", "m")).to_draft(),
    )
    .expect("the fixture publishes a valid frame");
    assert_eq!(bs.pump(), 1, "live continues after the resync");
    assert!(bs.cache().card("I-9").is_some());
    assert_eq!(bs.last_seq(), 9);
}
