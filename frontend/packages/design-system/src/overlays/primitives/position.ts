// The ONE anchored-float positioner (overlays.md §9 / doc 10 §2: "a single helper clamps every
// caret/anchor-positioned float … one source of truth, never copy-pasted (and drifting)"). Popover,
// Menu and Tooltip all anchor through THIS function.
//
// BOUNDED — honest scope: this does basic vertical flip (place above the anchor when there isn't
// room below) and viewport clamping (pull the right edge in to keep a gutter; clamp left to the
// gutter). It does NOT do full collision-aware flipping on every axis, arrow/beak repositioning, or
// scroll/resize re-tracking — those are DEFERRED to the later positioning hardening (a Floating-UI-
// class helper) and are called out in the MR-017 report. It is sufficient and correct for the
// keyboard/a11y gate; production polish on tight viewports is the deferred follow-up.

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
