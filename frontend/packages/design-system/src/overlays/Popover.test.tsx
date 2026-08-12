// Popover accessibility, dismissal, focus return, scroll behavior, and hovercard activation.
import { render, screen, fireEvent } from "@solidjs/testing-library";
import { axe } from "vitest-axe";
import { describe, it, expect } from "vitest";
import { Popover } from "./Popover";

describe("Popover (click)", () => {
  it("is axe-clean open, toggles on the trigger, exposes aria-expanded/controls", async () => {
    render(() => (
      <Popover triggerLabel="Filters" label="Filter builder">
        <button>Apply</button>
      </Popover>
    ));
    const trigger = screen.getByRole("button", { name: "Filters" });
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    trigger.focus();
    fireEvent.click(trigger);
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByRole("dialog", { name: "Filter builder" })).toBeTruthy();
    const results = await axe(document.body, { rules: { "color-contrast": { enabled: false }, region: { enabled: false } } });
    expect(results).toHaveNoViolations();
  });

  it("is NON-modal: it does not lock body scroll", () => {
    render(() => (
      <Popover triggerLabel="Filters" label="Filter builder">
        <button>Apply</button>
      </Popover>
    ));
    const trigger = screen.getByRole("button", { name: "Filters" });
    fireEvent.click(trigger);
    expect(document.body.style.overflow).toBe(""); // not "hidden" — non-modal never scroll-locks
  });

  it("Escape dismisses and returns focus to the trigger", () => {
    render(() => (
      <Popover triggerLabel="Filters" label="Filter builder">
        <button>Apply</button>
      </Popover>
    ));
    const trigger = screen.getByRole("button", { name: "Filters" });
    trigger.focus();
    fireEvent.click(trigger);
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("dismisses on an outside pointer-down", () => {
    render(() => (
      <Popover triggerLabel="Filters" label="Filter builder">
        <button>Apply</button>
      </Popover>
    ));
    const trigger = screen.getByRole("button", { name: "Filters" });
    fireEvent.click(trigger);
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("Popover (hovercard)", () => {
  it("opens on keyboard focus of the trigger (1.4.13: hover AND focus)", () => {
    render(() => (
      <Popover variant="hover" triggerLabel="@alice" label="alice card">
        Profile
      </Popover>
    ));
    const trigger = screen.getByRole("button", { name: "@alice" });
    trigger.focus(); // real focus dispatches focusin → opens (hover AND focus)
    expect(screen.getByRole("note", { name: "alice card" })).toBeTruthy();
    // Esc dismisses (dismissable).
    fireEvent.keyDown(trigger, { key: "Escape" });
    expect(screen.queryByRole("note")).toBeNull();
  });

  it("does NOT advertise dialog semantics for the hovercard (finding 7)", () => {
    render(() => (
      <Popover variant="hover" triggerLabel="@alice" label="alice card">
        Profile
      </Popover>
    ));
    const trigger = screen.getByRole("button", { name: "@alice" });
    // The hover trigger must not claim aria-haspopup=dialog (it's a non-modal informational surface).
    expect(trigger.getAttribute("aria-haspopup")).toBeNull();
    trigger.focus();
    // The panel is a lighter role=note, not role=dialog.
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("note", { name: "alice card" })).toBeTruthy();
  });

  it("the click variant still advertises dialog semantics", () => {
    render(() => (
      <Popover variant="click" triggerLabel="Filters" label="Filter builder">
        <button>Apply</button>
      </Popover>
    ));
    const trigger = screen.getByRole("button", { name: "Filters" });
    expect(trigger.getAttribute("aria-haspopup")).toBe("dialog");
    fireEvent.click(trigger);
    expect(screen.getByRole("dialog", { name: "Filter builder" })).toBeTruthy();
  });
});
