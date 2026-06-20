//! # D-10 — the firehose reconnect-loses-zero-ops drill (EB-21 / P-141)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row D-10
//! (firehose reconnect loses zero ops — architecture §8 D-10; the CI-D11 / ISS-D13 / KN-D1 /
//! CHAT-D1 surface variants all reduce to this transport property). Threshold: **0 ops lost; the
//! resync path correct; the transport rejects an over-broad scope**.
//!
//! ## What this drill proves (the EB-21 GATE)
//! 1. **0 OPS LOST across a reconnect.** Drop a subscribed connection mid-stream on a hot
//!    `(stream, scope)`; while it is down, more frames are published; `resume(last_seq)` backfills
//!    `(last_seq, now]` then goes live — the client sees EVERY op exactly once (0 lost, 0 duplicate).
//! 2. **The resync path is correct (NAMED not silent).** An out-of-window `last_seq` (older than the
//!    bounded retention window) yields `resync_required` — the signal is RAISED (the client falls
//!    back to a `*.snapshot` replay; the rebuild itself is EB-22 / P-142).
//! 3. **The transport rejects an over-broad scope.** `scope = *` is rejected (the whitelist-not-`*`
//!    rule, BUS-3, generalised).
//!
//! The drill reads its verdict off the **§10.2 firehose survival signals** through the FROZEN harness
//! assertion library (`SignalSource` / `Predicate` / `Assertion`, P-S04): `FirehoseFrameLag` (the
//! per-`(stream, scope)` seq-gap — asserted `== 0` after the reconnect backfill: no gap remains) and
//! `ResyncRequiredCount` (asserted `>= 1` on the over-window leg: the resync signal fired).
//!
//! The connection drop is driven through the harness `Dependency::Firehose` reversible break injector
//! (P-S03) so the drill is the catalogue's "drop a firehose connection" fault, reversibly.

use myelin_events::{Firehose, FirehoseError, FirehoseScope, FrameDraft, DEFAULT_INFLIGHT_CAP};
use myelin_harness::{
    Dependency, DependencyBreaker, Label, Predicate, Scope, SignalName, SignalSource,
};

fn scope(s: &str) -> FirehoseScope {
    FirehoseScope::parse(s).expect("a bounded scope")
}

/// Bridge the firehose's measured seq-gap + resync count into the FROZEN §10.2 harness assertion
/// library (the DEVIATION bridge the Bus's other drills use, e.g. EB-11's telemetry self-test): the
/// firehose protocol owns the *measurement*; the harness owns the *assertion* vocabulary.
fn set_firehose_signals(src: &mut SignalSource, stream: &str, scope: &str, seq_gap: i64, resync: i64) {
    src.set_labelled(
        SignalName::FirehoseFrameLag,
        vec![
            Label::new("stream", stream),
            Label::new("scope", scope),
        ],
        seq_gap,
    );
    src.set_scalar(SignalName::ResyncRequiredCount, resync);
}

/// **D-10 LEG 1 — drop a connection mid-stream; `resume(last_seq)` loses ZERO ops; the seq-gap
/// telemetry reads 0 after the backfill.** The headline pass condition.
#[test]
fn d10_reconnect_backfills_then_live_loses_zero_ops() {
    let mut fh = Firehose::new();
    let stream = "kn-ops";
    let s = scope("doc:hot-design"); // a hot doc (KN KD-8) — the OQ-J co-designed case.

    // a connected client consumes live up to seq 3.
    let breaker = DependencyBreaker::new();
    let sub = fh.subscribe(stream, &s, None).expect("bounded scope subscribes");
    for _ in 0..3 {
        fh.publish(stream, &s, FrameDraft::new("op"));
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(seen, vec![1, 2, 3], "the client saw 1,2,3 while connected");
    let last_seq = sub.last_seq();
    assert_eq!(last_seq, 3, "its resume cursor is the last delivered seq");

    // DROP the firehose connection mid-stream (the catalogue's "drop a firehose connection" fault,
    // reversibly — P-S03). While down, the producer keeps publishing the gap 4,5,6,7.
    breaker.break_dependency(Dependency::Firehose, Scope::Global);
    assert!(breaker.is_broken(&Dependency::Firehose, &Scope::Global), "the connection is down");
    for _ in 0..4 {
        fh.publish(stream, &s, FrameDraft::new("op"));
    }
    // the old subscription is gone (the connection dropped); the durable log kept the gap.

    // RECONNECT: resume(last_seq=3) → backfill (3, now] = {4,5,6,7}, then live.
    breaker.restore_dependency(Dependency::Firehose, Scope::Global);
    let resumed = fh.resume(stream, &s, last_seq).expect("an in-window resume backfills the gap");
    let backfilled: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(backfilled, vec![4, 5, 6, 7], "the gap (last_seq, now] is replayed — 0 ops lost");

    // a subsequent LIVE frame continues gap-free, no duplicate.
    fh.publish(stream, &s, FrameDraft::new("op"));
    let live: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(live, vec![8], "live continues contiguously — 0 duplicate across the boundary");

    // ZERO OPS LOST: across the whole reconnect the client saw 1..8, each exactly once.
    let mut total = seen;
    total.extend(backfilled);
    total.extend(live);
    assert_eq!(total, (1..=8).collect::<Vec<u64>>(), "every op delivered exactly once: 0 lost, 0 dup");

    // the seq-gap survival signal reads 0 after the reconnect (no op outstanding) → assert it GREEN
    // through the frozen §10.2 library. resync count is 0 (this leg never went out-of-window).
    let remaining_gap = (fh.head_seq(stream, &s) - resumed.last_seq()) as i64;
    let mut src = SignalSource::new();
    set_firehose_signals(&mut src, stream, &s.selector(), remaining_gap, 0);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Eq(0))
        .expect_green(); // no resync fired on the in-window reconnect leg
    src.assert_labelled(
        SignalName::FirehoseFrameLag,
        vec![Label::new("stream", stream), Label::new("scope", s.selector())],
        Predicate::Eq(0),
    )
    .expect_green(); // 0 ops lost: the seq-gap is closed after the backfill
}

/// **D-10 LEG 2 — an out-of-window `last_seq` yields `resync_required` (the resync path is correct,
/// NAMED not silent); the resync-count telemetry fires `>= 1`.** A SMALL retention window forces the
/// gap's head to be evicted, so the reconnect cannot backfill from the window → resync.
#[test]
fn d10_out_of_window_resume_raises_resync_required() {
    // a window holding only the most-recent 3 frames (the D-10 drill drives a SMALL window to force
    // the resync path deterministically; the production window is the NAMED floor → EB-30 measures it).
    let mut fh = Firehose::with_limits(3, DEFAULT_INFLIGHT_CAP);
    let stream = "chat-live";
    let s = scope("channel:town-hall"); // a hot channel (CHAT) — the OQ-J co-designed case.

    // the client last saw seq 2, then dropped. Meanwhile 3..8 are published — the window now holds
    // only {6,7,8}; ops 3,4,5 (and earlier) were evicted past the retention floor.
    let last_seq = 2u64;
    for _ in 0..8 {
        fh.publish(stream, &s, FrameDraft::new("msg"));
    }
    assert_eq!(fh.window_len(stream, &s), 3, "the retention window is bounded");

    // RECONNECT past the window → resync_required (NAMED, not a silent partial replay).
    let err = fh.resume(stream, &s, last_seq).expect_err("out-of-window resume cannot backfill");
    assert!(err.is_resync_required(), "an out-of-window cursor RAISES resync_required");
    let resync_fired = if let FirehoseError::ResyncRequired { window_floor, .. } = err {
        assert_eq!(window_floor, 6, "the window floor is the oldest held seq (6)");
        1
    } else {
        0
    };

    // the resync-count survival signal fired (>= 1) → assert GREEN through the frozen §10.2 library.
    // (The *.snapshot rebuild that follows is EB-22 / P-142 — proven cold==live by BUS-D5 there.)
    let mut src = SignalSource::new();
    set_firehose_signals(&mut src, stream, &s.selector(), 0, resync_fired);
    src.assert_signal(SignalName::ResyncRequiredCount, Predicate::Gte(1))
        .expect_green(); // the resync_required signal fired on the out-of-window reconnect (NAMED)
}

/// **D-10 LEG 3 — the transport REJECTS an over-broad scope (`scope = *`).** The whitelist-not-`*`
/// rule generalised: an unbounded subscription is rejected at `subscribe` (a `400`/close at the
/// connection tier), never admitted.
#[test]
fn d10_transport_rejects_an_over_broad_scope() {
    let mut fh = Firehose::new();
    // the headline fixture: scope = `*`.
    let err = fh
        .subscribe_raw("chat-live", "*", None)
        .expect_err("the transport rejects an over-broad scope");
    assert!(err.is_over_broad_scope(), "scope = * is rejected (BUS-3 generalised)");
    // a bounded scope through the same entry subscribes fine.
    assert!(
        fh.subscribe_raw("chat-live", "channel:eng", None).is_ok(),
        "a bounded scope subscribes"
    );
}

/// **D-10 ISS-D13 variant — a huge BOARD at `scope = board:<id>` drops mid-edit-storm; resume
/// backfill then live loses zero ops.** Proves the SAME transport property holds for the board case
/// (the OQ-J co-designed third surface), so all three (board/doc/channel) ride it identically.
#[test]
fn d10_board_scope_reconnect_loses_zero_ops() {
    let mut fh = Firehose::new();
    let stream = "issues";
    let s = scope("board:proj-42"); // a huge board (ISS) — the OQ-J co-designed case.

    let sub = fh.subscribe(stream, &s, None).expect("a board scope subscribes");
    for _ in 0..10 {
        fh.publish(stream, &s, FrameDraft::new("row-edit"));
    }
    let seen: Vec<u64> = sub.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(seen, (1..=10).collect::<Vec<u64>>());

    // mid-edit-storm drop; the storm keeps editing (11..20).
    for _ in 0..10 {
        fh.publish(stream, &s, FrameDraft::new("row-edit"));
    }
    let resumed = fh.resume(stream, &s, sub.last_seq()).expect("board resume backfills");
    let gap: Vec<u64> = resumed.drain_ready().iter().map(|f| f.seq).collect();
    assert_eq!(gap, (11..=20).collect::<Vec<u64>>(), "the whole edit-storm gap is replayed — 0 lost");

    let mut src = SignalSource::new();
    let remaining = (fh.head_seq(stream, &s) - resumed.last_seq()) as i64;
    set_firehose_signals(&mut src, stream, &s.selector(), remaining, 0);
    src.assert_labelled(
        SignalName::FirehoseFrameLag,
        vec![Label::new("stream", stream), Label::new("scope", s.selector())],
        Predicate::Eq(0),
    )
    .expect_green(); // the board's seq-gap is closed after the backfill — 0 ops lost
}
