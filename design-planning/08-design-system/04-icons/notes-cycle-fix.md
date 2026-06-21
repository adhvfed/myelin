# cycle / rerun arrowhead fix

## The bug (before)
Both `cycle` and `rerun` used a 3-point open arrowhead path placed loosely near
the top gap of the ring. The chevron was not tangent to the arc terminus and the
barbs splayed the wrong way, so it read as a detached caret/flag and the head
appeared to point *against* the arc's sweep — i.e. it implied the reverse
rotation. `cycle` in particular "had the wrong rotation."

Old geometry (cycle): arc terminus `e` at (9,5); arrowhead a(5,6) b(9,5) c(10,9)
— a loose flag near the gap, not a tangent arrowhead.

## The fix (after)
Re-authored both as a clean MIRRORED pair, modelled on Lucide
`rotate-cw` / `rotate-ccw`:

- Ring is the standard r=8 circle centred at (12,12) with a gap at the top.
- The arc's drawing order defines the travel direction; the arrowhead is a
  2-segment chevron whose TIP sits exactly on the arc terminus and points along
  the forward tangent (computed: tip ± ~50° barbs, length ~4.3). This guarantees
  the head points in the same rotational direction the arc sweeps.

### rerun = CLOCKWISE
- Arc order a(14.74,4.48 top-right) → (20,12) → (12,20) → (4,12) → e(9.26,4.48
  top-left): right → bottom → left = clockwise.
- Arrowhead tip at e (top-left) pointing up-and-right (forward, into the gap).

### cycle = COUNTER-CLOCKWISE (the mirror)
- Arc order a(9.26,4.48 top-left) → (4,12) → (12,20) → (20,12) → e(14.74,4.48
  top-right): left → bottom → right = counter-clockwise. `bulge=left` (mirror of
  rerun's `bulge=right`) so the arc bows outward in the reversed travel order.
- Arrowhead tip at e (top-right) pointing up-and-left (forward).
- `@meaning` updated to note counter-clockwise direction.

## Verification
- Rendered at 256 / 24 / 16 px: arrowhead is a crisp chevron at every size and
  the rotational direction is unambiguous; the two are clearly opposite-handed
  and distinct from `commit` / `run`.
- `strok audit` on both: no suggestions.
- `bash build.sh`: svg/ uses currentColor, no hardcoded hex; preview + dist +
  contact sheet regenerated (42 icons).
