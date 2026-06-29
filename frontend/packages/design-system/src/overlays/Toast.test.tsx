// Toast gate (WCAG 4.1.3): announced via a live region; never steals focus; polite by default,
// assertive (role=alert) for danger; Undo affordance; auto-timeout with pause; danger persistent.
import { render, screen, fireEvent } from "@solidjs/testing-library";
import { axe } from "vitest-axe";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { ToastProvider, useToast, type ToastOptions } from "./Toast";

function Trigger(props: { opts: ToastOptions; label: string }) {
  const toast = useToast();
  return <button onClick={() => toast.show(props.opts)}>{props.label}</button>;
}

describe("Toast", () => {
  it("announces via a polite live region without stealing focus, and is axe-clean", async () => {
    render(() => (
      <ToastProvider>
        <Trigger label="Notify" opts={{ title: "Saved" }} />
      </ToastProvider>
    ));
    const trigger = screen.getByRole("button", { name: "Notify" });
    trigger.focus();
    fireEvent.click(trigger);

    const status = screen.getByRole("status");
    expect(status).toHaveTextContent("Saved");
    expect(screen.getByRole("region", { name: "Notifications" })).toBeTruthy();
    // Never steals focus: focus stays on the trigger.
    expect(document.activeElement).toBe(trigger);
    const results = await axe(document.body, { rules: { "color-contrast": { enabled: false }, region: { enabled: false } } });
    expect(results).toHaveNoViolations();
  });

  it("uses an assertive role=alert for danger toasts", () => {
    render(() => (
      <ToastProvider>
        <Trigger label="Fail" opts={{ title: "Upload failed", variant: "danger" }} />
      </ToastProvider>
    ));
    fireEvent.click(screen.getByRole("button", { name: "Fail" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Upload failed");
  });

  it("renders an Undo action that fires the callback and dismisses", () => {
    const onUndo = vi.fn();
    render(() => (
      <ToastProvider>
        <Trigger label="Move" opts={{ title: "Moved to In Progress", onUndo }} />
      </ToastProvider>
    ));
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    expect(onUndo).toHaveBeenCalledOnce();
    expect(screen.queryByRole("status")).toBeNull();
  });
});

describe("Toast timeouts", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("auto-dismisses a normal toast but keeps a persistent danger toast", () => {
    render(() => (
      <ToastProvider>
        <Trigger label="Info" opts={{ title: "FYI", duration: 1000 }} />
        <Trigger label="Err" opts={{ title: "Broke", variant: "danger" }} />
      </ToastProvider>
    ));
    fireEvent.click(screen.getByRole("button", { name: "Info" }));
    fireEvent.click(screen.getByRole("button", { name: "Err" }));
    expect(screen.getByText("FYI")).toBeTruthy();
    vi.advanceTimersByTime(1200);
    expect(screen.queryByText("FYI")).toBeNull(); // auto-dismissed
    expect(screen.getByText("Broke")).toBeTruthy(); // danger persists (needs acknowledgement)
  });
});
