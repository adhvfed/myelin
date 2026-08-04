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

#[test]
fn cdc_3_5_connection_tier_rejects_a_wildcard_subscription() {
    assert_eq!(BoundedSelector::parse("*"), Err(SelectorError::Wildcard));
    assert_eq!(
        BoundedSelector::parse("board:*"),
        Err(SelectorError::Wildcard)
    );
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
    assert!(matches!(
        BoundedSelector::parse("42"),
        Err(SelectorError::Unprefixed)
    ));
}

#[test]
fn cdc_3_5_a_50k_row_board_delivers_only_its_paginated_slice() {
    let sel = BoundedSelector::parse("board:huge").expect("a bounded board selector");
    let window = ScopeWindow::new(20_000, 100, 50);
    assert_eq!(
        window.delivered_span(),
        200,
        "the window bounds memory, not the 50k board"
    );
    let mut sel = FrameSelector::new("kn-ops", &sel, 8, 64, window);

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

#[test]
fn cdc_3_5_presence_frames_shed_before_message_delivery() {
    let sel = BoundedSelector::parse("channel:eng").unwrap();
    let mut sel = FrameSelector::new(
        "chat-live",
        &sel,
        8,
        10_000,
        ScopeWindow::new(0, 1, u64::MAX),
    );
    assert_eq!(sel.offer(presence(1), None), FrameOutcome::Buffered);
    assert_eq!(sel.offer(presence(2), None), FrameOutcome::Buffered);
    assert_eq!(sel.offer(presence(3), None), FrameOutcome::ShedByClass);
    assert_eq!(sel.budget().shed_count(FrameClass::Presence), 1);
    assert_eq!(
        sel.offer(human(4), None),
        FrameOutcome::Buffered,
        "message frames are shed last"
    );
    assert_eq!(sel.budget().shed_count(FrameClass::HumanDelivery), 0);
}

#[test]
fn cdc_3_5_bounded_selector_lowers_to_the_one_survival_signal_key() {
    let sel = BoundedSelector::parse("doc:abc").unwrap();
    let mut sel = FrameSelector::new("kn-ops", &sel, 4, 8, ScopeWindow::new(0, 1, u64::MAX));
    sel.offer(human(1), None);
    assert_eq!(sel.buffer().scope().selector(), "doc:abc");
    assert_eq!(sel.buffer().stream(), "kn-ops");
}
