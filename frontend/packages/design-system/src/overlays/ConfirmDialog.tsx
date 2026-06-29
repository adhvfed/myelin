// ConfirmDialog — the small modal that gates IRREVERSIBLE / GDPR / agent-HITL actions (overlays.md
// §3). A thin specialization of Dialog: it is the SAME modal substrate with role="alertdialog",
// the consequence text wired to aria-describedby, and the binding house rule — DEFAULT FOCUS ON THE
// SAFE ACTION (Cancel), never the destructive one (overlays.md §3, "don't default-focus the
// destructive action"). Everything else (reversible actions) should prefer an undo-Toast instead.

import { createUniqueId, mergeProps, splitProps, Show, type JSX } from "solid-js";
import { Dialog } from "./Dialog";
import { Icon } from "../Icon";

export interface ConfirmDialogProps {
  open: boolean;
  /** Cancel / Escape / backdrop — the SAFE path. */
  onCancel: () => void;
  /** The consequential action. */
  onConfirm: () => void;
  /** Plain-language consequence as the title. */
  title: string;
  /** What will change, on what — announced via aria-describedby (concrete for GDPR/HITL). */
  description: string;
  /** destructive = danger token + glyph; confirm = neutral primary. Default "confirm". */
  variant?: "confirm" | "destructive";
  confirmLabel?: string;
  cancelLabel?: string;
}

export function ConfirmDialog(props: ConfirmDialogProps): JSX.Element {
  const merged = mergeProps(
    { variant: "confirm" as const, confirmLabel: "Confirm", cancelLabel: "Cancel" },
    props,
  );
  const [local] = splitProps(merged, [
    "open",
    "onCancel",
    "onConfirm",
    "title",
    "description",
    "variant",
    "confirmLabel",
    "cancelLabel",
  ]);

  let cancelBtn: HTMLButtonElement | undefined;
  const confirmId = createUniqueId();

  const confirmBg = () =>
    local.variant === "destructive" ? "var(--c-btn-danger-bg)" : "var(--c-btn-primary-bg)";
  const confirmFg = () =>
    local.variant === "destructive" ? "var(--c-btn-danger-text)" : "var(--c-btn-primary-text)";

  return (
    <Dialog
      open={local.open}
      onClose={local.onCancel}
      title={local.title}
      description={local.description}
      size="sm"
      role="alertdialog"
      // SAFE-action default focus — the binding ConfirmDialog rule.
      initialFocus={() => cancelBtn}
      footer={
        <>
          <button
            ref={cancelBtn}
            type="button"
            onClick={() => local.onCancel()}
            style={{
              height: "var(--control-h)",
              padding: "0 var(--space-4)",
              background: "transparent",
              color: "var(--text-primary)",
              border: "var(--hairline) solid var(--border-strong)",
              "border-radius": "var(--radius-1)",
              cursor: "pointer",
            }}
          >
            {local.cancelLabel}
          </button>
          <button
            type="button"
            id={confirmId}
            onClick={() => local.onConfirm()}
            style={{
              display: "inline-flex",
              "align-items": "center",
              gap: "var(--space-1)",
              height: "var(--control-h)",
              padding: "0 var(--space-4)",
              background: confirmBg(),
              color: confirmFg(),
              border: "none",
              "border-radius": "var(--radius-1)",
              cursor: "pointer",
            }}
          >
            {/* destructive pairs the danger token with a glyph + label — never colour alone (§7b). */}
            <Show when={local.variant === "destructive"}>
              <Icon name="check-fail" size={14} />
            </Show>
            {local.confirmLabel}
          </button>
        </>
      }
    />
  );
  // The describedby consequence text is rendered by Dialog from `description`; the ConfirmDialog
  // body stays empty (no children) unless a host extends it.
}
