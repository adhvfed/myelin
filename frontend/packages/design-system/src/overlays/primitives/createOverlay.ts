// createOverlay — the shared Solid behaviour atom (overlays.md §0: "the focus/trap/return/scroll-lock/
// ARIA logic is written ONCE here and is the mechanical a11y guarantee for every transient/modal
// surface"). Dialog, ConfirmDialog, Popover and Menu all drive their open/close mechanics through
// this single function; it composes the framework-agnostic helpers in `overlay-core.ts` onto Solid's
// reactive lifecycle. Tooltip/Toast (which never trap or steal focus) reuse the Portal + tokens but
// deliberately not the trap — noted at each call site.

import { createEffect, onCleanup } from "solid-js";
import {
  trapFocus,
  lockScroll,
  unlockScroll,
  hideOthers,
  pushOverlay,
  removeOverlay,
  isTopmost,
} from "./overlay-core";

export interface CreateOverlayOptions {
  /** Reactive open state — the effect re-runs when this flips. */
  isOpen: () => boolean;
  /** Called when the overlay requests dismissal (Escape / outside-pointer). */
  onDismiss: () => void;
  /** The panel element (the focus/inside boundary). Read lazily — it mounts after open. */
  contentRef: () => HTMLElement | undefined;
  /** The trigger to restore focus to on close (APG return-focus). Falls back to the prior activeElement. */
  triggerRef?: () => HTMLElement | undefined;
  /** Modal: focus-trap + scroll-lock + inert background (Dialog/Confirm). Default false (Popover/Menu). */
  modal?: boolean;
  /** Escape dismisses (topmost only). Default true. Accessor allowed for reactive flags. */
  closeOnEscape?: boolean | (() => boolean);
  /** A pointer outside the panel dismisses (topmost only). Default true. Accessor allowed. */
  closeOnOutsidePointer?: boolean | (() => boolean);
  /** Move focus into the panel on open. Default true. Provide a getter to target a specific element
   *  (e.g. ConfirmDialog focuses the SAFE action; Menu focuses the first item). */
  autoFocus?: boolean | (() => HTMLElement | undefined);
  /** Return focus to the trigger on close. Default true. */
  restoreFocus?: boolean;
}

export function createOverlay(opts: CreateOverlayOptions): void {
  createEffect(() => {
    if (!opts.isOpen()) return;

    const panel = opts.contentRef();
    if (!panel) return;

    const resolveBool = (v: boolean | (() => boolean) | undefined, def: boolean): boolean =>
      v === undefined ? def : typeof v === "function" ? v() : v;

    const id = pushOverlay();
    const modal = opts.modal ?? false;
    const restoreFocus = opts.restoreFocus ?? true;
    // The reactive dismiss flags are read at EVENT time inside the handlers below — NOT snapshotted
    // here. Snapshotting subscribed this effect to e.g. Dialog's `() => dismissable`, so toggling it
    // mid-open (a real pattern: freeze dismiss during an in-flight confirm) tore down + re-ran the
    // whole setup — churning scroll-lock/inert and yanking initial focus back. Reading at event time
    // keeps the flag reactive without re-running the effect.

    // Record where focus came from BEFORE we move it, for the return-focus guarantee.
    const previouslyFocused = document.activeElement as HTMLElement | null;

    let unhide: (() => void) | undefined;
    if (modal) {
      lockScroll();
      unhide = hideOthers(panel);
    }

    // Initial focus (APG: focus moves in on open).
    const autoFocus = opts.autoFocus ?? true;
    if (autoFocus !== false) {
      const target =
        typeof autoFocus === "function"
          ? autoFocus()
          : panel.querySelector<HTMLElement>(
              "[data-autofocus]," +
                "button:not([disabled]),[href],input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex='-1'])",
            ) ?? panel;
      // Panel needs tabindex=-1 to be programmatically focusable as the fallback.
      if (target === panel && !panel.hasAttribute("tabindex")) panel.tabIndex = -1;
      target?.focus();
    }

    const onKeydown = (e: KeyboardEvent) => {
      if (!isTopmost(id)) return; // nested: only the top-most overlay reacts (overlays.md §8)
      if (e.key === "Escape" && resolveBool(opts.closeOnEscape, true)) {
        // stopPropagation so a modal opened from inside a panel doesn't also collapse that panel.
        e.stopPropagation();
        opts.onDismiss();
        return;
      }
      if (e.key === "Tab" && modal) {
        const current = opts.contentRef();
        if (current) trapFocus(e, current);
      }
    };

    const onPointerDown = (e: Event) => {
      if (!isTopmost(id) || !resolveBool(opts.closeOnOutsidePointer, true)) return;
      const current = opts.contentRef();
      const trigger = opts.triggerRef?.();
      const target = e.target as Node | null;
      if (!target) return;
      if (current?.contains(target)) return; // inside the panel — not a dismiss
      if (trigger?.contains(target)) return; // on the trigger — its own handler owns the toggle
      opts.onDismiss();
    };

    // Capture phase for keydown so the trap/Escape see the event before app handlers.
    document.addEventListener("keydown", onKeydown, true);
    document.addEventListener("pointerdown", onPointerDown, true);

    onCleanup(() => {
      document.removeEventListener("keydown", onKeydown, true);
      document.removeEventListener("pointerdown", onPointerDown, true);
      removeOverlay(id);
      if (modal) {
        unhide?.();
        unlockScroll();
      }
      // Return focus to the trigger (preferred) or wherever it was (APG return-focus).
      if (restoreFocus) {
        const trigger = opts.triggerRef?.();
        (trigger ?? previouslyFocused)?.focus();
      }
    });
  });
}
