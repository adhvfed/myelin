//! # CDC 3.5 (substrate half) — the firehose scope-bounded selector + frame shed budgets (P-S29 → P-136)
//!
//! **Contract-index:** row 3.5 (`Firehose transport + the resume-cursor subscription protocol` — `scope`
//! a bounded selector never `*`; `board:`/`doc:`/`channel:`). The protocol is **Bus-owned**
//! (`subscribe`/`resume` + the zero-loss-replay half, P-141/EB-21); THIS consumer-driven contract test
//! exercises the **substrate's scope-bounded-selector + frame-shed-budget slice** from OUTSIDE the crate
//! — the consumer is the firehose transport / connection tier (Chat M4) that parses a client's
//! subscription scope, opens a per-connection [`FrameSelector`] on a paginated window, and offers frames
//! to it. It pins the two halves the Bus and the substrate agree on at the seam (§7.7 / §7.6):
//!
//! - **(a) scope a bounded selector, never `*`** — the connection tier rejects a `*` subscription and
//!   admits only `board:`/`doc:`/`channel:<id>`; a 50k-row board delivers only its paginated slice.
//! - **(b) the per-surface frame shed budgets** — presence/speculative frames shed before message
//!   delivery; agents shed before humans (the §7.6 order applied to frames).
//!
//! The provider side is [`myelin_substrate::firehose_selector`] (the `BoundedSelector` + `ScopeWindow` +
//! `FrameShedBudget` + `FrameSelector`). This is the consumer (the transport parsing scopes + offering
//! frames). Together with `cdc_3_5_firehose_backpressure.rs` (the P-S28 cap + slow-consumer-drop half)
//! it is the dated green artifact's CDC half for the COMPLETE substrate slice of 3.5. The
//! **zero-loss-replay** half of D-11 (zero ops lost across a reconnect) needs the Bus impl — **P-141**,
//! named, not asserted here.

use myelin_substrate::{
    BoundedSelector, Frame, FrameClass, FrameOutcome, FrameSelector, ScopeWindow, SelectorError,
    SelectorKind,
};

fn presence(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::Presence)
}
fn human(seq: u64) -> Frame {
    Frame::new(seq, FrameClass::HumanDelivery)
}

/// **CDC 3.5 (a) — the connection tier rejects a `*` subscription (bounded selector only).** The
/// transport and the substrate agree: a firehose `subscribe` names a SINGLE bounded resource; `*` (or a
/// `*`-containing, empty, un-prefixed, or unknown-kind selector) is rejected — one client cannot
/// subscribe to the whole tenant firehose (§7.7, the whitelist-not-`*` rule generalised).
#[test]
fn cdc_3_5_connection_tier_rejects_a_wildcard_subscription() {
    // the headline rejection: `*` is unbounded.
    assert_eq!(BoundedSelector::parse("*"), Err(SelectorError::Wildcard));
    assert_eq!(
        BoundedSelector::parse("board:*"),
        Err(SelectorError::Wildcard)
    );
    // only the three bounded kinds are admitted.
    assert_eq!(
        BoundedSelector::parse("board:42").unwrap().kind(),
        SelectorKind::Board
    );
    assert_eq!(
        BoundedSelector::parse("doc:design").unwrap().kind(),
        SelectorKind::Doc
    );
    assert_eq!(
        BoundedSelector::parse("channel:eng").unwrap().kind(),
        SelectorKind::Channel
    );
    // a bare id (no kind) is ambiguous → rejected.
    assert!(matches!(
        BoundedSelector::parse("42"),
        Err(SelectorError::Unprefixed)
    ));
}

/// **CDC 3.5 (a) — a 50k-row board delivers only its paginated slice's frames (§7.7).** The transport
/// opens a per-connection selector on the visible window + margin; an off-screen board row never enters
/// the buffer — so memory is bounded by the window, not the board.
#[test]
fn cdc_3_5_a_50k_row_board_delivers_only_its_paginated_slice() {
    let sel = BoundedSelector::parse("board:huge").expect("a bounded board selector");
    let window = ScopeWindow::new(20_000, 100, 50); // delivers rows [19_950, 20_150)
    assert_eq!(
        window.delivered_span(),
        200,
        "the window bounds memory, not the 50k board"
    );
    let mut sel = FrameSelector::new("kn-ops", &sel, 8, 64, window);

    // an in-window frame is delivered; an off-screen one is OutOfWindow (never buffers).
    assert_eq!(sel.offer(human(1), Some(20_050)), FrameOutcome::Buffered);
    assert_eq!(
        sel.offer(human(2), Some(0)),
        FrameOutcome::OutOfWindow,
        "off-screen board row not delivered"
    );
    assert_eq!(sel.offer(human(3), Some(49_999)), FrameOutcome::OutOfWindow);
    assert_eq!(
        sel.buffer().buffered_frames(),
        1,
        "only the in-window frame consumes buffer memory"
    );
}

/// **CDC 3.5 (b) — presence/speculative frames shed before message delivery (§7.6).** The transport and
/// the substrate agree on the frame-level shed order: under pressure the presence budget fills first, so
/// presence frames shed while message (human) frames still have budget.
#[test]
fn cdc_3_5_presence_frames_shed_before_message_delivery() {
    let sel = BoundedSelector::parse("channel:eng").unwrap();
    // cap 8 → presence budget 2; a wide window so the window never filters; a high lag ceiling so only
    // the class budget (not the slow-consumer drop) fires.
    let mut sel = FrameSelector::new(
        "chat-live",
        &sel,
        8,
        10_000,
        ScopeWindow::new(0, 1, u64::MAX),
    );
    assert_eq!(sel.offer(presence(1), None), FrameOutcome::Buffered);
    assert_eq!(sel.offer(presence(2), None), FrameOutcome::Buffered);
    // the presence budget (2) is reached → the 3rd presence frame sheds BY CLASS, though the buffer is
    // nowhere near full (presence/speculative shed before message delivery).
    assert_eq!(sel.offer(presence(3), None), FrameOutcome::ShedByClass);
    assert_eq!(sel.budget().shed_count(FrameClass::Presence), 1);
    // a human (message) frame still buffers — message delivery is shed last.
    assert_eq!(
        sel.offer(human(4), None),
        FrameOutcome::Buffered,
        "message frames are shed last"
    );
    assert_eq!(sel.budget().shed_count(FrameClass::HumanDelivery), 0);
}

/// **CDC 3.5 — the bounded selector lowers to the existing `(stream,scope)` survival-signal key.** The
/// transport's scope parses to a `BoundedSelector` whose `.scope()` IS the [`firehose::FirehoseScope`]
/// the P-S28 buffer is keyed by — so the frame-lag / `resync_required` signals stay ONE set (no parallel
/// telemetry for the P-S29 slice).
#[test]
fn cdc_3_5_bounded_selector_lowers_to_the_one_survival_signal_key() {
    let sel = BoundedSelector::parse("doc:abc").unwrap();
    let mut sel = FrameSelector::new("kn-ops", &sel, 4, 8, ScopeWindow::new(0, 1, u64::MAX));
    sel.offer(human(1), None);
    // the buffer carries the lowered scope key (`doc:abc`) — the same FirehoseScope P-S28 signals on.
    assert_eq!(sel.buffer().scope().selector(), "doc:abc");
    assert_eq!(sel.buffer().stream(), "kn-ops");
}
