// Tooltip — the tiny anchored label for icon-buttons and truncated text (overlays.md §6). Shows on
// hover AND keyboard focus; NEVER takes or steals focus (overlays.md §6: "Tooltip never takes
// focus"). role="tooltip" + the trigger's aria-describedby points at it. Dismissable (Escape),
// hoverable + persistent (WCAG 2.2 1.4.13). Anchored through the shared `computePosition`.
//
// The trigger is supplied via a render prop so we can wire aria-describedby + the hover/focus
// handlers ONTO the real interactive element (the gate asserts describedby lands on the trigger).

import {
  Show,
  createSignal,
  createEffect,
  createUniqueId,
  onCleanup,
  type JSX,
} from "solid-js";
import { OverlayPortal } from "./primitives/OverlayPortal";
import { computePosition, type Placement } from "./primitives/position";

export interface TooltipTriggerProps {
  "aria-describedby": string | undefined;
  onPointerEnter: JSX.EventHandler<HTMLElement, PointerEvent>;
  onPointerLeave: JSX.EventHandler<HTMLElement, PointerEvent>;
  onFocus: JSX.EventHandler<HTMLElement, FocusEvent>;
  onBlur: JSX.EventHandler<HTMLElement, FocusEvent>;
  onKeyDown: JSX.EventHandler<HTMLElement, KeyboardEvent>;
}

export interface TooltipProps {
  /** The label text. */
  text: string;
  placement?: Placement;
  /** Render the trigger; spread the given props onto the interactive element. */
  trigger: (props: TooltipTriggerProps) => JSX.Element;
}

export function Tooltip(props: TooltipProps): JSX.Element {
  const [open, setOpen] = createSignal(false);
  const [pos, setPos] = createSignal({ left: 0, top: 0, maxBlockSize: 0 });
  let anchor: HTMLElement | undefined;
  let tip: HTMLDivElement | undefined;
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  const id = createUniqueId();

  const cancelHide = () => {
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = undefined;
  };
  // Persistent: a brief pointer-leave does not dismiss (1.4.13).
  const scheduleHide = () => {
    cancelHide();
    hideTimer = setTimeout(() => setOpen(false), 120);
  };
  onCleanup(cancelHide);

  const show = (el: HTMLElement) => {
    anchor = el;
    cancelHide();
    setOpen(true);
  };

  createEffect(() => {
    if (!open() || !anchor || !tip) return;
    const p = computePosition(anchor, tip, props.placement ?? "bottom-start");
    setPos({ left: p.left, top: p.top, maxBlockSize: p.maxBlockSize });
  });

  const triggerProps: TooltipTriggerProps = {
    get "aria-describedby"() {
      return open() ? id : undefined;
    },
    onPointerEnter: (e) => show(e.currentTarget),
    onPointerLeave: scheduleHide,
    onFocus: (e) => show(e.currentTarget), // shows on keyboard focus, not hover-only
    onBlur: () => setOpen(false),
    onKeyDown: (e) => {
      if (e.key === "Escape") setOpen(false); // dismissable
    },
  };

  return (
    <>
      {props.trigger(triggerProps)}
      <Show when={open()}>
        <OverlayPortal>
          <div
            ref={tip}
            id={id}
            role="tooltip"
            // hoverable: keep open while the pointer is on the tip (1.4.13).
            onPointerEnter={cancelHide}
            onPointerLeave={scheduleHide}
            style={{
              position: "fixed",
              left: `${pos().left}px`,
              top: `${pos().top}px`,
              "z-index": "var(--z-popover)",
              "max-inline-size": "16rem",
              // Apply the positioner's viewport clamp so a long tooltip scrolls, never overflows (finding 2).
              "max-block-size": pos().maxBlockSize > 0 ? `${pos().maxBlockSize}px` : undefined,
              "overflow-y": "auto",
              background: "var(--surface-overlay)",
              color: "var(--text-primary)",
              border: "var(--hairline) solid var(--border)",
              "border-radius": "var(--radius-1)",
              padding: "var(--space-1) var(--space-2)",
              "font-family": "var(--font-sans)",
              "font-size": "var(--fs-caption)",
              "box-shadow": "var(--shadow-popover)",
              transition: "opacity var(--dur-micro) var(--ease-enter)",
            }}
          >
            {props.text}
          </div>
        </OverlayPortal>
      </Show>
    </>
  );
}
