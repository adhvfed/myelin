use myelin_events::{Firehose, FirehoseError, FirehoseScope, DEFAULT_INFLIGHT_CAP};
use myelin_issues::{BoardCard, BoardOp, BoardSync};
use myelin_substrate::firehose_selector::ScopeWindow;

fn window() -> ScopeWindow {
    ScopeWindow::new(0, 200, 50)
}

#[test]
fn consumer_board_scope_equals_provider_scope_and_star_is_rejected() {
    let bs = BoardSync::open("fan.acme.web", "board:proj-7", window()).expect("bounded scope");
    let provider_scope = FirehoseScope::parse("board:proj-7").expect("provider parses board:<id>");
    assert_eq!(
        bs.scope(),
        &provider_scope,
        "the consumer's board scope IS the provider's bounded FirehoseScope (no second validator)"
    );
    assert_eq!(bs.scope().selector(), "board:proj-7");

    assert!(
        BoardSync::open("fan.acme.web", "*", window())
            .err()
            .unwrap()
            .is_over_broad_scope(),
        "the consumer rejects `*` at the provider's chokepoint"
    );
    assert!(
        FirehoseScope::parse("*").unwrap_err().is_over_broad_scope(),
        "the provider rejects `*` identically"
    );
}

#[test]
fn consumer_reconnect_consumes_provider_resume_backfill_losing_zero_ops() {
    let mut fh = Firehose::new();
    let mut bs = BoardSync::open("fan.acme.web", "board:p1", window()).expect("bounded scope");
    bs.subscribe(&mut fh, None)
        .expect("subscribe with no cursor starts live");

    for i in 1..=3u64 {
        let frame = fh.publish(
            bs.stream(),
            bs.scope(),
            BoardOp::Upsert(BoardCard::new(format!("I-{i}"), "todo", "m")).to_draft(),
        );
        assert_eq!(
            frame.seq, i,
            "the PROVIDER assigns the per-(stream, scope) monotone seq"
        );
    }
    bs.pump();
    assert_eq!(
        bs.last_seq(),
        3,
        "the consumer's cursor tracks the provider's seq"
    );

    for i in 4..=6u64 {
        fh.publish(
            bs.stream(),
            bs.scope(),
            BoardOp::Upsert(BoardCard::new(format!("I-{i}"), "todo", "m")).to_draft(),
        );
    }
    let gap = bs.reconnect(&mut fh).expect("in-window resume backfills");
    assert_eq!(
        gap, 3,
        "the consumer received EXACTLY the (3, 6] gap - 0 ops lost"
    );
    assert_eq!(bs.cache().len(), 6, "every backfilled op is in the cache");
    assert_eq!(bs.last_seq(), 6);
}

#[test]
fn consumer_surfaces_provider_resync_required_and_rebuilds_from_snapshot() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let mut bs = BoardSync::open("fan.acme.web", "board:p1", window()).expect("bounded scope");
    bs.subscribe(&mut fh, None).expect("subscribe");

    for i in 1..=6u64 {
        fh.publish(
            bs.stream(),
            bs.scope(),
            BoardOp::Upsert(BoardCard::new(format!("I-{i}"), "todo", "m")).to_draft(),
        );
        if i == 2 {
            bs.pump();
        }
    }
    let err = bs
        .reconnect(&mut fh)
        .expect_err("over-window cursor cannot backfill");
    assert!(
        matches!(err, FirehoseError::ResyncRequired { .. }),
        "the consumer surfaces the provider's resync_required (never a silent partial board)"
    );
    let snapshot: Vec<BoardCard> = (1..=6u64)
        .map(|i| BoardCard::new(format!("I-{i}"), "todo", "m"))
        .collect();
    bs.resync_from_snapshot(&mut fh, snapshot, 6)
        .expect("the snapshot resync re-subscribes");
    assert_eq!(bs.cache().len(), 6, "the cache rebuilt from the *.snapshot");
    assert_eq!(
        bs.resync_required_count(),
        1,
        "the resync fallback fired (NAMED)"
    );
}

#[test]
fn consumer_apply_is_idempotent_zero_dup() {
    let mut fh = Firehose::new();
    let mut bs = BoardSync::open("fan.acme.web", "board:p1", window()).expect("bounded scope");
    bs.subscribe(&mut fh, None).expect("subscribe");
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-1", "todo", "a")).to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "done".into(),
        }
        .to_draft(),
    );

    let mut a = bs.cache().clone();
    let ops = [
        BoardOp::Upsert(BoardCard::new("I-1", "todo", "a")),
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "done".into(),
        },
    ];
    for _ in 0..2 {
        for op in &ops {
            a.apply(op);
        }
    }
    assert_eq!(
        a.len(),
        1,
        "no duplicate card created (idempotent on the card id)"
    );
    assert_eq!(
        a.card("I-1").unwrap().state_category,
        "done",
        "the same final state"
    );
}
