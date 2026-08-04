pub type Px = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: Px,
    pub y: Px,
    pub w: Px,
    pub h: Px,
}

impl Rect {
    #[must_use]
    pub fn new(x: Px, y: Px, w: Px, h: Px) -> Self {
        Self {
            x,
            y,
            w: w.max(0),
            h: h.max(0),
        }
    }

    #[must_use]
    pub fn right(&self) -> Px {
        self.x + self.w
    }

    #[must_use]
    pub fn bottom(&self) -> Px {
        self.y + self.h
    }

    #[must_use]
    pub fn is_contained_by(&self, outer: &Rect) -> bool {
        self.x >= outer.x
            && self.y >= outer.y
            && self.right() <= outer.right()
            && self.bottom() <= outer.bottom()
    }

    #[must_use]
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Below,
    Above,
    Right,
    Left,
}

impl Side {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub rect: Rect,
    pub side: Side,
    pub flipped: bool,
    pub clamped: bool,
}

#[must_use]
pub fn place_overlay(anchor: &Rect, overlay: &Rect, prefer: Side, viewport: &Rect) -> Placement {
    let candidate = |side: Side| -> (Px, Px) {
        match side {
            Side::Below => (anchor.x, anchor.bottom()),
            Side::Above => (anchor.x, anchor.y - overlay.h),
            Side::Right => (anchor.right(), anchor.y),
            Side::Left => (anchor.x - overlay.w, anchor.y),
        }
    };

    let fits = |side: Side| -> bool {
        let (cx, cy) = candidate(side);
        match side {
            Side::Below => cy + overlay.h <= viewport.bottom(),
            Side::Above => cy >= viewport.y,
            Side::Right => cx + overlay.w <= viewport.right(),
            Side::Left => cx >= viewport.x,
        }
    };

    let (side, flipped) = if fits(prefer) {
        (prefer, false)
    } else if fits(prefer.flip()) {
        (prefer.flip(), true)
    } else {
        (prefer, false)
    };

    let (mut x, mut y) = candidate(side);

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

#[must_use]
pub fn center_dialog(dialog: &Rect, viewport: &Rect) -> Rect {
    let w = dialog.w.min(viewport.w);
    let h = dialog.h.min(viewport.h);
    let x = viewport.x + (viewport.w - w) / 2;
    let y = viewport.y + (viewport.h - h) / 2;
    Rect::new(x, y, w, h)
}

#[must_use]
pub fn reachable_within(control: &Rect, container: &Rect) -> bool {
    control.is_contained_by(container)
}

pub type FocusId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusMove {
    Next,
    Prev,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusTrap {
    stops: Vec<FocusId>,
    pos: usize,
}

impl FocusTrap {
    #[must_use]
    pub fn new(stops: Vec<FocusId>) -> Option<Self> {
        if stops.is_empty() {
            return None;
        }
        Some(Self { stops, pos: 0 })
    }

    #[must_use]
    pub fn current(&self) -> FocusId {
        self.stops[self.pos]
    }

    pub fn step(&mut self, dir: FocusMove) -> FocusId {
        let n = self.stops.len();
        self.pos = match dir {
            FocusMove::Next => (self.pos + 1) % n,
            FocusMove::Prev => (self.pos + n - 1) % n,
        };
        self.current()
    }

    pub fn focus(&mut self, id: FocusId) -> bool {
        match self.stops.iter().position(|&s| s == id) {
            Some(i) => {
                self.pos = i;
                true
            }
            None => false,
        }
    }

    #[must_use]
    pub fn contains(&self, id: FocusId) -> bool {
        self.stops.contains(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.stops.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Rect {
        Rect::new(0, 0, 360, 640)
    }

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
        let anchor = Rect::new(350, 20, 8, 24);
        let picker = Rect::new(0, 0, 200, 120);
        let p = place_overlay(&anchor, &picker, Side::Below, &vp);
        assert!(p.rect.is_contained_by(&vp));
        assert!(p.clamped);
    }

    #[test]
    fn overlay_is_always_on_screen_for_every_side_and_anchor() {
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

    #[test]
    fn centered_dialog_is_contained_and_centered() {
        let vp = viewport();
        let dialog = Rect::new(0, 0, 300, 400);
        let d = center_dialog(&dialog, &vp);
        assert!(d.is_contained_by(&vp));
        assert_eq!(d.x, (360 - 300) / 2);
        assert_eq!(d.y, (640 - 400) / 2);
    }

    #[test]
    fn dialog_larger_than_viewport_shrinks_to_fit_never_clips() {
        let vp = Rect::new(0, 0, 200, 200);
        let dialog = Rect::new(0, 0, 500, 500);
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
        let below_the_fold = Rect::new(10, 700, 100, 40);
        assert!(reachable_within(&visible, &container));
        assert!(!reachable_within(&below_the_fold, &container));
    }

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
