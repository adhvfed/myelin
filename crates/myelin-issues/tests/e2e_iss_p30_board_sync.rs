//! **ISS-P30 / P-397 — the chained-mutation board-sync e2e (edit-storm → drop → resume → 0 ops
//! lost).**
//!
//! The DoD's chained-mutation e2e: a board viewer applies an optimistic edit, the server confirms it,
//! an agent edit-storm runs while the connection is DROPPED, then the viewer RECONNECTS and resumes —
//! asserting EVERY op in the gap is replayed into the normalised cache (0 ops lost), live continues
//! gap-free, and a server REJECT of a later optimistic edit rolls it back to its exact prior state.
//!
//! This drives the ONE frozen Bus-owned firehose resume-cursor protocol (`myelin_events::Firehose`,
//! contract 3.5) through the Issues-layer board-sync consumer (`myelin_issues::BoardSync`). It is
//! DB-free (the zero-loss property is a property of the transport protocol + the idempotent cache);
//! the live connection-tier drill is the shared Chat M4 gateway (P-403, a named floor).

use myelin_events::Firehose;
use myelin_issues::{BoardCard, BoardOp, BoardSync};
use myelin_substrate::firehose_selector::ScopeWindow;

fn board(fh: &mut Firehose) -> BoardSync {
    // a paginated window over a huge board (200 visible + 50 margin) — never the whole 50k rows.
    let mut bs = BoardSync::open("fan.acme.web", "board:proj-7", ScopeWindow::new(0, 200, 50))
        .expect("a bounded board:<id> scope");
    bs.subscribe(fh, None).expect("a bounded scope subscribes");
    bs
}

/// **The chained mutation: optimistic edit → confirm → agent edit-storm under a drop → reconnect →
/// 0 ops lost → reject rolls back.** The full §7 board-sync lifecycle in one chain.
#[test]
fn board_sync_chained_mutation_loses_zero_ops() {
    let mut fh = Firehose::new();
    let mut bs = board(&mut fh);

    // (1) the board fills: I-1, I-2 created (the server emits the authoritative frames).
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
    assert_eq!(bs.cache().len(), 2);

    // (2) OPTIMISTIC local move of I-1 → in_progress (applied immediately), then a server CONFIRM.
    bs.apply_local(
        "mut-move-1",
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "in_progress".into(),
        },
    )
    .expect("optimistic move applies");
    assert_eq!(
        bs.cache().card("I-1").unwrap().state_category,
        "in_progress",
        "optimistic move shows immediately"
    );
    // the server confirms + emits the authoritative frame (re-applied idempotently — a no-op).
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "in_progress".into(),
        }
        .to_draft(),
    );
    assert!(bs.confirm_local("mut-move-1"));
    bs.pump();
    assert_eq!(
        bs.cache().card("I-1").unwrap().state_category,
        "in_progress"
    );

    // (3) the connection DROPS. An agent edit-storm runs while disconnected (the gap).
    let storm = [
        BoardOp::Upsert(BoardCard::new("I-3", "todo", "c")),
        BoardOp::Move {
            issue_id: "I-2".into(),
            state_category: "in_progress".into(),
        },
        BoardOp::Upsert(BoardCard::new("I-4", "todo", "d")),
        BoardOp::Reorder {
            issue_id: "I-1".into(),
            order_key: "z".into(),
        },
        BoardOp::Remove {
            issue_id: "I-3".into(),
        },
        BoardOp::Move {
            issue_id: "I-1".into(),
            state_category: "done".into(),
        },
        BoardOp::Upsert(BoardCard::new("I-5", "todo", "e")),
    ];
    for op in &storm {
        fh.publish(bs.stream(), bs.scope(), op.to_draft());
    }

    // (4) RECONNECT → resume(last_seq) backfills the whole storm gap. 0 ops lost.
    let gap = bs
        .reconnect(&mut fh)
        .expect("in-window resume backfills the gap");
    assert_eq!(
        gap,
        storm.len(),
        "every storm op was replayed on reconnect — 0 ops lost"
    );

    // the cache reflects the FULL chain: I-1 done+reordered, I-2 in_progress, I-3 removed, I-4/I-5 added.
    assert_eq!(bs.cache().card("I-1").unwrap().state_category, "done");
    assert_eq!(bs.cache().card("I-1").unwrap().order_key, "z");
    assert_eq!(
        bs.cache().card("I-2").unwrap().state_category,
        "in_progress"
    );
    assert!(
        bs.cache().card("I-3").is_none(),
        "I-3's create+remove both replayed"
    );
    assert!(bs.cache().card("I-4").is_some());
    assert!(bs.cache().card("I-5").is_some());

    // (5) a LATER optimistic edit that the server REJECTS rolls back to the exact prior state.
    let prior_lane = bs.cache().card("I-2").unwrap().state_category.clone();
    bs.apply_local(
        "mut-bad",
        BoardOp::Move {
            issue_id: "I-2".into(),
            state_category: "done".into(),
        },
    )
    .expect("optimistic edit applies");
    assert_eq!(
        bs.cache().card("I-2").unwrap().state_category,
        "done",
        "optimistic edit shows"
    );
    assert!(bs.reject_local("mut-bad"), "the server rejects → roll back");
    assert_eq!(
        bs.cache().card("I-2").unwrap().state_category,
        prior_lane,
        "the rejected edit rolled back to the EXACT prior state (never a silent loss)"
    );

    // (6) live continues gap-free after the whole chain.
    fh.publish(
        bs.stream(),
        bs.scope(),
        BoardOp::Upsert(BoardCard::new("I-6", "todo", "f")).to_draft(),
    );
    assert_eq!(bs.pump(), 1, "live continues gap-free");
    assert!(bs.cache().card("I-6").is_some());
}
