// Non-modal anchored surface with click and hovercard variants. It supports Escape, outside-click,
// and focus return without trapping focus or locking scroll.

import {
  Show,
  createSignal,
  createEffect,
  createUniqueId,
  mergeProps,
  splitProps,
  onCleanup,
  type JSX,
} from "solid-js";
import { OverlayPortal } from "./primitives/OverlayPortal";
import { createOverlay } from "./primitives/createOverlay";
import { computePosition, type Placement } from "./primitives/position";
import { getFocusable } from "./primitives/overlay-core";

export interface PopoverProps {
  /** Visible content of the trigger button. */
  triggerLabel: JSX.Element;
  /** Accessible name for the popover region. */
  label: string;
  /** click = interactive (focus moves in); hover = hovercard (1.4.13). Default "click". */
  variant?: "click" | "hover";
  placement?: Placement;
  children: JSX.Element;
}

export function Popover(props: PopoverProps): JSX.Element {
  const merged = mergeProps({ variant: "click" as const, placement: "bottom-start" as Placement }, props);
  const [local] = splitProps(merged, ["triggerLabel", "label", "variant", "placement", "children"]);

  const [open, setOpen] = createSignal(false);
  const [pos, setPos] = createSignal({ left: 0, top: 0, maxBlockSize: 0 });
  let trigger: HTMLButtonElement | undefined;
  let panel: HTMLDivElement | undefined;
  let closeTimer: ReturnType<typeof setTimeout> | undefined;
  const panelId = createUniqueId();

  createOverlay({
    isOpen: open,
    onDismiss: () => setOpen(false),
    contentRef: () => panel,
    triggerRef: () => trigger,
    modal: false,
    // Hovercard must not steal focus; the click popover moves focus in (read deferred = reactive-safe).
    autoFocus: () =>
      local.variant === "click" ? (panel ? (getFocusable(panel)[0] ?? panel) : undefined) : undefined,
  });

  // Position after the panel mounts (shared clamp helper).
  createEffect(() => {
    if (!open() || !trigger || !panel) return;
    const p = computePosition(trigger, panel, local.placement);
    setPos({ left: p.left, top: p.top, maxBlockSize: p.maxBlockSize });
  });

  const cancelClose = () => {
    if (closeTimer) clearTimeout(closeTimer);
    closeTimer = undefined;
  };
  // Persistent: a brief pointer-leave does not dismiss (WCAG 1.4.13 "persistent").
  const scheduleClose = () => {
    if (local.variant !== "hover") return;
    cancelClose();
    closeTimer = setTimeout(() => setOpen(false), 120);
  };
  onCleanup(cancelClose);

  // Handlers are always attached; each is a tracked scope and no-ops for the other variant. This
  // keeps the variant read out of component-setup scope (solid/reactivity).
  const onTriggerPointerEnter = () => {
    if (local.variant !== "hover") return;
    cancelClose();
    setOpen(true);
  };
  const onTriggerFocusIn = () => {
    if (local.variant === "hover") setOpen(true);
  };

  return (
    <>
      <button
        ref={trigger}
        type="button"
        // Only the interactive click variant advertises a dialog popup.
        aria-haspopup={local.variant === "click" ? "dialog" : undefined}
        aria-expanded={open()}
        aria-controls={open() ? panelId : undefined}
        onClick={() => local.variant === "click" && setOpen((v) => !v)}
        onPointerEnter={onTriggerPointerEnter}
        onPointerLeave={scheduleClose}
        onFocusIn={onTriggerFocusIn}
        onFocusOut={scheduleClose}
        style={{
          height: "var(--control-h)",
          padding: "0 var(--space-3)",
          background: "var(--surface-raised)",
          color: "var(--text-primary)",
          border: "var(--hairline) solid var(--border)",
          "border-radius": "var(--radius-1)",
          cursor: "pointer",
        }}
      >
        {local.triggerLabel}
      </button>

      <Show when={open()}>
        <OverlayPortal>
          <div
            ref={panel}
            id={panelId}
            // click = an interactive dialog; hover = a non-modal hovercard (role=note, lighter
            // semantics — never over-promise modal-dialog to AT) (fe-ds finding 7).
            role={local.variant === "click" ? "dialog" : "note"}
            aria-label={local.label}
            // hoverable: keep the hovercard open while the pointer is on it (1.4.13).
            onPointerEnter={cancelClose}
            onPointerLeave={scheduleClose}
            style={{
              position: "fixed",
              left: `${pos().left}px`,
              top: `${pos().top}px`,
              "z-index": "var(--z-popover)",
              "max-inline-size": "20rem",
              // Apply the positioner's viewport clamp so a tall popover scrolls, never overflows
              // off-screen (fe-ds finding 2).
              "max-block-size": pos().maxBlockSize > 0 ? `${pos().maxBlockSize}px` : undefined,
              "overflow-y": "auto",
              background: "var(--surface-overlay)",
              color: "var(--text-primary)",
              border: "var(--hairline) solid var(--border)",
              "border-radius": "var(--radius-1)",
              "box-shadow": "var(--shadow-popover)",
              padding: "var(--space-3)",
              "font-family": "var(--font-sans)",
              "font-size": "var(--fs-body)",
              transition: "opacity var(--dur-micro) var(--ease-enter)",
            }}
          >
            {local.children}
          </div>
        </OverlayPortal>
      </Show>
    </>
  );
}
