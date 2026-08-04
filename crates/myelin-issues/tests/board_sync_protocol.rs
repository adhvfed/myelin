use myelin_events::{Firehose, FrameDraft, DEFAULT_INFLIGHT_CAP};
use myelin_issues::{BoardCard, BoardOp, BoardSync};
use myelin_substrate::firehose_selector::ScopeWindow;

fn open(stream: &str, scope: &str) -> BoardSync {
    BoardSync::open(stream, scope, ScopeWindow::new(0, 200, 50)).expect("a bounded board scope")
}

#[test]
fn live_frame_patches_the_cache() {
    let mut fh = Firehose::new();
    let mut bs = open("fan.acme.web", "board:p1");
    bs.subscribe(&mut fh, None)
        .expect("a bounded scope subscribes");
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-1", "todo", "m")).to_draft(),
    );
    bs.pump();

    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "in_review".into(),
        }
        .to_draft(),
    );
    let applied = bs.pump();
    assert_eq!(applied, 1, "one live op applied");
    assert_eq!(
        bs.cache().card("I-1").unwrap().state_category,
        "in_review",
        "the agent-moved card animated into the new lane (bus-driven invalidation)"
    );
}

#[test]
fn presence_frame_advances_cursor_without_patching_cache() {
    let mut fh = Firehose::new();
    let mut bs = open("fan.acme.web", "board:p1");
    bs.subscribe(&mut fh, None).unwrap();
    fh.publish(
        bs.stream(),
        bs.scope(),
        FrameDraft::new("presence|user-7|typing"),
    );
    bs.pump();
    assert!(bs.cache().is_empty(), "a presence frame patches no card");
    assert_eq!(
        bs.last_seq(),
        1,
        "the cursor still advanced (the frame was consumed)"
    );
}

#[test]
fn reconnect_backfills_the_edit_storm_gap_losing_zero_ops() {
    let mut fh = Firehose::new();
    let mut bs = open("fan.acme.web", "board:p1");
    bs.subscribe(&mut fh, None).unwrap();

    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-1", "todo", "a")).to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-2", "todo", "b")).to_draft(),
    );
    bs.pump();
    assert_eq!(bs.last_seq(), 2);
    assert_eq!(bs.cache().len(), 2);

    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "in_progress".into(),
        }
        .to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-3", "todo", "c")).to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Move {
            issue_id: "I-2".into(),
            state_category: "done".into(),
        }
        .to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-4", "todo", "d")).to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Reorder {
            issue_id: "I-1".into(),
            order_key: "z".into(),
        }
        .to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-5", "todo", "e")).to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Remove {
            issue_id: "I-3".into(),
        }
        .to_draft(),
    );
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Move {
            issue_id: "I-4".into(),
            state_category: "done".into(),
        }
        .to_draft(),
    );
    assert_eq!(
        fh.head_seq(bs.stream(), bs.scope()),
        10,
        "the storm reached seq 10"
    );

    let gap = bs.reconnect(&mut fh).expect("in-window resume backfills");
    assert_eq!(
        gap, 8,
        "the whole edit-storm gap (3..10) was applied - 0 ops lost"
    );
    assert_eq!(bs.last_seq(), 10, "the cursor advanced to the head");

    assert_eq!(
        bs.cache().card("I-1").unwrap().state_category,
        "in_progress"
    );
    assert_eq!(bs.cache().card("I-1").unwrap().order_key, "z");
    assert_eq!(bs.cache().card("I-2").unwrap().state_category, "done");
    assert!(
        bs.cache().card("I-3").is_none(),
        "I-3 was removed mid-storm"
    );
    assert_eq!(bs.cache().card("I-4").unwrap().state_category, "done");
    assert!(
        bs.cache().card("I-5").is_some(),
        "I-5 was created mid-storm"
    );

    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-6", "todo", "f")).to_draft(),
    );
    assert_eq!(bs.pump(), 1, "live continues gap-free");
    assert_eq!(bs.last_seq(), 11);
    assert!(bs.cache().card("I-6").is_some());
}

#[test]
fn reconnect_is_idempotent_no_double_apply() {
    let mut fh = Firehose::new();
    let mut bs = open("fan.acme.web", "board:p1");
    bs.subscribe(&mut fh, None).unwrap();
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
    bs.pump();
    let lane_before = bs.cache().card("I-1").unwrap().state_category.clone();

    bs.reconnect(&mut fh).expect("re-backfill is in-window");
    assert_eq!(
        bs.cache().card("I-1").unwrap().state_category,
        lane_before,
        "re-applying the backfilled ops is idempotent (zero-dup)"
    );
    assert_eq!(bs.cache().len(), 1, "no duplicate card was created");
}

#[test]
fn past_window_cursor_resyncs_to_snapshot_named_not_silent() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let mut bs = open("fan.acme.web", "board:p1");
    bs.subscribe(&mut fh, None).unwrap();

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
    assert_eq!(
        bs.last_seq(),
        2,
        "the viewer's cursor is at 2 (now evicted)"
    );

    let err = bs
        .reconnect(&mut fh)
        .expect_err("an out-of-window cursor cannot backfill");
    assert!(
        err.is_resync_required(),
        "the over-window cursor RAISES resync_required"
    );
    assert_eq!(
        bs.resync_required_count(),
        0,
        "not yet resynced (the signal was just raised)"
    );

    let snapshot: Vec<BoardCard> = (1..=6u64)
        .map(|i| BoardCard::new(format!("I-{i}"), "todo", "m"))
        .collect();
    bs.resync_from_snapshot(&mut fh, snapshot, 6)
        .expect("the snapshot resync re-subscribes");
    assert_eq!(
        bs.cache().len(),
        6,
        "the cache was rebuilt from the full snapshot"
    );
    assert_eq!(
        bs.last_seq(),
        6,
        "the cursor re-pinned to the snapshot's as_of_seq"
    );
    assert_eq!(
        bs.resync_required_count(),
        1,
        "the cold-rebuild path was taken LOUDLY (NAMED)"
    );

    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-7", "todo", "m")).to_draft(),
    );
    assert_eq!(bs.pump(), 1, "live continues after the resync");
    assert!(bs.cache().card("I-7").is_some());
    assert_eq!(bs.last_seq(), 7);
}
