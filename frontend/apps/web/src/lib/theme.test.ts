import { describe, expect, it } from "vitest";
import { applyTheme, nextTheme, restoreTheme } from "./theme";

describe("appearance theme", () => {
  it("cycles all supported modes without inventing a fourth state", () => {
    expect(nextTheme("dark")).toBe("light");
    expect(nextTheme("light")).toBe("high-contrast");
    expect(nextTheme("high-contrast")).toBe("dark");
  });

  it("persists explicit choices and rejects an unknown stored mode", () => {
    const root = { dataset: {} } as HTMLElement;
    const writes: Record<string, string> = {};
    expect(applyTheme("light", root, { setItem: (key, value) => { writes[key] = value; } })).toBe("light");
    expect(root.dataset.theme).toBe("light");
    expect(Object.values(writes)).toEqual(["light"]);
    expect(restoreTheme(root, { getItem: () => "invented" })).toBe("light");
  });
});
