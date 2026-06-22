//! The shared overlay/state primitives — the design-system bug-class FLOOR (P-S23; SUB-M0 item 5).
//!
//! CANON:
//!   - `external-insights/01-process-and-quality-doctrine.md` §4 (the off-screen-picker /
//!     clipped-dialog / unreachable-control bug-classes: "a modal that renders in the wrong place,
//!     a control unreachable on a phone, a picker that opens off-screen") and §7 (abstract at the
//!     THIRD copy — applied PRE-EMPTIVELY here because the doctrine names these EXACT recurring bugs,
//!     so the primitive is hoisted ONCE before any feature hand-rolls it a first time).
//!   - `planning/05-refined-shared-systems-architecture/testing-strategy/README.md` §5 (the shared
//!     overlay/state primitives, built BEFORE any feature consumes them so the off-screen-picker /
//!     clipped-dialog / focus-leak bug-classes are foreclosed at the design-system layer; an R-3
//!     Phase-6 sequencing prerequisite — a keystone, not a side-car).
//!   - `planning/06-roadmaps/shared/00-platform-substrate.md` §2 SUB-M0 item 5 (the shared
//!     overlay/state primitives built once so no feature hand-rolls them).
//!
//! ## What this module is (and what it deliberately is NOT yet)
//!
//! Myelin has **no chosen frontend stack** at M0 (the workspace is Rust services; there is no DOM,
//! no renderer, no widget toolkit). The P-S23 prompt anticipates exactly this: *"If the frontend
//! stack is not yet chosen, ship the primitive CONTRACT (the API + the bug-class invariants the
//! primitives must hold) and name the implementation as a floor tied to the first frontend-bearing
//! subsystem (M3+)."*
//!
//! So this module ships the **stack-agnostic CORE** of the overlay/state primitives: the pure
//! placement/containment/focus-trap LOGIC, expressed over a tiny geometry + focus model, with the
//! three bug-class invariants encoded as TESTABLE functions — not as prose a renderer might or might
//! not honour. The three doctrine bug-classes become three structural guarantees:
//!
//!   1. **off-screen-picker** → [`place_overlay`] returns a rect that is ALWAYS within the viewport
//!      (it flips to the opposite side, then clamps, before it would overflow). A picker that opens
//!      off-screen cannot be produced by this primitive.
//!   2. **clipped-dialog** → [`center_dialog`] / [`Rect::is_contained_by`] guarantee a dialog is
//!      fully inside the viewport (it shrinks-to-fit a viewport smaller than the dialog rather than
//!      clipping). A dialog clipped by the viewport edge cannot be produced by this primitive.
//!      [`reachable_within`] forecloses the §4 "control unreachable on a phone" sibling: an
//!      actionable control's hit-rect must be inside the same containing rect, never below the fold.
//!   3. **focus-leak** → [`FocusTrap`] is a closed cyclic state machine over a non-empty stop set;
//!      `next`/`prev` always return a member of the trap and `Tab` past the last (resp. `Shift+Tab`
//!      before the first) WRAPS, never escaping. Focus cannot leak out of an open overlay.
//!
//! The **rendering binding** (mapping these rects + this focus order onto the chosen frontend
//! toolkit's real layout/portal/focus APIs) is the **named floor**: it lands with the first
//! frontend-bearing subsystem (**M3+** — the Git-hosting / Knowledge design-system pass, e.g.
//! `GIT-P7`, the design-system pass + X-1 affordances). Every frontend feature from M3 on builds on
//! THESE primitives — it does not re-derive placement math, dialog containment, or focus order
//! (EI-01 §7: abstract once, here). The invariants below are the contract that binding must honour;
//! they are committed + tested NOW so the bug-classes are foreclosed before the first consumer.

/// A whole, non-negative pixel coordinate or extent. Frozen unit: CSS *logical pixels* (the unit a
/// frontend layout reasons in). `i64` (not `u64`) because an overlay's *desired* anchor can land at
/// a negative offset (e.g. a tooltip whose left edge would be off the left viewport edge) BEFORE the
/// placement primitive flips/clamps it back on-screen — the whole point is to accept an off-screen
/// DESIRE and return an on-screen RESULT.
pub type Px = i64;

/// An axis-aligned rectangle in logical pixels: top-left (`x`,`y`) + non-negative `w`×`h`. The one
/// geometry primitive the overlay/dialog/picker placement reasons over (a viewport, a dialog, a
/// picker, an anchor, a control hit-rect are all `Rect`s). Width/height are clamped to `>= 0` at
/// construction so a degenerate negative extent cannot silently invert containment math.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge (logical px). May be negative for a *desired* (pre-placement) rect.
    pub x: Px,
    /// Top edge (logical px). May be negative for a *desired* (pre-placement) rect.
    pub y: Px,
    /// Width (logical px), always `>= 0`.
    pub w: Px,
    /// Height (logical px), always `>= 0`.
    pub h: Px,
}

impl Rect {
    /// Construct a rect, clamping a negative width/height to `0` (a degenerate extent is a point,
    /// never an inverted rect — inverted rects make containment math lie).
    #[must_use]
    pub fn new(x: Px, y: Px, w: Px, h: Px) -> Self {
        Self {
            x,
            y,
            w: w.max(0),
            h: h.max(0),
        }
    }

    /// The right edge (`x + w`).
    #[must_use]
    pub fn right(&self) -> Px {
        self.x + self.w
    }

    /// The bottom edge (`y + h`).
    #[must_use]
    pub fn bottom(&self) -> Px {
        self.y + self.h
    }

    /// True iff `self` is FULLY inside `outer` on both axes (edges may touch). This is the
    /// clipped-dialog / off-screen-picker structural test: an overlay is on-screen iff
    /// `overlay.is_contained_by(viewport)`. The placement primitives below are specified to return
    /// a rect for which this holds whenever the viewport is at least as large as the rect.
    #[must_use]
    pub fn is_contained_by(&self, outer: &Rect) -> bool {
        self.x >= outer.x
            && self.y >= outer.y
            && self.right() <= outer.right()
            && self.bottom() <= outer.bottom()
    }

    /// True iff the two rects share any interior area (touching edges do NOT overlap).
    #[must_use]
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

/// The side of the anchor an overlay (popover/picker/menu) PREFERS to open on. The placement
/// primitive honours the preference when it fits and FLIPS to the opposite side when it would
/// overflow that edge — the core off-screen-picker foreclosure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Open below the anchor (the default for a dropdown/picker).
    Below,
    /// Open above the anchor.
    Above,
    /// Open to the right of the anchor.
    Right,
    /// Open to the left of the anchor.
    Left,
}

impl Side {
    /// The opposite side — the side the placement flips to when the preferred side overflows.
    #[must_use]
    pub fn flip(self) -> Side {
        match self {
            Side::Below => Side::Above,
            Side::Above => Side::Below,
            Side::Right => Side::Left,
            Side::Left => Side::Right,
        }
    }
}

/// The outcome of placing an overlay: where it ended up + WHY (so a frontend can render an arrow on
/// the resolved side, and so a test can assert the flip/clamp actually fired). The bug-class
/// guarantee is on [`Placement::rect`]: it is ALWAYS contained by the viewport when the viewport is
/// at least as large as the overlay (proven in tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The final on-screen rectangle. Guaranteed `rect.is_contained_by(viewport)` whenever the
    /// viewport can hold the overlay.
    pub rect: Rect,
    /// The side the overlay actually opened on (may differ from the preference if it flipped).
    pub side: Side,
    /// True iff the placement had to FLIP to the opposite side to avoid overflowing the preferred
    /// edge (telemetry/explainability — a frontend can surface "opened upward" and a test asserts it).
    pub flipped: bool,
    /// True iff, after choosing a side, the placement had to CLAMP the cross-axis (or the chosen
    /// axis on a tight viewport) to stay on-screen. A clamp means "we kept it on-screen by sliding
    /// it"; never a clip.
    pub clamped: bool,
}

/// Place an overlay of size `overlay` (its `x`/`y` are ignored; only `w`×`h` are used) relative to
/// the `anchor`, preferring `prefer`, constrained to be on-screen within `viewport`.
///
/// **The off-screen-picker foreclosure.** The algorithm:
///   1. Position the overlay on the `prefer` side of the anchor.
///   2. If it would overflow that viewport edge AND the opposite side has more room, FLIP.
///   3. CLAMP the result into the viewport on both axes (slide, never clip).
///
/// **Guarantee (tested):** if `overlay.w <= viewport.w && overlay.h <= viewport.h`, the returned
/// [`Placement::rect`] satisfies `rect.is_contained_by(viewport)` — a picker NEVER opens off-screen.
/// (If the overlay is larger than the viewport — an impossible-on-a-real-screen degenerate — it is
/// clamped top-left and `clamped` is set; containment is then best-effort, never a panic.)
#[must_use]
pub fn place_overlay(anchor: &Rect, overlay: &Rect, prefer: Side, viewport: &Rect) -> Placement {
    // Candidate top-left for a given side.
    let candidate = |side: Side| -> (Px, Px) {
        match side {
            Side::Below => (anchor.x, anchor.bottom()),
            Side::Above => (anchor.x, anchor.y - overlay.h),
            Side::Right => (anchor.right(), anchor.y),
            Side::Left => (anchor.x - overlay.w, anchor.y),
        }
    };

    // Does a side's candidate fit within the viewport on its primary axis (before cross-axis clamp)?
    let fits = |side: Side| -> bool {
        let (cx, cy) = candidate(side);
        match side {
            Side::Below => cy + overlay.h <= viewport.bottom(),
            Side::Above => cy >= viewport.y,
            Side::Right => cx + overlay.w <= viewport.right(),
            Side::Left => cx >= viewport.x,
        }
    };

    // 1+2: choose the side, flipping if the preferred side overflows and the opposite fits.
    let (side, flipped) = if fits(prefer) {
        (prefer, false)
    } else if fits(prefer.flip()) {
        (prefer.flip(), true)
    } else {
        // Neither side fits cleanly (a tight viewport): keep the preference and let the clamp
        // below slide it on-screen as best it can.
        (prefer, false)
    };

    let (mut x, mut y) = candidate(side);

    // 3: clamp both axes into the viewport (slide, never clip). Clamp the far edge first so a small
    // viewport pins the near edge.
    let clamp = |v: Px, extent: Px, lo: Px, hi: Px| -> (Px, bool) {
        let mut out = v;
        if out + extent > hi {
            out = hi - extent;
        }
        if out < lo {
            out = lo;
        }
        (out, out != v)
    };
    let (nx, cx_clamped) = clamp(x, overlay.w, viewport.x, viewport.right());
    let (ny, cy_clamped) = clamp(y, overlay.h, viewport.y, viewport.bottom());
    x = nx;
    y = ny;

    Placement {
        rect: Rect::new(x, y, overlay.w, overlay.h),
        side,
        flipped,
        clamped: cx_clamped || cy_clamped,
    }
}

/// Center a `dialog` (only its `w`×`h` are used) inside `viewport`, guaranteeing it is fully
/// CONTAINED — the clipped-dialog foreclosure. If the viewport is SMALLER than the dialog on an
/// axis, the dialog SHRINKS-to-fit that axis (a fit, never a clip) and is pinned to the viewport
/// origin on it. The result always satisfies `result.is_contained_by(viewport)`.
#[must_use]
pub fn center_dialog(dialog: &Rect, viewport: &Rect) -> Rect {
    let w = dialog.w.min(viewport.w);
    let h = dialog.h.min(viewport.h);
    let x = viewport.x + (viewport.w - w) / 2;
    let y = viewport.y + (viewport.h - h) / 2;
    Rect::new(x, y, w, h)
}

/// The "control unreachable on a phone" sibling foreclosure (§4): an actionable control is
/// reachable iff its hit-rect is fully contained by the scroll/containing rect it lives in (it is
/// not pushed below the fold / off the side). A design-system surface asserts this for every
/// primary action; here is the one shared predicate.
#[must_use]
pub fn reachable_within(control: &Rect, container: &Rect) -> bool {
    control.is_contained_by(container)
}

/// A focusable stop's stable identity within an overlay (a button, a field, a menu item). An opaque
/// index into the trap's ordered stop list — the frontend binding maps it to a real focusable node.
pub type FocusId = usize;

/// A direction a `Tab` / `Shift+Tab` moves focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusMove {
    /// `Tab` — advance to the next stop (wraps from last → first).
    Next,
    /// `Shift+Tab` — retreat to the previous stop (wraps from first → last).
    Prev,
}

/// The focus-trap state machine — the focus-leak foreclosure. A non-empty, ordered, CYCLIC ring of
/// focus stops with a current position. `Tab` past the last stop wraps to the first; `Shift+Tab`
/// before the first wraps to the last; focus can NEVER move to a stop outside the trap. While an
/// overlay is open, ALL keyboard focus motion goes through this primitive — so a frontend feature
/// cannot accidentally let focus escape into the page behind the overlay.
///
/// Constructing a trap requires at least one stop (an overlay with nothing focusable is a design
/// bug; [`FocusTrap::new`] returns `None` for an empty stop set rather than producing a leaky trap).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusTrap {
    /// The ordered focus ring (always non-empty by construction).
    stops: Vec<FocusId>,
    /// The current position within `stops` (always a valid index by construction).
    pos: usize,
}

impl FocusTrap {
    /// Build a trap over `stops` (tab order), initially focused on the first. Returns `None` if
    /// `stops` is empty — an empty trap would be a leak waiting to happen (focus would have nowhere
    /// to go and a naive renderer would let it escape), so it is structurally disallowed.
    #[must_use]
    pub fn new(stops: Vec<FocusId>) -> Option<Self> {
        if stops.is_empty() {
            return None;
        }
        Some(Self { stops, pos: 0 })
    }

    /// The currently focused stop (always a real member of the trap).
    #[must_use]
    pub fn current(&self) -> FocusId {
        self.stops[self.pos]
    }

    /// Move focus one step; ALWAYS returns a member of the trap, WRAPPING at the ends. This is the
    /// closed-cycle guarantee: there is no `Tab` sequence that exits the trap.
    pub fn step(&mut self, dir: FocusMove) -> FocusId {
        let n = self.stops.len();
        self.pos = match dir {
            FocusMove::Next => (self.pos + 1) % n,
            // +n keeps the operand non-negative before the modulo (usize-safe wrap).
            FocusMove::Prev => (self.pos + n - 1) % n,
        };
        self.current()
    }

    /// Focus a specific stop by identity (e.g. a click inside the overlay). Rejects (returns
    /// `false`, leaving focus unchanged) any id NOT in the trap — focus cannot be set to a stop
    /// outside the overlay, even programmatically.
    pub fn focus(&mut self, id: FocusId) -> bool {
        match self.stops.iter().position(|&s| s == id) {
            Some(i) => {
                self.pos = i;
                true
            }
            None => false,
        }
    }

    /// True iff `id` is a member of this trap (the containment predicate the focus-leak invariant
    /// test asserts over every reachable stop).
    #[must_use]
    pub fn contains(&self, id: FocusId) -> bool {
        self.stops.contains(&id)
    }

    /// The number of focus stops.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stops.len()
    }

    /// Always `false` by construction (a trap is non-empty); present so clippy's `len_without_is_empty`
    /// is satisfied and consumers can ask without special-casing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Rect {
        // A modest phone-ish viewport — the §4 "unreachable on a phone" case is the tight one.
        Rect::new(0, 0, 360, 640)
    }

    // ---- bug-class 1: a picker NEVER opens off-screen -----------------------------------------

    #[test]
    fn picker_below_anchor_fits_when_room() {
        let vp = viewport();
        let anchor = Rect::new(20, 20, 100, 24);
        let picker = Rect::new(0, 0, 200, 120);
        let p = place_overlay(&anchor, &picker, Side::Below, &vp);
        assert!(p.rect.is_contained_by(&vp));
        assert_eq!(p.side, Side::Below);
        assert!(!p.flipped);
    }

    #[test]
    fn picker_flips_above_when_no_room_below() {
        let vp = viewport();
        // Anchor near the bottom: a below-picker would overflow → must flip above.
        let anchor = Rect::new(20, 600, 100, 24);
        let picker = Rect::new(0, 0, 200, 120);
        let p = place_overlay(&anchor, &picker, Side::Below, &vp);
        assert!(
            p.rect.is_contained_by(&vp),
            "flipped picker must be on-screen: {:?}",
            p
        );
        assert_eq!(p.side, Side::Above);
        assert!(p.flipped);
    }

    #[test]
    fn picker_clamps_horizontally_rather_than_overflowing_right_edge() {
        let vp = viewport();
        // Anchor hard against the right edge: the below-picker fits vertically but would overflow
        // the right edge → it must clamp left, never clip.
        let anchor = Rect::new(350, 20, 8, 24);
        let picker = Rect::new(0, 0, 200, 120);
        let p = place_overlay(&anchor, &picker, Side::Below, &vp);
        assert!(p.rect.is_contained_by(&vp));
        assert!(p.clamped);
    }

    #[test]
    fn overlay_is_always_on_screen_for_every_side_and_anchor() {
        // Exhaustive-ish sweep: every side, anchors near every edge — the placement primitive must
        // produce an on-screen rect in EVERY case (the off-screen-picker class made impossible).
        let vp = viewport();
        let picker = Rect::new(0, 0, 180, 100);
        for side in [Side::Below, Side::Above, Side::Right, Side::Left] {
            for ax in [-50, 0, 100, 200, 350, 400] {
                for ay in [-50, 0, 100, 300, 600, 700] {
                    let anchor = Rect::new(ax, ay, 80, 24);
                    let p = place_overlay(&anchor, &picker, side, &vp);
                    assert!(
                        p.rect.is_contained_by(&vp),
                        "off-screen picker for side={:?} anchor=({ax},{ay}) -> {:?}",
                        side,
                        p,
                    );
                }
            }
        }
    }

    // ---- bug-class 2: a dialog is NEVER clipped (and a control is never below the fold) ---------

    #[test]
    fn centered_dialog_is_contained_and_centered() {
        let vp = viewport();
        let dialog = Rect::new(0, 0, 300, 400);
        let d = center_dialog(&dialog, &vp);
        assert!(d.is_contained_by(&vp));
        // centered: equal margins either side (within the integer-division rounding).
        assert_eq!(d.x, (360 - 300) / 2);
        assert_eq!(d.y, (640 - 400) / 2);
    }

    #[test]
    fn dialog_larger_than_viewport_shrinks_to_fit_never_clips() {
        let vp = Rect::new(0, 0, 200, 200);
        let dialog = Rect::new(0, 0, 500, 500); // bigger than the viewport on both axes
        let d = center_dialog(&dialog, &vp);
        assert!(
            d.is_contained_by(&vp),
            "a too-big dialog must fit, never clip: {:?}",
            d
        );
        assert_eq!(d.w, 200);
        assert_eq!(d.h, 200);
    }

    #[test]
    fn unreachable_control_is_detected() {
        let container = Rect::new(0, 0, 360, 640);
        let visible = Rect::new(10, 10, 100, 40);
        let below_the_fold = Rect::new(10, 700, 100, 40); // pushed past the bottom
        assert!(reachable_within(&visible, &container));
        assert!(!reachable_within(&below_the_fold, &container));
    }

    // ---- bug-class 3: focus is TRAPPED (never leaks out of the overlay) -------------------------

    #[test]
    fn empty_trap_is_disallowed() {
        assert!(FocusTrap::new(vec![]).is_none());
    }

    #[test]
    fn tab_cycles_forward_and_wraps() {
        let mut t = FocusTrap::new(vec![10, 20, 30]).unwrap();
        assert_eq!(t.current(), 10);
        assert_eq!(t.step(FocusMove::Next), 20);
        assert_eq!(t.step(FocusMove::Next), 30);
        assert_eq!(
            t.step(FocusMove::Next),
            10,
            "Tab past the last wraps to the first"
        );
    }

    #[test]
    fn shift_tab_cycles_backward_and_wraps() {
        let mut t = FocusTrap::new(vec![10, 20, 30]).unwrap();
        assert_eq!(
            t.step(FocusMove::Prev),
            30,
            "Shift+Tab before the first wraps to the last"
        );
        assert_eq!(t.step(FocusMove::Prev), 20);
        assert_eq!(t.step(FocusMove::Prev), 10);
    }

    #[test]
    fn focus_never_leaves_the_trap_under_any_key_sequence() {
        let stops = vec![1, 2, 3, 4];
        let mut t = FocusTrap::new(stops.clone()).unwrap();
        // Drive a long, mixed Tab/Shift+Tab sequence; focus must ALWAYS be a trap member.
        let seq = [
            FocusMove::Next,
            FocusMove::Next,
            FocusMove::Prev,
            FocusMove::Next,
            FocusMove::Next,
            FocusMove::Next,
            FocusMove::Prev,
            FocusMove::Prev,
            FocusMove::Prev,
        ];
        for m in seq {
            let now = t.step(m);
            assert!(t.contains(now), "focus leaked to {now}, not in {:?}", stops);
        }
    }

    #[test]
    fn programmatic_focus_outside_the_trap_is_rejected() {
        let mut t = FocusTrap::new(vec![1, 2, 3]).unwrap();
        assert!(t.focus(2));
        assert_eq!(t.current(), 2);
        assert!(!t.focus(99), "a stop outside the overlay cannot be focused");
        assert_eq!(t.current(), 2, "rejected focus must not move the cursor");
    }
}
