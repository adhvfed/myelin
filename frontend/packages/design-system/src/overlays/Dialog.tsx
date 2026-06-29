// Dialog — the viewport-centred MODAL primitive (overlays.md §2). The one surface every modal-class
// component inherits: it owns the substrate mechanics via `createOverlay` (portal-to-root, focus-trap
// + return-focus, scroll-lock, inert background, Escape/backdrop dismiss) and the APG dialog ARIA
// (role=dialog + aria-modal + aria-labelledby/aria-describedby). ConfirmDialog and any future
// modal (command palette = Dialog + a search header) compose from THIS — never re-implement it.

import { Show, createUniqueId, splitProps, mergeProps, type JSX } from "solid-js";
import { OverlayPortal } from "./primitives/OverlayPortal";
import { createOverlay } from "./primitives/createOverlay";
import { Icon } from "../Icon";

export type DialogSize = "sm" | "md" | "lg";

export interface DialogProps {
  open: boolean;
  onClose: () => void;
  /** Accessible title — rendered as the heading and wired to aria-labelledby. */
  title: string;
  /** Optional descriptive text wired to aria-describedby (announced by AT). */
  description?: string;
  /** Max inline size token bucket (never a fixed width — German strings must not clip, §8b.4). */
  size?: DialogSize;
  /** Escape + backdrop dismiss. Default true; false = a blocking step that must be resolved. */
  dismissable?: boolean;
  /** alertdialog for consequential confirms (ConfirmDialog sets this). Default "dialog". */
  role?: "dialog" | "alertdialog";
  /** Override the initial-focus target (e.g. ConfirmDialog focuses the SAFE action). */
  initialFocus?: () => HTMLElement | undefined;
  /** Footer action row (primary action right). */
  footer?: JSX.Element;
  children?: JSX.Element;
}

const MAX_INLINE: Record<DialogSize, string> = {
  sm: "24rem",
  md: "32rem",
  lg: "48rem",
};

export function Dialog(props: DialogProps): JSX.Element {
  const merged = mergeProps(
    { size: "md" as DialogSize, dismissable: true, role: "dialog" as const },
    props,
  );
  const [local] = splitProps(merged, [
    "open",
    "onClose",
    "title",
    "description",
    "size",
    "dismissable",
    "role",
    "initialFocus",
    "footer",
    "children",
  ]);

  let panel: HTMLDivElement | undefined;
  const titleId = createUniqueId();
  const descId = createUniqueId();

  createOverlay({
    isOpen: () => local.open,
    onDismiss: () => local.onClose(),
    contentRef: () => panel,
    modal: true,
    closeOnEscape: () => local.dismissable,
    closeOnOutsidePointer: () => local.dismissable, // backdrop dismiss tracks the same flag
    autoFocus: local.initialFocus ?? true,
  });

  return (
    <Show when={local.open}>
      <OverlayPortal>
        {/* Full-viewport layer at the modal z-index (token only — never a magic number). */}
        <div
          style={{
            position: "fixed",
            inset: "0",
            "z-index": "var(--z-modal)",
            display: "flex",
            "align-items": "center",
            "justify-content": "center",
            padding: "var(--space-5)",
          }}
        >
          {/* Scrim: purely visual; dismissal is handled by createOverlay's outside-pointer +
              Escape (so no interactive handler lives on a non-interactive element — jsx-a11y clean). */}
          <div
            aria-hidden="true"
            style={{
              position: "absolute",
              inset: "0",
              background: "var(--overlay-scrim)",
              opacity: "0.6",
              transition: "opacity var(--dur-fast) var(--ease-enter)",
            }}
          />
          <div
            ref={panel}
            role={local.role}
            aria-modal="true"
            aria-labelledby={titleId}
            aria-describedby={local.description ? descId : undefined}
            style={{
              position: "relative",
              display: "flex",
              "flex-direction": "column",
              "max-inline-size": MAX_INLINE[local.size],
              "inline-size": "100%",
              "max-block-size": "calc(100vh - var(--space-7))",
              background: "var(--surface-overlay)",
              color: "var(--text-primary)",
              border: "var(--hairline) solid var(--border-strong)",
              "border-radius": "var(--radius-2)",
              "box-shadow": "var(--shadow-overlay)",
              "font-family": "var(--font-sans)",
              "font-size": "var(--fs-body)",
              transition: "opacity var(--dur-fast) var(--ease-enter)",
            }}
          >
            <header
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "space-between",
                gap: "var(--space-3)",
                padding: "var(--space-4)",
                "border-block-end": "var(--hairline) solid var(--border)",
              }}
            >
              <h2
                id={titleId}
                style={{ margin: "0", "font-size": "var(--fs-h3)", "font-weight": "var(--weight-semibold)" }}
              >
                {local.title}
              </h2>
              <Show when={local.dismissable}>
                <button
                  type="button"
                  onClick={() => local.onClose()}
                  aria-label="Close dialog"
                  style={{
                    display: "inline-flex",
                    "align-items": "center",
                    "justify-content": "center",
                    "min-inline-size": "var(--target-min)",
                    "min-block-size": "var(--target-min)",
                    background: "transparent",
                    border: "none",
                    color: "var(--text-muted)",
                    "border-radius": "var(--radius-1)",
                    cursor: "pointer",
                  }}
                >
                  <Icon name="close" size={16} />
                </button>
              </Show>
            </header>

            <div style={{ padding: "var(--space-4)", overflow: "auto", "min-block-size": "0" }}>
              <Show when={local.description}>
                <p id={descId} style={{ margin: "0 0 var(--space-3)", color: "var(--text-muted)" }}>
                  {local.description}
                </p>
              </Show>
              {local.children}
            </div>

            <Show when={local.footer}>
              <footer
                style={{
                  display: "flex",
                  "justify-content": "flex-end",
                  gap: "var(--space-2)",
                  padding: "var(--space-4)",
                  "border-block-start": "var(--hairline) solid var(--border)",
                }}
              >
                {local.footer}
              </footer>
            </Show>
          </div>
        </div>
      </OverlayPortal>
    </Show>
  );
}
