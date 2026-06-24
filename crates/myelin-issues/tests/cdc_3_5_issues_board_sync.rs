//! **ISS-P30 / P-397 — the contract-3.5 CONSUMER-leg CDC pair (Issues board sync over the firehose
//! resume-cursor protocol).**
//!
//! Contract 3.5 is the firehose transport + resume-cursor protocol (OQ-J), co-designed ONCE for a
//! huge board (ISS) / a hot doc (KN) / a hot channel (CHAT). The Bus PROVIDES the protocol
//! (`myelin_events::Firehose` — `subscribe(stream, scope, cursor?)` / `resume(stream, scope,
//! last_seq)` / per-`(stream, scope)` monotone `seq` / `(last_seq, now]` backfill / `resync_required`
//! → `*.snapshot` / bounded scope never `*`, P-141/EB-21). Issues CONSUMES it for the board case via
//! `myelin_issues::BoardSync`.
//!
//! This CDC pair pins that the Issues consumer drives the PROVIDER's frozen shape EXACTLY — the SAME
//! `subscribe`/`resume`/`resync_required` vocabulary, the SAME bounded `board:<id>` scope through the
//! ONE `*`-rejecting chokepoint, the SAME per-`(stream, scope)` monotone seq. The consumer adds NO
//! second transport and NO second resume-cursor implementation (EI-01 §7). If the Bus protocol shape
//! drifts, this pair fails to compile/assert — the cross-system seam is held.

use myelin_events::{Firehose, FirehoseError, FirehoseScope, DEFAULT_INFLIGHT_CAP};
use myelin_issues::{BoardCard, BoardOp, BoardSync};
use myelin_substrate::firehose_selector::ScopeWindow;

fn window() -> ScopeWindow {
    ScopeWindow::new(0, 200, 50)
}

/// **PROVIDER ↔ CONSUMER: the board scope the consumer constructs is the SAME bounded
/// `FirehoseScope` the PROVIDER admits — and `*` is rejected at the SAME chokepoint.** The consumer
/// (`BoardSync::open`) parses `board:<id>` through `FirehoseScope::parse`; the resulting scope equals
/// the provider's parse of the same string; an over-broad scope is rejected identically.
#[test]
fn consumer_board_scope_equals_provider_scope_and_star_is_rejected() {
    // the bounded board scope the consumer drives == the provider's parse of the same selector.
    let bs = BoardSync::open("fan.acme.web", "board:proj-7", window()).expect("bounded scope");
    let provider_scope = FirehoseScope::parse("board:proj-7").expect("provider parses board:<id>");
    assert_eq!(
        bs.scope(),
        &provider_scope,
        "the consumer's board scope IS the provider's bounded FirehoseScope (no second validator)"
    );
    assert_eq!(bs.scope().selector(), "board:proj-7");

    // `*` / over-broad is rejected at the SAME chokepoint on both legs.
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

/// **PROVIDER ↔ CONSUMER: the consumer's `reconnect` drives the provider's `resume` and consumes the
/// exact `(last_seq, now]` backfill — 0 ops lost, the per-`(stream, scope)` monotone seq honoured.**
/// The provider assigns seqs 1..N on one scope; the consumer reconnects at its cursor and receives
/// EXACTLY the gap, in seq order, applied to the cache.
#[test]
fn consumer_reconnect_consumes_provider_resume_backfill_losing_zero_ops() {
    let mut fh = Firehose::new();
    let mut bs = BoardSync::open("fan.acme.web", "board:p1", window()).expect("bounded scope");
    bs.subscribe(&mut fh, None)
        .expect("subscribe with no cursor starts live");

    // the PROVIDER assigns per-(stream, scope) monotone seqs 1..3; the consumer applies them.
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

    // the gap 4..6 is published while disconnected; the consumer reconnects (drives `resume(3)`).
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
        "the consumer received EXACTLY the (3, 6] gap — 0 ops lost"
    );
    assert_eq!(bs.cache().len(), 6, "every backfilled op is in the cache");
    assert_eq!(bs.last_seq(), 6);
}

/// **PROVIDER ↔ CONSUMER: an over-window cursor surfaces the provider's `resync_required` to the
/// consumer, which falls back to a `*.snapshot` (NAMED, not silent).** The provider raises
/// `FirehoseError::ResyncRequired`; the consumer surfaces it (does not silently partial-replay) and
/// rebuilds from a snapshot.
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
    // the consumer's reconnect surfaces the PROVIDER's resync_required verdict (NAMED).
    let err = bs
        .reconnect(&mut fh)
        .expect_err("over-window cursor cannot backfill");
    assert!(
        matches!(err, FirehoseError::ResyncRequired { .. }),
        "the consumer surfaces the provider's resync_required (never a silent partial board)"
    );
    // the consumer falls back to the *.snapshot cold rebuild.
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

/// **CONSUMER idempotence: a frame re-delivered across the backfill→live boundary lands the same
/// state (the zero-DUP half of the provider's zero-loss guarantee).** The consumer applies ops
/// idempotently on the card id, so an overlapping resume never double-applies.
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

    // apply the same two ops twice (the in-process FrameBuffer's BoardOp decode) — same final state.
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
