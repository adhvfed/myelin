// Menu gate (APG menu-button): axe-clean open; roving tabindex; ↑/↓ navigation; type-ahead;
// Enter activates; Escape closes + returns focus.
import { render, screen, fireEvent } from "@solidjs/testing-library";
import { axe } from "vitest-axe";
import { describe, it, expect, vi } from "vitest";
import { Menu, type MenuItemSpec } from "./Menu";

interface MenuSpies {
  rename: ReturnType<typeof vi.fn>;
  duplicate: ReturnType<typeof vi.fn>;
  del: ReturnType<typeof vi.fn>;
}

function makeItems(spy: MenuSpies): MenuItemSpec[] {
  return [
    { label: "Rename", onSelect: spy.rename },
    { label: "Duplicate", onSelect: spy.duplicate },
    { label: "Delete", onSelect: spy.del },
  ];
}

function renderMenu() {
  const spy: MenuSpies = { rename: vi.fn(), duplicate: vi.fn(), del: vi.fn() };
  render(() => <Menu label="Row actions" items={makeItems(spy)} triggerLabel="Actions" />);
  const trigger = screen.getByRole("button", { name: /Actions/ });
  return { spy, trigger };
}

describe("Menu", () => {
  it("is axe-clean open with role=menu / menuitem", async () => {
    renderMenu();
    fireEvent.click(screen.getByRole("button", { name: /Actions/ }));
    expect(screen.getByRole("menu", { name: "Row actions" })).toBeTruthy();
    expect(screen.getAllByRole("menuitem")).toHaveLength(3);
    const results = await axe(document.body, { rules: { "color-contrast": { enabled: false }, region: { enabled: false } } });
    expect(results).toHaveNoViolations();
  });

  it("opens with ArrowDown focusing the first item and roves with arrow keys (roving tabindex)", () => {
    const { trigger } = renderMenu();
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const items = screen.getAllByRole("menuitem");
    expect(document.activeElement).toBe(items[0]);
    expect(items[0]!.getAttribute("tabindex")).toBe("0");
    expect(items[1]!.getAttribute("tabindex")).toBe("-1");

    fireEvent.keyDown(items[0]!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(items[1]);
    expect(items[1]!.getAttribute("tabindex")).toBe("0");
  });

  it("type-ahead jumps to the first matching item", () => {
    const { trigger } = renderMenu();
    fireEvent.click(trigger);
    const items = screen.getAllByRole("menuitem");
    fireEvent.keyDown(items[0]!, { key: "d" }); // -> "Duplicate"
    expect(document.activeElement).toBe(items[1]);
  });

  it("Enter activates the active item, closes, and returns focus to the trigger", () => {
    const { spy, trigger } = renderMenu();
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "ArrowDown" }); // open, active = Rename
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" }); // active = Duplicate
    fireEvent.keyDown(document.activeElement!, { key: "Enter" });
    expect(spy.duplicate).toHaveBeenCalledOnce();
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("Escape closes and returns focus to the trigger", () => {
    const { trigger } = renderMenu();
    trigger.focus();
    fireEvent.click(trigger);
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});
