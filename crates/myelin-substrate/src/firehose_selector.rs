//! # The firehose scope-bounded selector + per-surface frame shed budgets (P-S29 → global P-136)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §7.7 (scope is a bounded selector, **never `*`** — a 50k-row board paginates its scope to the
//! visible window + margin; the firehose delivers only that slice's frames; the per-surface shed
//! budgets §7.6 apply to frames — presence/speculative frames shed before message delivery, agents
//! shed before humans), §7.6 (the per-surface shed-budget v1 floor table), §11 row D-11 (the firehose
//! bounded-queue half).
//!
//! **Contract-index:** row 3.5 (the firehose transport + resume-cursor protocol — `scope` a bounded
//! selector never `*`; `board:`/`doc:`/`channel:`). The protocol is **Bus-owned** (`subscribe`/`resume`,
//! the zero-loss-replay half, P-141/EB-21); THIS module is the substrate's **bounded-and-sheds** half.
//! Together with [`crate::firehose`] (P-S28 → P-135, the per-connection caps + slow-consumer drop) it
//! completes the substrate's half of D-11 (bounded-and-sheds-never-unbounded-memory).
//!
//! ## What this module is (the two slices P-S29 names — the complement of P-S28)
//! P-S28 ([`crate::firehose`]) shipped the per-connection frame cap + the slow-consumer drop to
//! `resync_required`, and LEFT two seams for this prompt: [`crate::firehose::FirehoseScope`] (the raw
//! selector — the `*`-rejection + paginated-window logic was named P-S29's half) and
//! [`crate::firehose::FrameClass`] (the shed priority order — the per-surface budget *selector* that
//! reads it was named P-S29's half). This module fills exactly those two seams:
//!
//! - **(a) scope as a bounded selector, never `*`** — [`BoundedSelector::parse`] admits ONLY
//!   `board:`/`doc:`/`channel:` selectors and **REJECTS `*`** (and every other un-prefixed/empty form)
//!   with a typed [`SelectorError`]. A 50k-row board does NOT subscribe to the whole board: it
//!   subscribes to a [`ScopeWindow`] (the visible row window + a prefetch margin), and the firehose
//!   delivers only the frames whose row index falls in that window ([`ScopeWindow::contains`]) — every
//!   other frame is **out-of-window** ([`WindowVerdict::OutOfWindow`]) and never enters the buffer
//!   (memory is bounded by the window size, not the board size — the §7.7 "never `*`" guarantee).
//! - **(b) the per-surface shed budgets applied to FRAMES** — [`FrameShedBudget`] gives each
//!   [`crate::firehose::FrameClass`] a fraction of the per-connection buffer (presence < agent <
//!   human, the §7.6 order). When the buffer is under pressure a frame is admitted only while its
//!   class's budget has room: **presence/speculative frames shed BEFORE message delivery; agents shed
//!   BEFORE humans** ([`FrameShedBudget::admit`]). [`FrameSelector`] composes the bounded-selector
//!   window + the per-surface frame budget over the P-S28 [`crate::firehose::FrameBuffer`] into the
//!   one call the connection tier (Chat M4) makes per inbound frame, [`FrameSelector::offer`].
//!
//! ## The shape of the composition (why a wrapper, not a re-impl)
//! Coherence (EI-01 §7): the per-connection cap + the slow-consumer drop + the `(stream,scope)`
//! frame-lag/`resync_required` survival signals are ALREADY built + proven in [`crate::firehose`].
//! This module does **not** re-define the buffer — it WRAPS it. [`FrameSelector::offer`] runs the two
//! P-S29 gates FIRST (out-of-window → drop the frame silently-but-counted before the buffer; over-class-
//! budget → shed by class, exporting the per-class shed count) and then defers to the existing
//! [`crate::firehose::FrameBuffer::offer`] (the cap + the slow-consumer drop, unchanged). The survival
//! signals stay the [`crate::firehose::FirehoseSignals`] set; this module adds the per-class
//! frame-shed-budget count (the §10.2 `ShedCount`-by-lane signal, labelled by frame class).
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - The Bus-side **zero-loss-replay half** (`subscribe`/`resume`/`resync_required → *.snapshot`) is
//!   **P-141 (EB-21)** — the full D-11 reconnect-loses-zero-ops proof needs BOTH halves. This module +
//!   P-S28 are the substrate bounded-and-sheds half, now COMPLETE.
//! - The real **connection tier** that opens a [`FrameSelector`] per subscription, derives the
//!   [`ScopeWindow`] from the client's visible viewport, and drives delivery is **Chat M4**; the M4
//!   connection-storm re-confirm of this backpressure half is **P-S31 (P-326)**. Here the selector is
//!   the in-process primitive; the SUB-D11 drill (extended) proves it against synthetic hot load.
//! - The per-class frame-budget FRACTIONS (the [`FrameShedBudget::v1_floor`] split) are the **M0/M2 v1
//!   floor** (the same named-floor posture as the §7.6 [`crate::shed::ShedBudgetTable`] numbers) →
//!   tuned by the connection-storm drill in **M5 (P-S33)**. The discipline (presence sheds first,
//!   humans last, every budget bounded) is the contract and is tested here.

use crate::firehose::{Frame, FrameBuffer, FrameClass, FirehoseScope, PushOutcome};
use std::collections::HashMap;

/// **A bounded scope selector — `board:`/`doc:`/`channel:`, NEVER `*` (§7.7, contract 3.5).**
///
/// The whitelist-not-`*` rule (BUS-3, generalised) applied to the firehose: a subscription names a
/// SINGLE bounded resource (`board:123`, `doc:abc`, `channel:eng`), never the whole tenant firehose.
/// A `*` (or empty, or un-prefixed, or unknown-prefix) selector is **structurally rejected** with a
/// typed [`SelectorError`] — there is no code path that admits an unbounded subscription, so "I
/// subscribed to everything" is impossible by construction.
///
/// The parsed selector is the bounded-scope half of the firehose key; it lowers to the existing
/// [`FirehoseScope`] (the `(stream,scope)` telemetry key the P-S28 buffer is keyed by) via
/// [`BoundedSelector::scope`], so the survival signals stay one set.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoundedSelector {
    kind: SelectorKind,
    /// The bounded resource id (the part after the prefix). A PII-free identifier (a board/doc/channel
    /// id), so a telemetry label built from the selector is `control-plane-pii-free` by construction.
    id: String,
}

/// The bounded selector kinds the firehose admits (§7.7 / contract 3.5: `board:`/`doc:`/`channel:`/
/// `inbox:`). There is deliberately **no `All`/`*` variant** — the type cannot represent an unbounded
/// subscription. `Inbox` (Notif's own-inbox slice, `notifications.md` §7 / C4) is the fourth bounded
/// kind, kept 1:1 with the Bus-protocol [`myelin_events`-side] `ScopeKind` so a scope validated at the
/// Bus seam lowers to the SAME selector key at this connection-tier seam (EI-01 §7 coherence).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SelectorKind {
    /// A board (Issues huge-board) — paginated to the visible window + margin (the 50k-row case).
    Board,
    /// A document (Knowledge hot-doc) — the collab op-stream + presence on one doc.
    Doc,
    /// A channel (Chat hot-channel) — live delivery + presence on one channel.
    Channel,
    /// One principal's inbox slice (Notif `inbox watch`, §7 / C4) — a BOUNDED selector
    /// `inbox:<principal>`; never `*` (a client gets only its own inbox's frames).
    Inbox,
}

impl SelectorKind {
    /// The selector prefix (`board` / `doc` / `channel` / `inbox`) — the wire form before the `:`.
    pub fn prefix(self) -> &'static str {
        match self {
            SelectorKind::Board => "board",
            SelectorKind::Doc => "doc",
            SelectorKind::Channel => "channel",
            SelectorKind::Inbox => "inbox",
        }
    }
}

/// **Why a firehose subscription was rejected (the typed `*`-rejection, §7.7).** Every variant is a
/// LOUD typed value the connection tier maps to a `400`/close — never a silent admit of an unbounded
/// subscription.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectorError {
    /// The selector was `*` (or contained a `*`) — an UNBOUNDED subscription, which the firehose never
    /// admits (one client may not subscribe to the whole tenant firehose, §7.7). The headline rejection.
    Wildcard,
    /// The selector had no `prefix:` (e.g. `123` with no `board:`/`doc:`/`channel:`) — ambiguous and
    /// therefore rejected (a bounded selector MUST name its kind).
    Unprefixed,
    /// The prefix was not one of `board:`/`doc:`/`channel:` (an unknown selector kind).
    UnknownKind(String),
    /// The selector was empty, or its resource id (the part after the prefix) was empty.
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
    /// **Parse a bounded selector — the `*`-rejection chokepoint (§7.7, contract 3.5).** Admits ONLY
    /// `board:<id>` / `doc:<id>` / `channel:<id>` (a non-empty id); REJECTS `*`, any `*`-containing
    /// form, the empty string, an un-prefixed bare id, and an unknown prefix. There is no other way to
    /// construct a [`BoundedSelector`], so an unbounded subscription is unrepresentable.
    pub fn parse(raw: &str) -> Result<BoundedSelector, SelectorError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(SelectorError::Empty);
        }
        // the headline rule: `*` (or any `*`-containing selector — `board:*`, `*`, `doc:a*`) is an
        // unbounded subscription and is rejected. Checked FIRST so `board:*` reads as Wildcard, not a
        // valid board id of "*".
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
        Ok(BoundedSelector { kind, id: id.to_string() })
    }

    /// The selector kind (`board`/`doc`/`channel`).
    pub fn kind(&self) -> SelectorKind {
        self.kind
    }

    /// The bounded resource id (the part after the prefix) — a PII-free identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The canonical wire/telemetry selector string (`board:123`). Round-trips through [`Self::parse`].
    pub fn as_str(&self) -> String {
        format!("{}:{}", self.kind.prefix(), self.id)
    }

    /// Lower to the existing [`FirehoseScope`] (the `(stream,scope)` survival-signal key the P-S28
    /// [`FrameBuffer`] is keyed by) — so a bounded-selector subscription's frame-lag/`resync` signals
    /// stay the ONE [`crate::firehose::FirehoseSignals`] set (no parallel telemetry).
    pub fn scope(&self) -> FirehoseScope {
        FirehoseScope(self.as_str())
    }
}

/// **A scope WINDOW — the paginated slice of a bounded selector the firehose delivers (§7.7).**
///
/// A 50k-row board never subscribes to all 50k rows: it subscribes to the **visible window + a
/// prefetch margin** (the rows the client can actually see plus a small look-ahead). The firehose
/// delivers only the frames whose row index falls in `[start - margin, start + len + margin)`; every
/// other frame is [`WindowVerdict::OutOfWindow`] and never enters the per-connection buffer — so the
/// buffered memory is bounded by the WINDOW size, not the board size. This is what makes "scope a
/// bounded selector" bound *memory*, not just *naming*: a huge board cannot flood one connection.
///
/// A frame whose [`Frame`] carries no row index (a whole-scope frame — e.g. a channel-level presence
/// summary) is always in-window (it is not row-addressed); the window only filters row-addressed frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeWindow {
    /// The first visible row (0-based).
    start: u64,
    /// The number of visible rows (the viewport height).
    len: u64,
    /// The prefetch margin (look-ahead/behind rows the client may scroll into before re-subscribing).
    margin: u64,
}

/// Whether a frame's row falls in the delivered window (§7.7 paginated slice).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowVerdict {
    /// The frame's row is in `[start - margin, start + len + margin)` (or the frame is not
    /// row-addressed) — it is delivered into the per-connection buffer.
    InWindow,
    /// The frame's row is outside the window — it is NOT delivered (the firehose delivers only the
    /// slice; a 50k-row board's off-screen rows never enter this connection's buffer). Memory bounded.
    OutOfWindow,
}

impl WindowVerdict {
    /// `true` iff the frame is in the delivered window.
    pub fn is_in_window(self) -> bool {
        matches!(self, WindowVerdict::InWindow)
    }
}

impl ScopeWindow {
    /// A window of `len` visible rows starting at `start`, with a `margin` of prefetch rows on each
    /// side. A `len` of 0 is raised to 1 (a window always contains at least its first row).
    pub fn new(start: u64, len: u64, margin: u64) -> ScopeWindow {
        ScopeWindow { start, len: len.max(1), margin }
    }

    /// The inclusive lower bound of the delivered row range (`start - margin`, saturating at 0).
    pub fn lower(&self) -> u64 {
        self.start.saturating_sub(self.margin)
    }

    /// The exclusive upper bound of the delivered row range (`start + len + margin`).
    pub fn upper(&self) -> u64 {
        self.start.saturating_add(self.len).saturating_add(self.margin)
    }

    /// The total number of rows the window delivers (`upper - lower`) — the BOUND on this connection's
    /// row-addressed in-flight, independent of the board size. The §7.7 "paginates its scope" guarantee.
    pub fn delivered_span(&self) -> u64 {
        self.upper().saturating_sub(self.lower())
    }

    /// Whether a `row` falls in the delivered window (`[lower, upper)`).
    pub fn contains(&self, row: u64) -> bool {
        row >= self.lower() && row < self.upper()
    }

    /// The window verdict for a frame: a row-addressed frame is filtered by the window; a frame with
    /// no row (a whole-scope frame) is always in-window (it is not paginated away).
    pub fn verdict(&self, frame: &Frame, row: Option<u64>) -> WindowVerdict {
        let _ = frame; // the verdict reads the row, not the frame payload (PII stays out).
        match row {
            Some(r) if !self.contains(r) => WindowVerdict::OutOfWindow,
            _ => WindowVerdict::InWindow,
        }
    }
}

/// **The per-surface frame shed budget — the §7.6 shed order applied to FRAMES (P-S29 (b)).**
///
/// The §7.6 discipline (presence/speculative shed before message delivery; agents shed before humans)
/// applied to the per-connection frame buffer: each [`FrameClass`] gets a fraction of the buffer
/// `capacity`, and a frame is admitted only while its class's budget has room. Because the fractions
/// are ordered presence ≤ agent ≤ human, under pressure the LOWER classes hit their budget FIRST — so
/// presence sheds before agent sheds before human (the human/message frames are protected, shed last).
///
/// This is the frame-level mirror of [`crate::shed::ShedLane`] (the request-level human-lane shed
/// order): the same doctrine, the same order, applied to firehose frames instead of HTTP requests.
#[derive(Clone, Debug)]
pub struct FrameShedBudget {
    /// The per-connection buffer capacity (the §7.1 bound — the same cap the [`FrameBuffer`] holds).
    capacity: u32,
    /// Per-class ceilings (the max in-flight frames of each class). Ordered presence ≤ agent ≤ human,
    /// so lower classes shed first. The human ceiling equals the capacity (humans use the whole buffer,
    /// shed last — only in true saturation).
    class_ceiling: HashMap<FrameClass, u32>,
    /// Per-class in-flight (admitted, not yet delivered) — the accounting the budget reads.
    class_in_flight: HashMap<FrameClass, u32>,
    /// Per-class cumulative frame-shed count (the §10.2 `ShedCount`-by-lane signal, labelled by class).
    class_shed: HashMap<FrameClass, u64>,
}

/// The verdict of consulting the per-surface frame shed budget for one frame (P-S29 (b)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameBudgetVerdict {
    /// The frame's class has budget — it may proceed to the per-connection buffer.
    WithinBudget,
    /// The frame's class is at/over its shed budget — it sheds (a lower class sheds before a higher
    /// one). The connection survives; only the budget-exceeding class's frames are dropped.
    OverBudget,
}

impl FrameShedBudget {
    /// **The §7.6 v1 FLOOR split (named floors, tuned by the connection-storm drill in M5/P-S33).**
    /// Presence gets the smallest slice (it is ephemeral, shed first), agent a larger slice, human the
    /// whole buffer (shed last). The fractions are the M0/M2 floor — the ORDER (presence ≤ agent ≤
    /// human) is the contract and is tested; the exact fractions are tuned by the drill.
    pub fn v1_floor(capacity: u32) -> FrameShedBudget {
        let cap = capacity.max(1);
        // presence ≤ agent ≤ human == cap. Conservative round fractions; each ≥ 1 so even a tiny
        // buffer admits at least one frame of each class before shedding it.
        let presence = (cap / 4).max(1);
        let agent = (cap / 2).max(presence);
        let human = cap; // humans use the whole buffer — shed last, only in true saturation.
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

    /// **Consult the budget for one frame (the §7.6 frame-level shed order).** A frame of `class` is
    /// [`FrameBudgetVerdict::WithinBudget`] only while that class's in-flight is below its ceiling;
    /// otherwise it is [`FrameBudgetVerdict::OverBudget`] (shed, counted). Does NOT take a slot — the
    /// caller takes the per-connection buffer slot on a buffered outcome and calls [`Self::admitted`].
    pub fn consult(&mut self, class: FrameClass) -> FrameBudgetVerdict {
        let ceiling = self.class_ceiling.get(&class).copied().unwrap_or(self.capacity);
        let in_flight = self.class_in_flight.get(&class).copied().unwrap_or(0);
        if in_flight < ceiling {
            FrameBudgetVerdict::WithinBudget
        } else {
            *self.class_shed.entry(class).or_insert(0) += 1;
            FrameBudgetVerdict::OverBudget
        }
    }

    /// Record that a frame of `class` was admitted into the buffer (take a class slot).
    pub fn admitted(&mut self, class: FrameClass) {
        *self.class_in_flight.entry(class).or_insert(0) += 1;
    }

    /// Record that a frame of `class` was delivered (release a class slot). Saturating at 0.
    pub fn delivered(&mut self, class: FrameClass) {
        if let Some(c) = self.class_in_flight.get_mut(&class) {
            *c = c.saturating_sub(1);
        }
    }

    /// Reset all per-class in-flight to 0 (called when the connection drops to `resync_required` — the
    /// buffer is released, so the per-class accounting is released too; the shed counts are kept).
    pub fn release_all(&mut self) {
        self.class_in_flight.clear();
    }

    /// The per-class ceiling (the §7.6 budget fraction for `class`).
    pub fn ceiling(&self, class: FrameClass) -> u32 {
        self.class_ceiling.get(&class).copied().unwrap_or(self.capacity)
    }

    /// The current per-class in-flight.
    pub fn in_flight(&self, class: FrameClass) -> u32 {
        self.class_in_flight.get(&class).copied().unwrap_or(0)
    }

    /// The cumulative per-class frame-shed count (the §10.2 `ShedCount`-by-lane producer signal,
    /// labelled by the frame class's [`FrameClass::label`]).
    pub fn shed_count(&self, class: FrameClass) -> u64 {
        self.class_shed.get(&class).copied().unwrap_or(0)
    }

    /// The total frame-shed count across all classes (the per-surface frame-budget signal).
    pub fn total_shed_count(&self) -> u64 {
        self.class_shed.values().sum()
    }
}

/// **The composed firehose frame selector — the connection tier's one call per inbound frame (P-S29).**
///
/// Wraps the P-S28 [`FrameBuffer`] (the per-connection cap + slow-consumer drop, unchanged) with the
/// two P-S29 gates, applied in order:
/// 1. **scope window** (§7.7) — a row-addressed frame outside the [`ScopeWindow`] is
///    [`FrameOutcome::OutOfWindow`] and never touches the buffer (a 50k-row board's off-screen rows
///    never enter this connection — memory bounded by the window, not the board);
/// 2. **per-surface frame budget** (§7.6) — a frame whose class is over its [`FrameShedBudget`] is
///    [`FrameOutcome::ShedByClass`] (presence sheds before agent before human);
/// 3. the existing [`FrameBuffer::offer`] (the cap + the slow-consumer drop) — mapped through to
///    [`FrameOutcome`].
///
/// This is the §7.7+§7.6 substrate half made one chokepoint: the connection tier (Chat M4) opens a
/// `FrameSelector` per subscription and calls [`Self::offer`] per inbound frame.
#[derive(Clone, Debug)]
pub struct FrameSelector {
    buffer: FrameBuffer,
    window: ScopeWindow,
    budget: FrameShedBudget,
}

/// The outcome of offering a frame to a [`FrameSelector`] — the union of the two P-S29 gates and the
/// P-S28 buffer outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    /// The frame was buffered (in-window, within its class budget, a buffer slot was free).
    Buffered,
    /// The frame's row is outside the [`ScopeWindow`] — not delivered (the paginated-slice guarantee).
    /// Counted but harmless: an off-screen board row is simply not this connection's concern.
    OutOfWindow,
    /// The frame's class is over its per-surface [`FrameShedBudget`] — shed by class (presence before
    /// agent before human). The §7.6 frame-level shed order.
    ShedByClass,
    /// The per-connection buffer was at its cap — shed in the firehose's own bounded queue (§7.7 (a),
    /// the P-S28 over-cap shed).
    ShedOverCap,
    /// The consumer is too slow — dropped to `resync_required` (§7.7 (b), the P-S28 slow-consumer drop).
    /// Falls back to a `*.snapshot` replay (NAMED).
    ResyncRequired,
}

impl FrameOutcome {
    /// `true` iff the frame was buffered (delivered into the bounded buffer).
    pub fn is_buffered(self) -> bool {
        matches!(self, FrameOutcome::Buffered)
    }

    /// `true` iff the frame was shed or dropped (NOT buffered) by any of the gates.
    pub fn is_shed(self) -> bool {
        !matches!(self, FrameOutcome::Buffered)
    }
}

impl FrameSelector {
    /// Open a frame selector on a bounded `selector` (its [`BoundedSelector::scope`] is the buffer's
    /// `(stream,scope)` key) with a per-connection `capacity`, a slow-consumer `lag_ceiling`, the
    /// delivered `window`, and the §7.6 v1-floor frame budget for the capacity.
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

    /// **Offer a frame at an optional `row` (the connection tier's per-frame call).** The gate order is
    /// chosen so the §7.6 frame budget sheds the right classes first WITHOUT ever masking the P-S28
    /// slow-consumer drop (a consumer that is structurally too slow must always be detected, EI-01 §3
    /// named-not-silent):
    /// 1. **window (§7.7)** — an off-window row never enters the connection (the paginated slice);
    /// 2. **the §7.6 per-surface frame budget** — if the frame's class is over its budget it sheds BY
    ///    CLASS (presence before agent before human; humans use the whole buffer so they never class-
    ///    shed). A class shed does NOT take a buffer slot, but it STILL advances the lag via
    ///    [`FrameBuffer::note_shed_offer`] — so a class-shedding-but-stalled consumer is still detected
    ///    and dropped to `resync_required` (the slow-consumer drop is never masked by a class shed).
    /// 3. **the P-S28 cap + slow-consumer drop** — a within-budget frame is offered to the buffer (the
    ///    per-connection cap sheds over-cap; the slow-consumer ceiling drops a stalled consumer).
    ///
    /// A whole-scope frame passes `row = None`.
    pub fn offer(&mut self, frame: Frame, row: Option<u64>) -> FrameOutcome {
        // (1) §7.7 paginated slice: an off-window row is not this connection's concern — drop it BEFORE
        // the buffer (the board may be 50k rows; only the window's frames ever enter the buffer).
        if self.window.verdict(&frame, row) == WindowVerdict::OutOfWindow {
            return FrameOutcome::OutOfWindow;
        }
        // (2) §7.6 per-surface frame budget: presence sheds before agent sheds before human. A class
        // shed still NOTES the offer on the buffer (keeps the lag honest → the slow-consumer drop still
        // fires for a stalled consumer that is also class-shedding).
        if self.budget.consult(frame.class) == FrameBudgetVerdict::OverBudget {
            return match self.buffer.note_shed_offer(frame) {
                PushOutcome::ResyncRequired => {
                    self.budget.release_all();
                    FrameOutcome::ResyncRequired
                }
                // Shed (or the unreachable Buffered) → the frame was class-shed, lag noted.
                _ => FrameOutcome::ShedByClass,
            };
        }
        // (3) the P-S28 per-connection cap + slow-consumer drop (unchanged).
        match self.buffer.offer(frame) {
            PushOutcome::Buffered => {
                self.budget.admitted(frame.class);
                FrameOutcome::Buffered
            }
            PushOutcome::Shed => FrameOutcome::ShedOverCap,
            PushOutcome::ResyncRequired => {
                // the buffer released itself; release the per-class accounting too (kept: shed counts).
                self.budget.release_all();
                FrameOutcome::ResyncRequired
            }
        }
    }

    /// Deliver one buffered frame to the consumer (advance the P-S28 buffer + release the class slot).
    pub fn deliver(&mut self, frame: Frame) {
        self.buffer.deliver(frame);
        self.budget.delivered(frame.class);
    }

    /// The wrapped per-connection buffer (the P-S28 cap + slow-consumer drop + the survival signals).
    pub fn buffer(&self) -> &FrameBuffer {
        &self.buffer
    }

    /// The delivered scope window (§7.7).
    pub fn window(&self) -> &ScopeWindow {
        &self.window
    }

    /// The per-surface frame shed budget (§7.6) — the per-class ceilings + shed counts.
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

    // ---- (a) scope as a bounded selector, never `*` ----------------------------------------------

    /// **A `*` scope is REJECTED (bounded selector only) — the headline §7.7 guarantee.** `*`,
    /// `board:*`, an empty selector, an un-prefixed bare id, and an unknown prefix are all rejected;
    /// only `board:`/`doc:`/`channel:<id>` parse.
    #[test]
    fn a_wildcard_scope_is_rejected_bounded_selector_only() {
        // the headline: `*` is an unbounded subscription → rejected.
        assert_eq!(BoundedSelector::parse("*"), Err(SelectorError::Wildcard));
        // a `*`-containing selector (board:*) is also unbounded → rejected (not a board id of "*").
        assert_eq!(BoundedSelector::parse("board:*"), Err(SelectorError::Wildcard));
        assert_eq!(BoundedSelector::parse("doc:a*"), Err(SelectorError::Wildcard));
        // empty / un-prefixed / unknown-kind are rejected too.
        assert_eq!(BoundedSelector::parse(""), Err(SelectorError::Empty));
        assert_eq!(BoundedSelector::parse("   "), Err(SelectorError::Empty));
        assert_eq!(BoundedSelector::parse("12345"), Err(SelectorError::Unprefixed));
        assert_eq!(BoundedSelector::parse("board:"), Err(SelectorError::Empty));
        assert_eq!(
            BoundedSelector::parse("tenant:acme"),
            Err(SelectorError::UnknownKind("tenant".to_string()))
        );

        // ONLY board:/doc:/channel:<id> parse — the three bounded kinds.
        let b = BoundedSelector::parse("board:123").expect("a board selector is bounded");
        assert_eq!(b.kind(), SelectorKind::Board);
        assert_eq!(b.id(), "123");
        assert_eq!(b.as_str(), "board:123");
        assert_eq!(BoundedSelector::parse("doc:abc").unwrap().kind(), SelectorKind::Doc);
        assert_eq!(BoundedSelector::parse("channel:eng").unwrap().kind(), SelectorKind::Channel);
        // it lowers to the FirehoseScope survival-signal key (one telemetry set).
        assert_eq!(b.scope(), FirehoseScope("board:123".to_string()));
    }

    /// **A 50k-row board delivers ONLY its paginated slice's frames (§7.7).** The window is the visible
    /// rows + a margin; a frame on an off-screen row is OutOfWindow and never enters the buffer — so the
    /// connection's memory is bounded by the window (200-odd rows), not the 50 000-row board.
    #[test]
    fn a_50k_row_board_delivers_only_its_paginated_slice() {
        let sel = BoundedSelector::parse("board:huge").unwrap();
        // visible rows 10_000..10_100 (a 100-row viewport) + a 50-row margin each side.
        let window = ScopeWindow::new(10_000, 100, 50);
        // the delivered span is bounded (200 rows) — independent of the 50_000-row board.
        assert_eq!(window.delivered_span(), 200, "the window bounds memory, not the board size");
        let mut sel = FrameSelector::new("kn-ops", &sel, 8, 32, window);

        // a frame on a visible row (10_050) is delivered.
        assert_eq!(
            sel.offer(human(1), Some(10_050)),
            FrameOutcome::Buffered,
            "a frame in the visible window is delivered"
        );
        // a frame just inside the margin (9_960, within [9_950, 10_150)) is delivered.
        assert_eq!(sel.offer(human(2), Some(9_960)), FrameOutcome::Buffered);
        // a frame WAY off-screen (row 0 of a 50k board, and row 49_999) is NOT delivered — it never
        // enters the buffer (the §7.7 paginated-slice guarantee; memory bounded by the window).
        assert_eq!(
            sel.offer(human(3), Some(0)),
            FrameOutcome::OutOfWindow,
            "an off-screen board row is not delivered to this connection"
        );
        assert_eq!(sel.offer(human(4), Some(49_999)), FrameOutcome::OutOfWindow);
        // a whole-scope frame (no row — e.g. a board-level summary) is always delivered.
        assert_eq!(sel.offer(human(5), None), FrameOutcome::Buffered, "a whole-scope frame is delivered");

        // exactly the in-window frames entered the buffer (3 buffered: rows 10_050, 9_960, and None).
        assert_eq!(sel.buffer().buffered_frames(), 3, "only in-window frames consume buffer memory");
    }

    #[test]
    fn scope_window_contains_is_the_half_open_range_with_margin() {
        let w = ScopeWindow::new(100, 10, 5); // delivers [95, 115)
        assert_eq!(w.lower(), 95);
        assert_eq!(w.upper(), 115);
        assert!(!w.contains(94));
        assert!(w.contains(95), "lower bound is inclusive");
        assert!(w.contains(114));
        assert!(!w.contains(115), "upper bound is exclusive");
        // a window near 0 saturates the lower bound (no underflow).
        let w0 = ScopeWindow::new(2, 4, 10);
        assert_eq!(w0.lower(), 0, "the lower bound saturates at 0");
        assert!(w0.contains(0));
    }

    // ---- (b) the per-surface frame shed budgets: presence sheds before agent before human ---------

    /// **Presence/speculative frames shed BEFORE message delivery; agents shed before humans (§7.6).**
    /// The frame-level shed order: under a buffer of capacity 8 the presence budget (2) fills first, so
    /// presence frames shed while agent + human frames still have budget; the agent budget (4) fills
    /// next; human frames (whole buffer) are shed LAST.
    #[test]
    fn presence_frames_shed_before_message_frames_and_agents_before_humans() {
        let sel = BoundedSelector::parse("channel:eng").unwrap();
        // cap 8 → v1 floor: presence 2, agent 4, human 8. A big window so the window never filters.
        let window = ScopeWindow::new(0, 1, 1_000_000);
        let mut sel = FrameSelector::new("chat-live", &sel, 8, 1_000, window);

        // PRESENCE sheds first: the presence budget is 2 — the 3rd presence frame sheds by class while
        // the buffer is nowhere near its cap (only 2 frames in flight).
        assert_eq!(sel.offer(presence(1), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(presence(2), None), FrameOutcome::Buffered);
        assert_eq!(
            sel.offer(presence(3), None),
            FrameOutcome::ShedByClass,
            "presence sheds at its budget (2), before message delivery — the buffer is not full"
        );
        assert_eq!(sel.budget().shed_count(FrameClass::Presence), 1);

        // AGENT delivery still has budget (4): two agent frames buffer (in-flight now 2 presence + 2
        // agent = 4 of 8), the agent budget is reached, the 3rd agent frame sheds — while HUMAN frames
        // still buffer (agents shed before humans, §7.6).
        assert_eq!(sel.offer(agent(4), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(agent(5), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(agent(6), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(agent(7), None), FrameOutcome::Buffered); // agent in-flight now 4 == ceiling
        assert_eq!(
            sel.offer(agent(8), None),
            FrameOutcome::ShedByClass,
            "agent sheds at its budget (4), before human delivery"
        );
        assert_eq!(sel.budget().shed_count(FrameClass::AgentDelivery), 1);

        // HUMAN delivery is shed LAST: humans use the whole buffer. In-flight is 2 presence + 4 agent =
        // 6 of 8 → 2 human frames buffer (filling the cap), then the buffer's per-connection cap (not
        // the human class budget) sheds the next — humans shed only in true saturation.
        assert_eq!(sel.offer(human(9), None), FrameOutcome::Buffered);
        assert_eq!(sel.offer(human(10), None), FrameOutcome::Buffered); // buffer now full (8/8)
        assert_eq!(
            sel.offer(human(11), None),
            FrameOutcome::ShedOverCap,
            "a human frame is shed only when the WHOLE buffer is full (true saturation, shed last)"
        );
        // the human class itself never hit a class-budget shed (it is shed LAST, by the cap not the class).
        assert_eq!(sel.budget().shed_count(FrameClass::HumanDelivery), 0, "humans shed last");
    }

    #[test]
    fn frame_budget_v1_floor_orders_presence_le_agent_le_human() {
        let b = FrameShedBudget::v1_floor(16);
        let p = b.ceiling(FrameClass::Presence);
        let a = b.ceiling(FrameClass::AgentDelivery);
        let h = b.ceiling(FrameClass::HumanDelivery);
        assert!(p <= a, "presence budget ≤ agent budget (presence sheds first)");
        assert!(a <= h, "agent budget ≤ human budget (agents shed before humans)");
        assert_eq!(h, 16, "humans use the whole buffer (shed last)");
        assert!(p >= 1, "even a small buffer admits at least one presence frame before shedding");
    }

    #[test]
    fn delivering_a_frame_frees_its_class_budget() {
        let sel = BoundedSelector::parse("channel:x").unwrap();
        let mut sel = FrameSelector::new("chat-live", &sel, 8, 1_000, ScopeWindow::new(0, 1, u64::MAX));
        // fill the presence budget (2) and shed the 3rd.
        sel.offer(presence(1), None);
        sel.offer(presence(2), None);
        assert_eq!(sel.offer(presence(3), None), FrameOutcome::ShedByClass);
        // delivering a presence frame frees its class budget → a new presence frame buffers again.
        sel.deliver(presence(1));
        assert_eq!(
            sel.offer(presence(4), None),
            FrameOutcome::Buffered,
            "a delivered presence frame frees the class budget"
        );
    }

    // ---- composition: the two P-S29 gates compose with the P-S28 cap + slow-consumer drop ---------

    /// The slow-consumer drop (P-S28) still fires THROUGH the selector: a stalled consumer is dropped to
    /// `resync_required` and the per-class accounting is released (the buffer is released). The two P-S29
    /// gates do not break the P-S28 slow-consumer guarantee.
    #[test]
    fn the_slow_consumer_drop_still_fires_through_the_selector() {
        let sel = BoundedSelector::parse("doc:design").unwrap();
        // cap 4, slow-consumer ceiling 8, a big window. Human frames (whole-buffer budget) so the class
        // budget never sheds before the cap — isolating the slow-consumer drop.
        let mut sel = FrameSelector::new("kn-ops", &sel, 4, 8, ScopeWindow::new(0, 1, u64::MAX));
        let mut dropped = false;
        for seq in 1..=8u64 {
            if sel.offer(human(seq), None) == FrameOutcome::ResyncRequired {
                dropped = true;
            }
        }
        assert!(dropped, "a stalled consumer is dropped to resync_required through the selector");
        assert!(sel.buffer().resync_required(), "the connection is in the *.snapshot cold-rebuild path");
        assert_eq!(sel.buffer().buffered_frames(), 0, "the buffer is released (bounded memory)");
        // the per-class accounting was released on the drop (no stale class in-flight).
        assert_eq!(sel.budget().in_flight(FrameClass::HumanDelivery), 0, "class accounting released on drop");
    }

    /// An out-of-window frame never reaches the buffer OR the class budget — it is filtered first, so a
    /// 50k-row board's off-screen frames cost neither buffer memory nor class budget.
    #[test]
    fn an_out_of_window_frame_costs_no_buffer_and_no_class_budget() {
        let sel = BoundedSelector::parse("board:huge").unwrap();
        let mut sel = FrameSelector::new("kn-ops", &sel, 4, 8, ScopeWindow::new(100, 10, 5)); // [95,115)
        // 50 off-screen presence frames: all OutOfWindow, none touch the buffer or the presence budget.
        for seq in 1..=50u64 {
            assert_eq!(sel.offer(presence(seq), Some(1_000 + seq)), FrameOutcome::OutOfWindow);
        }
        assert_eq!(sel.buffer().buffered_frames(), 0, "off-window frames never buffer");
        assert_eq!(sel.budget().in_flight(FrameClass::Presence), 0, "off-window frames cost no class budget");
        assert_eq!(sel.budget().shed_count(FrameClass::Presence), 0, "off-window is not a class shed");
    }
}
