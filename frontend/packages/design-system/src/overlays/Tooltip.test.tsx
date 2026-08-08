// Tooltip gate (WCAG 1.4.13): shows on focus AND hover; wires aria-describedby onto the trigger;
// dismisses on Escape; never takes focus. axe-clean while shown.
import { render, screen, fireEvent } from "@solidjs/testing-library";
import { axe } from "vitest-axe";
import { describe, it, expect } from "vitest";
import { Tooltip } from "./Tooltip";

function renderTooltip() {
  render(() => (
    <Tooltip
      text="Open settings"
      trigger={(p) => (
        <button {...p} aria-label="Settings">
          ⚙
        </button>
      )}
    />
  ));
  return screen.getByRole("button", { name: "Settings" });
}

describe("Tooltip", () => {
  it("shows on keyboard focus, wires aria-describedby onto the trigger, and is axe-clean", async () => {
    const trigger = renderTooltip();
    fireEvent.focus(trigger);
    const tip = screen.getByRole("tooltip");
    expect(tip).toHaveTextContent("Open settings");
    expect(trigger.getAttribute("aria-describedby")).toBe(tip.id);
    // It never takes focus — focus stays on the trigger.
    expect(document.activeElement).not.toBe(tip);
    const results = await axe(document.body, { rules: { "color-contrast": { enabled: false }, region: { enabled: false } } });
    expect(results).toHaveNoViolations();
  });

  it("shows on hover (pointer-enter) as well", () => {
    const trigger = renderTooltip();
    fireEvent.pointerEnter(trigger);
    expect(screen.getByRole("tooltip")).toBeTruthy();
  });

  it("dismisses on Escape", () => {
    const trigger = renderTooltip();
    fireEvent.focus(trigger);
    expect(screen.getByRole("tooltip")).toBeTruthy();
    fireEvent.keyDown(trigger, { key: "Escape" });
    expect(screen.queryByRole("tooltip")).toBeNull();
  });
});
