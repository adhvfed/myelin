use crate::firehose::{FirehoseScope, Frame, FrameBuffer, FrameClass, PushOutcome};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoundedSelector {
    kind: SelectorKind,
    id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SelectorKind {
    Board,
    Doc,
    Channel,
    Inbox,
}

impl SelectorKind {
    pub fn prefix(self) -> &'static str {
        match self {
            SelectorKind::Board => "board",
            SelectorKind::Doc => "doc",
            SelectorKind::Channel => "channel",
            SelectorKind::Inbox => "inbox",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectorError {
    Wildcard,
    Unprefixed,
    UnknownKind(String),
    Empty,
}

impl core::fmt::Display for SelectorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SelectorError::Wildcard => write!(
                f,
                "firehose scope must be a bounded selector (board:/doc:/channel:), never `*` (§7.7)"
            ),
            SelectorError::Unprefixed => {
                write!(f, "firehose scope must name its kind: board:/doc:/channel:")
            }
            SelectorError::UnknownKind(p) => write!(f, "unknown firehose selector kind: `{p}:`"),
            SelectorError::Empty => write!(f, "firehose scope selector must not be empty"),
        }
    }
}

impl std::error::Error for SelectorError {}

impl BoundedSelector {
    pub fn parse(raw: &str) -> Result<BoundedSelector, SelectorError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(SelectorError::Empty);
        }
        if raw.contains('*') {
            return Err(SelectorError::Wildcard);
        }
        let Some((prefix, id)) = raw.split_once(':') else {
            return Err(SelectorError::Unprefixed);
        };
        if id.is_empty() {
            return Err(SelectorError::Empty);
        }
        let kind = match prefix {
            "board" => SelectorKind::Board,
            "doc" => SelectorKind::Doc,
            "channel" => SelectorKind::Channel,
            "inbox" => SelectorKind::Inbox,
            other => return Err(SelectorError::UnknownKind(other.to_string())),
        };
        Ok(BoundedSelector {
            kind,
            id: id.to_string(),
        })
    }

    pub fn kind(&self) -> SelectorKind {
        self.kind
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn as_str(&self) -> String {
        format!("{}:{}", self.kind.prefix(), self.id)
    }

    pub fn scope(&self) -> FirehoseScope {
        FirehoseScope(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeWindow {
    start: u64,
    len: u64,
    margin: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowVerdict {
    InWindow,
    OutOfWindow,
}

impl WindowVerdict {
    pub fn is_in_window(self) -> bool {
        matches!(self, WindowVerdict::InWindow)
    }
}

impl ScopeWindow {
    pub fn new(start: u64, len: u64, margin: u64) -> ScopeWindow {
        ScopeWindow {
            start,
            len: len.max(1),
            margin,
        }
    }

    pub fn lower(&self) -> u64 {
        self.start.saturating_sub(self.margin)
    }

    pub fn upper(&self) -> u64 {
        self.start
            .saturating_add(self.len)
            .saturating_add(self.margin)
    }

    pub fn delivered_span(&self) -> u64 {
        self.upper().saturating_sub(self.lower())
    }

    pub fn contains(&self, row: u64) -> bool {
        row >= self.lower() && row < self.upper()
    }

    pub fn verdict(&self, frame: &Frame, row: Option<u64>) -> WindowVerdict {
        let _ = frame;
        match row {
            Some(r) if !self.contains(r) => WindowVerdict::OutOfWindow,
            _ => WindowVerdict::InWindow,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FrameShedBudget {
    capacity: u32,
    class_ceiling: HashMap<FrameClass, u32>,
    class_in_flight: HashMap<FrameClass, u32>,
    class_shed: HashMap<FrameClass, u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameBudgetVerdict {
    WithinBudget,
    OverBudget,
}

impl FrameShedBudget {
    pub fn v1_floor(capacity: u32) -> FrameShedBudget {
        let cap = capacity.max(1);
        let presence = (cap / 4).max(1);
        let agent = (cap / 2).max(presence);
        let human = cap;
        let mut class_ceiling = HashMap::new();
        class_ceiling.insert(FrameClass::Presence, presence);
        class_ceiling.insert(FrameClass::AgentDelivery, agent);
        class_ceiling.insert(FrameClass::HumanDelivery, human);
        FrameShedBudget {
            capacity: cap,
            class_ceiling,
            class_in_flight: HashMap::new(),
            class_shed: HashMap::new(),
        }
    }

    pub fn consult(&mut self, class: FrameClass) -> FrameBudgetVerdict {
        let ceiling = self
            .class_ceiling
            .get(&class)
            .copied()
            .unwrap_or(self.capacity);
        let in_flight = self.class_in_flight.get(&class).copied().unwrap_or(0);
        if in_flight < ceiling {
            FrameBudgetVerdict::WithinBudget
        } else {
            *self.class_shed.entry(class).or_insert(0) += 1;
            FrameBudgetVerdict::OverBudget
        }
    }

    pub fn admitted(&mut self, class: FrameClass) {
        *self.class_in_flight.entry(class).or_insert(0) += 1;
    }

    pub fn delivered(&mut self, class: FrameClass) {
        if let Some(c) = self.class_in_flight.get_mut(&class) {
            *c = c.saturating_sub(1);
        }
    }

    pub fn release_all(&mut self) {
        self.class_in_flight.clear();
    }

    pub fn ceiling(&self, class: FrameClass) -> u32 {
        self.class_ceiling
            .get(&class)
            .copied()
            .unwrap_or(self.capacity)
    }

    pub fn in_flight(&self, class: FrameClass) -> u32 {
        self.class_in_flight.get(&class).copied().unwrap_or(0)
    }

    pub fn shed_count(&self, class: FrameClass) -> u64 {
        self.class_shed.get(&class).copied().unwrap_or(0)
    }

    pub fn total_shed_count(&self) -> u64 {
        self.class_shed.values().sum()
    }
}

#[derive(Clone, Debug)]
pub struct FrameSelector {
    buffer: FrameBuffer,
    window: ScopeWindow,
    budget: FrameShedBudget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    Buffered,
    OutOfWindow,
    ShedByClass,
    ShedOverCap,
    ResyncRequired,
}

impl FrameOutcome {
    pub fn is_buffered(self) -> bool {
        matches!(self, FrameOutcome::Buffered)
    }

    pub fn is_shed(self) -> bool {
        !matches!(self, FrameOutcome::Buffered)
    }
}

impl FrameSelector {
    pub fn new(
        stream: impl Into<String>,
        selector: &BoundedSelector,
        capacity: u32,
        lag_ceiling: u64,
        window: ScopeWindow,
    ) -> FrameSelector {
        FrameSelector {
            buffer: FrameBuffer::new(stream, selector.scope(), capacity, lag_ceiling),
            window,
            budget: FrameShedBudget::v1_floor(capacity),
        }
    }

    pub fn offer(&mut self, frame: Frame, row: Option<u64>) -> FrameOutcome {
        if self.window.verdict(&frame, row) == WindowVerdict::OutOfWindow {
            return FrameOutcome::OutOfWindow;
        }
        if self.budget.consult(frame.class) == FrameBudgetVerdict::OverBudget {
            return match self.buffer.note_shed_offer(frame) {
                PushOutcome::ResyncRequired => {
                    self.budget.release_all();
                    FrameOutcome::ResyncRequired
                }
                _ => FrameOutcome::ShedByClass,
            };
        }
        match self.buffer.offer(frame) {
            PushOutcome::Buffered => {
                self.budget.admitted(frame.class);
                FrameOutcome::Buffered
            }
            PushOutcome::Shed => FrameOutcome::ShedOverCap,
            PushOutcome::ResyncRequired => {
                self.budget.release_all();
                FrameOutcome::ResyncRequired
            }
        }
    }

    pub fn deliver(&mut self, frame: Frame) {
        self.buffer.deliver(frame);
        self.budget.delivered(frame.class);
    }

    pub fn buffer(&self) -> &FrameBuffer {
        &self.buffer
    }

    pub fn window(&self) -> &ScopeWindow {
        &self.window
    }

    pub fn budget(&self) -> &FrameShedBudget {
        &self.budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn presence(seq: u64) -> Frame {
        Frame::new(seq, FrameClass::Presence)
    }
    fn agent(seq: u64) -> Frame {
        Frame::new(seq, FrameClass::AgentDelivery)
    }
    fn human(seq: u64) -> Frame {
        Frame::new(seq, FrameClass::HumanDelivery)
    }

    #[test]
    fn a_wildcard_scope_is_rejected_bounded_selector_only() {
        assert_eq!(BoundedSelector::parse("*"), Err(SelectorError::Wildcard));
        assert_eq!(
            BoundedSelector::parse("board:*"),
            Err(SelectorError::Wildcard)
        );
        assert_eq!(
            BoundedSelector::parse("doc:a*"),
            Err(SelectorError::Wildcard)
        );
        assert_eq!(BoundedSelector::parse(""), Err(SelectorError::Empty));
        assert_eq!(BoundedSelector::parse("   "), Err(SelectorError::Empty));
        assert_eq!(
            BoundedSelector::parse("12345"),
            Err(SelectorError::Unprefixed)
        );
        assert_eq!(BoundedSelector::parse("board:"), Err(SelectorError::Empty));
        assert_eq!(
            BoundedSelector::parse("tenant:acme"),
            Err(SelectorError::UnknownKind("tenant".to_string()))
        );

        let b = BoundedSelector::parse("board:123").expect("a board selector is bounded");
        assert_eq!(b.kind(), SelectorKind::Board);
        assert_eq!(b.id(), "123");
        assert_eq!(b.as_str(), "board:123");
        assert_eq!(
            BoundedSelector::parse("doc:abc").unwrap().kind(),
            SelectorKind::Doc
        );
        assert_eq!(
            BoundedSelector::parse("channel:eng").unwrap().kind(),
            SelectorKind::Channel
        );
        assert_eq!(b.scope(), FirehoseScope("board:123".to_string()));
    }

    #[test]
    fn a_50k_row_board_delivers_only_its_paginated_slice() {
        let sel = BoundedSelector::parse("board:huge").unwrap();
        let window = ScopeWindow::new(10_000, 100, 50);
        assert_eq!(
            window.delivered_span(),
            200,
            "the window bounds memory, not the board size"
        );
        let mut sel = FrameSelector::new("kn-ops", &sel, 8, 32, window);

        assert_eq!(
            sel.offer(human(1), Some(10_050)),
            FrameOutcome::Buffered,
            "a frame in the visible window is delivered"
        );
        assert_eq!(sel.offer(human(2), Some(9_960)), FrameOutcome::Buffered);
        assert_eq!(
            sel.offer(human(3), Some(0)),
            FrameOutcome::OutOfWindow,
            "an off-screen board row is not delivered to this connection"
        );
        assert_eq!(sel.offer(human(4), Some(49_999)), FrameOutcome::OutOfWindow);
        assert_eq!(
            sel.offer(human(5), None),
            FrameOutcome::Buffered,
            "a whole-scope frame is delivered"
        );

        assert_eq!(
            sel.buffer().buffered_frames(),
            3,
            "only in-window frames consume buffer memory"
        );
    }

    #[test]
    fn scope_window_contains_is_the_half_open_range_with_margin() {
        let w = ScopeWindow::new(100, 10, 5);
        assert_eq!(w.lower(), 95);
        assert_eq!(w.upper(), 115);
        assert!(!w.contains(94));
        assert!(w.contains(95), "lower bound is inclusive");
        assert!(w.contains(114));
        assert!(!w.contains(115), "upper bound is exclusive");
        let w0 = ScopeWindow::new(2, 4, 10);
        assert_eq!(w0.lower(), 0, "the lower bound saturates at 0");
        assert!(w0.contains(0));
    }

    #[test]
    fn presence_frames_shed_before_message_frames_and_agents_before_humans() {
        let sel = BoundedSelector::parse("channel:eng").unwrap();
        let window = ScopeWindow::new(0, 1, 1_000_000);
        let mut sel = FrameSelector::new("chat-live", &sel, 8, 1_000, window);

        assert_eq!(sel.offer(presence(1), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(presence(2), None), FrameOutcome::Buffered);
        assert_eq!(
            sel.offer(presence(3), None),
            FrameOutcome::ShedByClass,
            "presence sheds at its budget (2), before message delivery - the buffer is not full"
        );
        assert_eq!(sel.budget().shed_count(FrameClass::Presence), 1);

        assert_eq!(sel.offer(agent(4), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(agent(5), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(agent(6), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(agent(7), None), FrameOutcome::Buffered);
        assert_eq!(
            sel.offer(agent(8), None),
            FrameOutcome::ShedByClass,
            "agent sheds at its budget (4), before human delivery"
        );
        assert_eq!(sel.budget().shed_count(FrameClass::AgentDelivery), 1);

        assert_eq!(sel.offer(human(9), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(human(10), None), FrameOutcome::Buffered);
        assert_eq!(
            sel.offer(human(11), None),
            FrameOutcome::ShedOverCap,
            "a human frame is shed only when the WHOLE buffer is full (true saturation, shed last)"
        );
        assert_eq!(
            sel.budget().shed_count(FrameClass::HumanDelivery),
            0,
            "humans shed last"
        );
    }

    #[test]
    fn frame_budget_v1_floor_orders_presence_le_agent_le_human() {
        let b = FrameShedBudget::v1_floor(16);
        let p = b.ceiling(FrameClass::Presence);
        let a = b.ceiling(FrameClass::AgentDelivery);
        let h = b.ceiling(FrameClass::HumanDelivery);
        assert!(
            p <= a,
            "presence budget ≤ agent budget (presence sheds first)"
        );
        assert!(
            a <= h,
            "agent budget ≤ human budget (agents shed before humans)"
        );
        assert_eq!(h, 16, "humans use the whole buffer (shed last)");
        assert!(
            p >= 1,
            "even a small buffer admits at least one presence frame before shedding"
        );
    }

    #[test]
    fn delivering_a_frame_frees_its_class_budget() {
        let sel = BoundedSelector::parse("channel:x").unwrap();
        let mut sel = FrameSelector::new(
            "chat-live",
            &sel,
            8,
            1_000,
            ScopeWindow::new(0, 1, u64::MAX),
        );
        sel.offer(presence(1), None);
        sel.offer(presence(2), None);
        assert_eq!(sel.offer(presence(3), None), FrameOutcome::ShedByClass);
        sel.deliver(presence(1));
        assert_eq!(
            sel.offer(presence(4), None),
            FrameOutcome::Buffered,
            "a delivered presence frame frees the class budget"
        );
    }

    #[test]
    fn the_slow_consumer_drop_still_fires_through_the_selector() {
        let sel = BoundedSelector::parse("doc:design").unwrap();
        let mut sel = FrameSelector::new("kn-ops", &sel, 4, 8, ScopeWindow::new(0, 1, u64::MAX));
        let mut dropped = false;
        for seq in 1..=8u64 {
            if sel.offer(human(seq), None) == FrameOutcome::ResyncRequired {
                dropped = true;
            }
        }
        assert!(
            dropped,
            "a stalled consumer is dropped to resync_required through the selector"
        );
        assert!(
            sel.buffer().resync_required(),
            "the connection is in the *.snapshot cold-rebuild path"
        );
        assert_eq!(
            sel.buffer().buffered_frames(),
            0,
            "the buffer is released (bounded memory)"
        );
        assert_eq!(
            sel.budget().in_flight(FrameClass::HumanDelivery),
            0,
            "class accounting released on drop"
        );
    }

    #[test]
    fn an_out_of_window_frame_costs_no_buffer_and_no_class_budget() {
        let sel = BoundedSelector::parse("board:huge").unwrap();
        let mut sel = FrameSelector::new("kn-ops", &sel, 4, 8, ScopeWindow::new(100, 10, 5));
        for seq in 1..=50u64 {
            assert_eq!(
                sel.offer(presence(seq), Some(1_000 + seq)),
                FrameOutcome::OutOfWindow
            );
        }
        assert_eq!(
            sel.buffer().buffered_frames(),
            0,
            "off-window frames never buffer"
        );
        assert_eq!(
            sel.budget().in_flight(FrameClass::Presence),
            0,
            "off-window frames cost no class budget"
        );
        assert_eq!(
            sel.budget().shed_count(FrameClass::Presence),
            0,
            "off-window is not a class shed"
        );
    }
}
