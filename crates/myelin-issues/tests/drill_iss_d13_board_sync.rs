//! **ISS-D13 / ISS-P30 / P-397 (M4) — the real-time board-sync drill (0 ops lost on reconnect + the
//! resync fallback).**
//!
//! This is the prompt's GATE artifact (drill catalogue row ISS-D13): a board at `scope = board:<id>`
//! drops mid-edit-storm → `resume` backfill then live loses **ZERO ops**; a `last_seq` past the
//! retention window → `resync_required` → `*.snapshot` replay (NAMED, not silent). The two green
//! artifacts are: (1) the seq-gap survival signal reads 0 after the reconnect (every op replayed),
//! and (2) the resync-count survival signal fires (>= 1) on the over-window leg.
//!
//! **Reconciliation (EI-01 §7).** This drives the ONE frozen Bus-owned firehose resume-cursor
//! protocol (`myelin_events::Firehose`, contract 3.5, P-141/EB-21) through the Issues-layer board-sync
//! CONSUMER (`myelin_issues::BoardSync`) — it does NOT re-implement the transport. The §10.2 survival
//! signals are asserted through the FROZEN harness assertion library
//! (`SignalSource`/`Predicate`/`SignalName::{FirehoseFrameLag, ResyncRequiredCount}`) — the SAME
//! library the EB-21 D-10 firehose drill uses (the firehose owns the *measurement*; the harness owns
//! the *assertion* vocabulary). The connection-drop fault is the `DependencyBreaker` (P-S03).
//!
//! DB-free: the zero-loss property is a property of the transport protocol + the idempotent cache, not
//! of a store. The live connection-tier board-sync drill rides the shared Chat M4 gateway (P-403, a
//! named floor); this drill proves the Issues consumer's half against the in-process firehose floor.

use myelin_events::{Firehose, DEFAULT_INFLIGHT_CAP};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};
use myelin_issues::{BoardCard, BoardOp, BoardSync};
use myelin_substrate::firehose_selector::ScopeWindow;

/// A paginated board-sync over a huge board (the OQ-J co-designed ISS case): 200 visible + 50 margin —
/// a 50k-row board NEVER streams all 50k frames; the bounded `board:<id>` scope rejects `*`.
fn open_board(fh: &mut Firehose, board: &str) -> BoardSync {
    let mut bs = BoardSync::open("fan.acme.web", board, ScopeWindow::new(0, 200, 50))
        .expect("a bounded board:<id> scope");
    bs.subscribe(fh, None).expect("a bounded scope subscribes");
    bs
}

/// Bridge the board-sync's measured seq-gap + resync count into the FROZEN §10.2 harness assertion
/// library (the same bridge the EB-21 firehose drill uses).
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

/// **ISS-D13 LEG 1 — a board drops mid-edit-storm; `resume` backfill then live loses ZERO ops.** The
/// headline pass condition: the seq-gap survival signal reads 0 after the reconnect (every op in the
/// gap was replayed into the normalised cache).
#[test]
fn iss_d13_board_reconnect_loses_zero_ops() {
    let mut fh = Firehose::new();
    let board = "board:proj-42";
    let mut bs = open_board(&mut fh, board);

    // the viewer consumes the board filling live (10 row-edits, seq 1..10).
    let breaker = DependencyBreaker::new();
    for i in 1..=10u64 {
        fh.publish(
            bs.stream(),
            bs.scope(),
            BoardOp::Upsert(BoardCard::new(format!("I-{i}"), "todo", "m")).to_draft(),
        );
    }
    let applied = bs.pump();
    assert_eq!(applied, 10, "the viewer saw 10 ops while connected");
    assert_eq!(
        bs.last_seq(),
        10,
        "its resume cursor is the last delivered seq"
    );
    assert_eq!(bs.cache().len(), 10);

    // DROP the firehose connection mid-edit-storm (the catalogue's "drop a firehose connection"
    // fault, reversibly — P-S03). While down, the storm keeps editing: seq 11..25 (the gap).
    breaker.break_dependency(Dependency::Firehose, Scope::Global);
    assert!(
        breaker.is_broken(&Dependency::Firehose, &Scope::Global),
        "the connection is down"
    );
    for i in 11..=25u64 {
        // a mix of creates + moves + a remove — a realistic edit-storm, all into the disconnected gap.
        let op = if i % 5 == 0 {
            BoardOp::Move {
                issue_id: format!("I-{}", i - 10),
                state_category: "in_progress".into(),
            }
        } else {
            BoardOp::Upsert(BoardCard::new(format!("I-{i}"), "todo", "m"))
        };
        fh.publish(bs.stream(), bs.scope(), op.to_draft());
    }
    assert_eq!(
        fh.head_seq(bs.stream(), bs.scope()),
        25,
        "the storm reached seq 25"
    );

    // heal + RECONNECT: resume(last_seq = 10) → backfill (10, 25] then live. EVERY gap op applied.
    breaker.restore_dependency(Dependency::Firehose, Scope::Global);
    let gap = bs
        .reconnect(&mut fh)
        .expect("in-window resume backfills the edit-storm gap");
    assert_eq!(
        gap, 15,
        "the whole 15-op edit-storm gap was replayed — 0 ops lost"
    );
    assert_eq!(bs.last_seq(), 25, "the cursor advanced to the head");
    assert_eq!(
        bs.resync_required_count(),
        0,
        "no resync needed (the gap was in-window)"
    );

    // THE GREEN ARTIFACT: the seq-gap survival signal reads 0 after the reconnect (no op outstanding).
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
    .expect_green(); // ISS-D13: 0 ops lost on reconnect (the seq-gap is closed)
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Eq(0))
        .expect_green(); // the in-window reconnect needed no resync

    // live continues gap-free after the reconnect.
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-final", "todo", "m")).to_draft(),
    );
    assert_eq!(bs.pump(), 1, "live continues gap-free");
    assert!(bs.cache().card("I-final").is_some());
}

/// **ISS-D13 LEG 2 — a `last_seq` past the retention window → `resync_required` → `*.snapshot` replay
/// (NAMED, not silent).** A SMALL window forces the out-of-window path; the reconnect raises
/// `resync_required`; the board falls back to a full snapshot rebuild — the resync-count survival
/// signal fires (>= 1).
#[test]
fn iss_d13_past_window_cursor_resyncs_to_snapshot() {
    // a window holding only the most-recent 3 frames forces the out-of-window resync deterministically.
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let board = "board:proj-42";
    let mut bs = open_board(&mut fh, board);

    // publish 1..8; the viewer catches up to seq 2 then drops. The window now holds {6,7,8}; the
    // viewer's cursor (2) is far past the window floor.
    for i in 1..=8u64 {
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
        "the viewer's cursor is at 2 (now far past the window floor)"
    );

    // RECONNECT → the gap's head (op 3) was evicted → resync_required (NAMED, not silent).
    let err = bs
        .reconnect(&mut fh)
        .expect_err("an out-of-window cursor cannot backfill");
    assert!(
        err.is_resync_required(),
        "the over-window cursor RAISES resync_required"
    );

    // FALL BACK to a *.snapshot replay (contract 2.6 / issue.issue.snapshot): the authoritative board
    // as of seq 8 (8 cards). The cold-rebuild path is taken LOUDLY.
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

    // THE GREEN ARTIFACT: the resync-count survival signal fired (>= 1) — the fallback is NAMED.
    let mut src = SignalSource::new();
    set_board_signals(&mut src, board, 0, bs.resync_required_count() as i64);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Gte(1))
        .expect_green(); // ISS-D13: the resync fallback fired + is NAMED (never a silent partial board)

    // post-resync, live continues from the snapshot seq gap-free.
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-9", "todo", "m")).to_draft(),
    );
    assert_eq!(bs.pump(), 1, "live continues after the resync");
    assert!(bs.cache().card("I-9").is_some());
    assert_eq!(bs.last_seq(), 9);
}
