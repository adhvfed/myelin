// Toast — the transient corner notice + host of the undo affordance (overlays.md §7). A toast NEVER
// steals focus (WCAG 4.1.3); AT is informed via a live region: role="status" (polite) for the vast
// majority, role="alert" (assertive) only for genuinely time-critical/blocking events (danger).
// Status is glyph + label, never colour alone (WCAG 1.4.1). Auto-timeout with pause-on-hover and
// pause-on-focus; danger is persistent by default. F6 moves focus into the region so keyboard users
// can reach an Undo (the documented hotkey, overlays.md §7).

import {
  For,
  Show,
  createContext,
  useContext,
  createSignal,
  createEffect,
  createUniqueId,
  onCleanup,
  onMount,
  type JSX,
} from "solid-js";
import { createStore, produce } from "solid-js/store";
import { OverlayPortal } from "./primitives/OverlayPortal";
import { Icon } from "../Icon";
import type { IconName } from "../icon-names";

export type ToastVariant = "info" | "success" | "warning" | "danger";

export interface ToastOptions {
  title: string;
  variant?: ToastVariant;
  /** Adds an Undo action (the optimistic-rollback affordance). */
  onUndo?: () => void;
  /** Auto-dismiss after ms. Default 5000. Ignored if persistent. */
  duration?: number;
  /** No auto-timeout (danger defaults to true). */
  persistent?: boolean;
}

interface ToastItem extends Required<Omit<ToastOptions, "onUndo">> {
  id: string;
  onUndo?: () => void;
}

interface ToastApi {
  show: (opts: ToastOptions) => string;
  dismiss: (id: string) => void;
}

const ToastContext = createContext<ToastApi>();

export function useToast(): ToastApi {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used within a <ToastProvider>");
  return ctx;
}

const GLYPH: Record<ToastVariant, IconName> = {
  info: "message",
  success: "check-pass",
  warning: "check-pending",
  danger: "check-fail",
};

export function ToastProvider(props: { children?: JSX.Element }): JSX.Element {
  const [toasts, setToasts] = createStore<ToastItem[]>([]);
  let region: HTMLDivElement | undefined;

  const dismiss = (id: string) =>
    setToasts(produce((list) => {
      const i = list.findIndex((t) => t.id === id);
      if (i !== -1) list.splice(i, 1);
    }));

  const show = (opts: ToastOptions): string => {
    const variant = opts.variant ?? "info";
    const id = createUniqueId();
    const item: ToastItem = {
      id,
      title: opts.title,
      variant,
      onUndo: opts.onUndo,
      duration: opts.duration ?? 5000,
      // A danger toast needs acknowledgement → persistent by default (overlays.md §7).
      persistent: opts.persistent ?? variant === "danger",
    };
    setToasts(produce((list) => list.push(item)));
    return id;
  };

  // F6 = the documented hotkey to move focus into the toast region (keyboard reach for Undo).
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "F6" && toasts.length > 0) {
      e.preventDefault();
      region?.querySelector<HTMLElement>("button")?.focus();
    }
  };
  // Register the listener AND its teardown inside onMount: onMount never runs on the server, so SSR
  // disposal won't touch `document` (which is undefined there). Client behaviour is unchanged.
  onMount(() => {
    document.addEventListener("keydown", onKey);
    onCleanup(() => document.removeEventListener("keydown", onKey));
  });

  return (
    <ToastContext.Provider value={{ show, dismiss }}>
      {props.children}
      <OverlayPortal>
        {/* A landmark region (not an <ol>): each toast is its own role=status/alert live region, so
            a list wrapper would fight axe's list-structure rule once li roles are overridden. */}
        <div
          ref={region}
          role="region"
          aria-label="Notifications"
          // Marks this as a persistent live layer: a modal's hideOthers() skips it, so a toast raised
          // while a Dialog is open stays announced (WCAG 4.1.3) and its Undo stays F6-reachable.
          data-overlay-live=""
          style={{
            position: "fixed",
            "inset-block-end": "var(--space-4)",
            "inset-inline-end": "var(--space-4)",
            "z-index": "var(--z-toast)",
            display: "flex",
            "flex-direction": "column",
            gap: "var(--space-2)",
            "max-inline-size": "22rem",
          }}
        >
          <For each={toasts}>
            {(t) => <ToastView toast={t} onDismiss={() => dismiss(t.id)} />}
          </For>
        </div>
      </OverlayPortal>
    </ToastContext.Provider>
  );
}

function ToastView(props: { toast: ToastItem; onDismiss: () => void }): JSX.Element {
  const [paused, setPaused] = createSignal(false);

  // Auto-timeout with pause-on-hover / pause-on-focus. Re-armed whenever pause flips.
  createEffect(() => {
    if (props.toast.persistent || paused()) return;
    const timer = setTimeout(props.onDismiss, props.toast.duration);
    onCleanup(() => clearTimeout(timer));
  });

  // polite for the vast majority; assertive only for the time-critical danger case.
  const role = () => (props.toast.variant === "danger" ? "alert" : "status");

  return (
    <div
      role={role()}
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
      onFocusIn={() => setPaused(true)}
      onFocusOut={() => setPaused(false)}
      style={{
        display: "flex",
        "align-items": "center",
        gap: "var(--space-2)",
        background: "var(--surface-overlay)",
        color: "var(--text-primary)",
        border: `var(--hairline) solid var(--${props.toast.variant})`,
        "border-radius": "var(--radius-1)",
        "box-shadow": "var(--shadow-overlay)",
        padding: "var(--space-2) var(--space-3)",
        "font-family": "var(--font-sans)",
        "font-size": "var(--fs-body-sm)",
        transition: "opacity var(--dur-fast) var(--ease-enter)",
      }}
    >
      {/* glyph + label — status is never carried by colour alone (WCAG 1.4.1). */}
      <span style={{ color: `var(--${props.toast.variant})`, display: "inline-flex" }}>
        <Icon name={GLYPH[props.toast.variant]} size={16} />
      </span>
      <span style={{ flex: "1" }}>{props.toast.title}</span>
      <Show when={props.toast.onUndo}>
        {(undo) => (
          <button
            type="button"
            onClick={() => {
              undo()();
              props.onDismiss();
            }}
            style={{
              background: "transparent",
              color: "var(--accent)",
              border: "none",
              "font-weight": "var(--weight-medium)",
              cursor: "pointer",
            }}
          >
            Undo
          </button>
        )}
      </Show>
      <button
        type="button"
        onClick={() => props.onDismiss()}
        aria-label="Dismiss notification"
        style={{
          display: "inline-flex",
          background: "transparent",
          color: "var(--text-muted)",
          border: "none",
          cursor: "pointer",
        }}
      >
        <Icon name="close" size={14} />
      </button>
    </div>
  );
}
