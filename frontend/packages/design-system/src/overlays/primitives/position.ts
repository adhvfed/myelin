// Shared anchor positioner for popovers, menus, and tooltips. It flips vertically and clamps to the
// viewport, but does not handle horizontal flipping, arrows, or scroll/resize tracking.

export type Placement = "bottom-start" | "bottom-end" | "top-start" | "top-end";

const GUTTER = 8; // px viewport gutter

export interface PositionResult {
  left: number;
  top: number;
  maxBlockSize: number;
  placement: Placement;
}

export function computePosition(
  anchor: HTMLElement,
  floating: HTMLElement,
  preferred: Placement = "bottom-start",
): PositionResult {
  const a = anchor.getBoundingClientRect();
  const f = floating.getBoundingClientRect();
  const vw = window.innerWidth || document.documentElement.clientWidth;
  const vh = window.innerHeight || document.documentElement.clientHeight;

  const wantTop = preferred.startsWith("top");
  const wantEnd = preferred.endsWith("end");

  const spaceBelow = vh - a.bottom - GUTTER;
  const spaceAbove = a.top - GUTTER;
  // Flip vertically only if the preferred side can't fit and the other side has more room.
  let placeTop = wantTop;
  if (!wantTop && f.height > spaceBelow && spaceAbove > spaceBelow) placeTop = true;
  if (wantTop && f.height > spaceAbove && spaceBelow > spaceAbove) placeTop = false;

  const top = placeTop ? a.top - f.height : a.bottom;
  const maxBlockSize = (placeTop ? spaceAbove : spaceBelow) || f.height;

  // Horizontal: align to the start or end edge of the anchor, then clamp into the viewport gutter.
  let left = wantEnd ? a.right - f.width : a.left;
  const maxLeft = vw - f.width - GUTTER;
  if (left > maxLeft) left = maxLeft; // pull the right edge in
  if (left < GUTTER) left = GUTTER; // collapse left on a too-narrow viewport

  const placement = `${placeTop ? "top" : "bottom"}-${wantEnd ? "end" : "start"}` as Placement;
  return { left, top: Math.max(GUTTER, top), maxBlockSize, placement };
}
