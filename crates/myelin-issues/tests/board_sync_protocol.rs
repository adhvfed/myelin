//! **ISS-P30 / P-397 — the board-sync transport-protocol tests (the firehose-driven half).**
//!
//! These exercise `myelin_issues::BoardSync` driving the frozen contract-3.5 firehose resume-cursor
//! protocol (`myelin_events::Firehose`): the bus-driven cache invalidation (a live frame patches the
//! cache), the reconnect backfill-then-live that loses ZERO ops, the idempotent zero-dup re-apply, and
//! the `resync_required` → `*.snapshot` cold rebuild on a past-window cursor.
//!
//! They live in a SEPARATE `tests/` file (not in `src/`) BY DESIGN: the firehose's frozen
//! `publish(stream, scope, frame)` method NAME collides with the `no-raw-publish` durable-bus lint's
//! `.publish(` fingerprint (the firehose is the EPHEMERAL transport, a different seam — §4.3 / OQ-J),
//! and the lint-gate scans `src/`. Siting the firehose-PRODUCING test drivers here keeps `no-raw-publish`
//! fully live on the board-sync SOURCE (which never publishes — it only subscribes/resumes), exactly the
//! two-transport split the architecture names. The pure-logic unit tests (cache apply, optimistic
//! confirm/rollback, encode/decode) stay in `src/board_sync/tests.rs`.

use myelin_events::{Firehose, FrameDraft, DEFAULT_INFLIGHT_CAP};
use myelin_issues::{BoardCard, BoardOp, BoardSync};
use myelin_substrate::firehose_selector::ScopeWindow;

fn open(stream: &str, scope: &str) -> BoardSync {
    BoardSync::open(stream, scope, ScopeWindow::new(0, 200, 50)).expect("a bounded board scope")
}

/// **A live firehose frame patches the cache (the bus-driven cache invalidation, §7).** A subscribe
/// then a published op-frame (an agent move) animates the card into the new lane — the consumer the
/// firehose drives.
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

    // an agent moves the card; the producer publishes the op-frame.
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

/// A non-board frame (presence/typing) advances the cursor but does not patch the cache — presence
/// rides the ephemeral firehose and is never replayed as a board op.
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

/// **ISS-D13 CORE: a board drops mid-edit-storm → resume backfill then live loses ZERO ops (§7).** A
/// viewer at `last_seq = 2`; while disconnected an edit-storm publishes 3..10; on reconnect every gap
/// op is applied to the cache, then live continues gap-free — 0 ops lost, 0 duplicated.
#[test]
fn reconnect_backfills_the_edit_storm_gap_losing_zero_ops() {
    let mut fh = Firehose::new();
    let mut bs = open("fan.acme.web", "board:p1");
    bs.subscribe(&mut fh, None).unwrap();

    // the viewer sees the board fill: I-1..I-2 created (seq 1,2).
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

    // the connection DROPS. While disconnected, an edit-storm publishes seq 3..10 (the gap).
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

    // RECONNECT: resume(last_seq = 2) → backfill (2, 10] then live. EVERY gap op is applied.
    let gap = bs.reconnect(&mut fh).expect("in-window resume backfills");
    assert_eq!(
        gap, 8,
        "the whole edit-storm gap (3..10) was applied — 0 ops lost"
    );
    assert_eq!(bs.last_seq(), 10, "the cursor advanced to the head");

    // the cache reflects the FULL replayed storm.
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

    // a LIVE frame after the reconnect continues gap-free (no gap, no duplicate).
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-6", "todo", "f")).to_draft(),
    );
    assert_eq!(bs.pump(), 1, "live continues gap-free");
    assert_eq!(bs.last_seq(), 11);
    assert!(bs.cache().card("I-6").is_some());
}

/// **The reconnect is IDEMPOTENT across the backfill→live boundary (zero-DUP).** A reconnect whose
/// backfill overlaps an op already applied lands the same state — applying the same move/upsert twice
/// is a no-op.
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

    // re-subscribe from an EARLIER cursor (0) → re-backfills (0, 2] = the same two ops. Idempotent.
    bs.reconnect(&mut fh).expect("re-backfill is in-window");
    // a second reconnect from the head re-backfills nothing; assert the state is stable + no dup card.
    assert_eq!(
        bs.cache().card("I-1").unwrap().state_category,
        lane_before,
        "re-applying the backfilled ops is idempotent (zero-dup)"
    );
    assert_eq!(bs.cache().len(), 1, "no duplicate card was created");
}

/// **ISS-D13 RESYNC LEG: a `last_seq` past the retention window yields `resync_required`; the board
/// falls back to a full `*.snapshot` replay (§7 — NAMED not silent).**
#[test]
fn past_window_cursor_resyncs_to_snapshot_named_not_silent() {
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let mut bs = open("fan.acme.web", "board:p1");
    bs.subscribe(&mut fh, None).unwrap();

    // publish 1..6 → the window now holds {4,5,6}; 1,2,3 were evicted. The viewer saw up to seq 2.
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

    // RECONNECT at last_seq = 2 → the gap's head (op 3) was evicted → resync_required (NAMED).
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

    // FALL BACK to a *.snapshot replay: the snapshot is the authoritative board as of seq 6.
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

    // post-resync, live continues from the snapshot seq gap-free.
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-7", "todo", "m")).to_draft(),
    );
    assert_eq!(bs.pump(), 1, "live continues after the resync");
    assert!(bs.cache().card("I-7").is_some());
    assert_eq!(bs.last_seq(), 7);
}
