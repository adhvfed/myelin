// Dialog gate (doc 08: "axe + keyboard tests are the gate"). Exercises the SUBSTRATE that every
// modal inherits: APG dialog ARIA + axe-clean open state, the focus-trap cycle, Escape-closes +
// focus-returns-to-trigger, and body scroll-lock applied/restored.
import { render, screen, fireEvent } from "@solidjs/testing-library";
import { axe } from "vitest-axe";
import { createSignal } from "solid-js";
import { describe, it, expect } from "vitest";
import { Dialog } from "./Dialog";

function Harness() {
  const [open, setOpen] = createSignal(false);
  return (
    <>
      <button onClick={() => setOpen(true)}>Open</button>
      <Dialog open={open()} onClose={() => setOpen(false)} title="Settings" description="Edit your settings.">
        <input aria-label="Name" />
        <button>Save</button>
      </Dialog>
    </>
  );
}

const openDialog = () => {
  const trigger = screen.getByRole("button", { name: "Open" });
  trigger.focus(); // real browsers focus a button on click; jsdom doesn't, so simulate it
  fireEvent.click(trigger);
  return trigger;
};

describe("Dialog", () => {
  it("is axe-clean in the open state (role=dialog, aria-modal, labelled)", async () => {
    render(() => <Harness />);
    openDialog();
    const dialog = screen.getByRole("dialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog).toHaveAccessibleName("Settings");
    const results = await axe(document.body, { rules: { "color-contrast": { enabled: false }, region: { enabled: false } } });
    expect(results).toHaveNoViolations();
  });

  it("traps focus: Tab from the last focusable wraps to the first, Shift+Tab from first wraps to last", () => {
    render(() => <Harness />);
    openDialog();
    const close = screen.getByRole("button", { name: "Close dialog" });
    const save = screen.getByRole("button", { name: "Save" });

    save.focus();
    fireEvent.keyDown(save, { key: "Tab" });
    expect(document.activeElement).toBe(close); // last → first

    close.focus();
    fireEvent.keyDown(close, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(save); // first → last
  });

  it("Escape closes and returns focus to the trigger", () => {
    render(() => <Harness />);
    const trigger = openDialog();
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("locks body scroll while open and restores it on close", () => {
    render(() => <Harness />);
    expect(document.body.style.overflow).toBe("");
    openDialog();
    expect(document.body.style.overflow).toBe("hidden");
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(document.body.style.overflow).toBe("");
  });
});
