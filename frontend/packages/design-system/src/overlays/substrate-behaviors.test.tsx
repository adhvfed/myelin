// Cross-overlay behavior: inert restoration, toast stacking, reactive dismissal, focus defaults,
// topmost Escape, menu Tab handling, toast pause, and scroll-lock padding.
import { render, screen, fireEvent } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { Dialog } from "./Dialog";
import { ConfirmDialog } from "./ConfirmDialog";
import { ToastProvider, useToast } from "./Toast";
import { lockScroll, unlockScroll } from "./primitives/overlay-core";

describe("background inert (set + restore)", () => {
  it("marks background body children [inert] while a modal is open and restores on close", () => {
    const bg = document.createElement("div");
    bg.textContent = "background";
    document.body.appendChild(bg);
    try {
      const [open, setOpen] = createSignal(true);
      render(() => (
        <Dialog open={open()} onClose={() => setOpen(false)} title="M">
          body
        </Dialog>
      ));
      expect(bg.hasAttribute("inert")).toBe(true); // background removed from the a11y tree
      setOpen(false);
      expect(bg.hasAttribute("inert")).toBe(false); // and restored exactly on close
    } finally {
      bg.remove();
    }
  });
});

describe("the Toast layer survives a modal's inert background (finding 1)", () => {
  it("keeps the Toast region live/announced and its Undo reachable over an open Dialog", () => {
    function Harness() {
      const toast = useToast();
      return (
        <>
          <button onClick={() => toast.show({ title: "Saved", onUndo: () => {} })}>Notify</button>
          <Dialog open={true} onClose={() => {}} title="M">
            body
          </Dialog>
        </>
      );
    }
    render(() => (
      <ToastProvider>
        <Harness />
      </ToastProvider>
    ));
    fireEvent.click(screen.getByRole("button", { name: "Notify" }));

    const region = screen.getByRole("region", { name: "Notifications" });
    // Keep the toast portal active while a modal is open.
    let bodyChild: HTMLElement | null = region;
    while (bodyChild && bodyChild.parentElement !== document.body) bodyChild = bodyChild.parentElement;
    expect(bodyChild).toBeTruthy();
    expect(bodyChild!.hasAttribute("inert")).toBe(false);
    // It is announced (role=status live region) and its Undo is not inside an inert subtree.
    expect(screen.getByRole("status")).toHaveTextContent("Saved");
    expect(screen.getByRole("button", { name: "Undo" }).closest("[inert]")).toBeNull();
  });
});

describe("Escape acts on the topmost overlay only (Confirm over Dialog)", () => {
  it("dismisses just the Confirm and leaves the outer Dialog open", () => {
    function Nest() {
      const [confirm, setConfirm] = createSignal(false);
      const [outer, setOuter] = createSignal(true);
      return (
        <Dialog open={outer()} onClose={() => setOuter(false)} title="Outer">
          <button onClick={() => setConfirm(true)}>Danger</button>
          <ConfirmDialog
            open={confirm()}
            title="Sure?"
            description="This is irreversible."
            onConfirm={() => setConfirm(false)}
            onCancel={() => setConfirm(false)}
          />
        </Dialog>
      );
    }
    render(() => <Nest />);
    fireEvent.click(screen.getByRole("button", { name: "Danger" }));
    expect(screen.getByRole("alertdialog")).toBeTruthy();

    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("alertdialog")).toBeNull(); // topmost dismissed
    expect(screen.getByRole("dialog")).toBeTruthy(); // outer survives
  });
});

describe("Dialog default initial focus (finding 5)", () => {
  it("targets the body's first control, never the Close (X)", () => {
    const [open, setOpen] = createSignal(false);
    render(() => (
      <>
        <button onClick={() => setOpen(true)}>Open</button>
        <Dialog open={open()} onClose={() => setOpen(false)} title="M">
          <input aria-label="name" />
          <button>Save</button>
        </Dialog>
      </>
    ));
    const trigger = screen.getByRole("button", { name: "Open" });
    trigger.focus();
    fireEvent.click(trigger);
    const close = screen.getByRole("button", { name: "Close dialog" });
    expect(document.activeElement).not.toBe(close);
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "name" }));
  });

  it("with no focusable body content, focuses the dialog panel (still not the Close X)", () => {
    render(() => (
      <Dialog open={true} onClose={() => {}} title="M">
        just text, nothing focusable
      </Dialog>
    ));
    const dialog = screen.getByRole("dialog");
    const close = screen.getByRole("button", { name: "Close dialog" });
    expect(document.activeElement).not.toBe(close);
    expect(document.activeElement).toBe(dialog);
  });
});

describe("reactive dismiss toggle does not churn the overlay (finding 3)", () => {
  it("toggling dismissable while open keeps focus + scroll-lock, and is honoured at event time", () => {
    const [open, setOpen] = createSignal(true);
    const [dismissable, setDismissable] = createSignal(true);
    render(() => (
      <Dialog open={open()} onClose={() => setOpen(false)} title="M" dismissable={dismissable()}>
        <input aria-label="field" />
        <button>Go</button>
      </Dialog>
    ));
    const go = screen.getByRole("button", { name: "Go" });
    go.focus(); // move off the initial-focus target
    expect(document.activeElement).toBe(go);
    expect(document.body.style.overflow).toBe("hidden");

    setDismissable(false);
    // Before the fix, re-running the effect yanked focus back to the initial target + churned the lock.
    expect(document.activeElement).toBe(go);
    expect(document.body.style.overflow).toBe("hidden");

    // The flag is read at event time: dismissable=false → Escape no longer closes.
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.getByRole("dialog")).toBeTruthy();

    // Re-enable → Escape closes.
    setDismissable(true);
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("scroll-lock scrollbar-width compensation", () => {
  it("pads the body by the scrollbar width on lock and restores it on unlock", () => {
    const innerDesc = Object.getOwnPropertyDescriptor(window, "innerWidth");
    const clientDesc = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
    Object.defineProperty(window, "innerWidth", { value: 1000, configurable: true });
    Object.defineProperty(document.documentElement, "clientWidth", { value: 985, configurable: true });
    document.body.style.paddingRight = "4px";
    try {
      lockScroll();
      expect(document.body.style.overflow).toBe("hidden");
      expect(document.body.style.paddingRight).toBe("19px"); // 4 + (1000 - 985)
      unlockScroll();
      expect(document.body.style.overflow).toBe("");
      expect(document.body.style.paddingRight).toBe("4px"); // restored exactly
    } finally {
      document.body.style.paddingRight = "";
      if (innerDesc) Object.defineProperty(window, "innerWidth", innerDesc);
      // Remove the instance-level shadow so the prototype getter is restored.
      delete (document.documentElement as unknown as { clientWidth?: number }).clientWidth;
      if (clientDesc) Object.defineProperty(HTMLElement.prototype, "clientWidth", clientDesc);
    }
  });
});

describe("Toast pause-on-hover re-arms the auto-dismiss timer", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("pauses the timeout while hovered and re-arms on pointer leave", () => {
    function Trigger() {
      const toast = useToast();
      return <button onClick={() => toast.show({ title: "FYI", duration: 1000 })}>Info</button>;
    }
    render(() => (
      <ToastProvider>
        <Trigger />
      </ToastProvider>
    ));
    fireEvent.click(screen.getByRole("button", { name: "Info" }));
    const toast = screen.getByRole("status");

    fireEvent.pointerEnter(toast); // pause
    vi.advanceTimersByTime(1500);
    expect(screen.queryByText("FYI")).toBeTruthy(); // paused → not dismissed past its duration

    fireEvent.pointerLeave(toast); // re-arm
    vi.advanceTimersByTime(1500);
    expect(screen.queryByText("FYI")).toBeNull(); // re-armed timer fired → dismissed
  });
});
