// Overlay substrate core — the "get it right ONCE" mechanics (design-manual 02-components/overlays.md
// §0: the four substrate mandates). Every overlay in this package composes from THIS module; none
// re-implements focus-trap / scroll-lock / inert-background / the Escape stack. (overlays.md §10:
// "the single substrate is the only defence" against per-feature overlays breaking these.)
//
// Framework-agnostic DOM helpers live here; the Solid binding that wires them to component lifecycle
// is `createOverlay.ts`. Splitting them keeps this layer unit-testable and obviously single-source.

/** The focusable-element selector (WAI-ARIA APG focus management). */
const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
  '[contenteditable]:not([contenteditable="false"])',
].join(",");

/**
 * The focusable descendants of `container`, in DOM order. Under jsdom there is no layout, so we do
 * NOT filter by visibility (offsetParent is always null); we filter by the attributes that matter
 * (disabled / tabindex=-1 / aria-hidden / inert), which is what the trap actually needs.
 */
export function getFocusable(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) =>
      el.getAttribute("aria-hidden") !== "true" &&
      !el.closest("[inert]") &&
      el.tabIndex !== -1,
  );
}

/**
 * The focus trap (APG modal-dialog): Tab from the last focusable wraps to the first; Shift+Tab from
 * the first wraps to the last; focus that has somehow escaped the panel is pulled back in. Call from
 * a `keydown` handler when `e.key === "Tab"`.
 */
export function trapFocus(e: KeyboardEvent, container: HTMLElement): void {
  const focusable = getFocusable(container);
  if (focusable.length === 0) {
    // Nothing focusable inside — keep focus on the panel itself rather than letting it escape.
    e.preventDefault();
    container.focus();
    return;
  }
  const first = focusable[0]!;
  const last = focusable[focusable.length - 1]!;
  const active = document.activeElement as HTMLElement | null;
  if (e.shiftKey) {
    if (active === first || !container.contains(active)) {
      e.preventDefault();
      last.focus();
    }
  } else {
    if (active === last || !container.contains(active)) {
      e.preventDefault();
      first.focus();
    }
  }
}

// ---------------------------------------------------------------------------------------------------
// Body scroll-lock with scrollbar-width compensation (overlays.md §0.3: "scroll-lock with
// scrollbar-width compensation" — so the page doesn't jolt sideways when a modal opens). Refcounted
// so nested modals (Confirm-over-Dialog) lock once and restore exactly once.
// ---------------------------------------------------------------------------------------------------
let lockCount = 0;
let savedOverflow = "";
let savedPaddingRight = "";

export function lockScroll(): void {
  if (lockCount === 0) {
    const body = document.body;
    savedOverflow = body.style.overflow;
    savedPaddingRight = body.style.paddingRight;
    // Compensate for the scrollbar that overflow:hidden removes, so layout doesn't shift.
    const scrollbar = window.innerWidth - document.documentElement.clientWidth;
    if (scrollbar > 0) {
      const current = parseFloat(getComputedStyle(body).paddingRight) || 0;
      body.style.paddingRight = `${current + scrollbar}px`;
    }
    body.style.overflow = "hidden";
  }
  lockCount++;
}

export function unlockScroll(): void {
  lockCount = Math.max(0, lockCount - 1);
  if (lockCount === 0) {
    document.body.style.overflow = savedOverflow;
    document.body.style.paddingRight = savedPaddingRight;
  }
}

// ---------------------------------------------------------------------------------------------------
// Background inert (overlays.md §0.3 / §2: "Background is inert"). For a modal, every body child that
// does NOT contain the overlay content is marked `inert` — which removes it from the tab order AND
// the accessibility tree per the HTML spec, the modern single-attribute replacement for the
// aria-hidden dance. Refcounted/stacked: each modal records exactly what IT hid and restores only
// that, so nested modals unwind correctly.
// ---------------------------------------------------------------------------------------------------
export function hideOthers(contentEl: HTMLElement): () => void {
  const hidden: HTMLElement[] = [];
  for (const child of Array.from(document.body.children)) {
    if (!(child instanceof HTMLElement)) continue;
    if (child.contains(contentEl)) continue; // the portal root holding the overlay — keep live
    if (child.hasAttribute("inert")) continue; // already inert (an outer modal) — don't double-touch
    child.setAttribute("inert", "");
    child.setAttribute("data-overlay-inert", "");
    hidden.push(child);
  }
  return () => {
    for (const el of hidden) {
      el.removeAttribute("inert");
      el.removeAttribute("data-overlay-inert");
    }
  };
}

// ---------------------------------------------------------------------------------------------------
// The overlay stack (overlays.md §8: "Esc closes the top-most only"). Every open overlay registers
// here; an overlay acts on Escape / outside-pointer ONLY when it is the topmost entry, so a Confirm
// opened over a Dialog dismisses just the Confirm.
// ---------------------------------------------------------------------------------------------------
const stack: symbol[] = [];

export function pushOverlay(): symbol {
  const id = Symbol("overlay");
  stack.push(id);
  return id;
}

export function removeOverlay(id: symbol): void {
  const i = stack.indexOf(id);
  if (i !== -1) stack.splice(i, 1);
}

export function isTopmost(id: symbol): boolean {
  return stack.length > 0 && stack[stack.length - 1] === id;
}

/** Test-only: the live overlay depth (used to assert nested-stack ordering). */
export function overlayDepth(): number {
  return stack.length;
}
