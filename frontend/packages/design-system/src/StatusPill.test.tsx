// StatusPill gate (R3.1 / DESIGN-MANUAL §3.1, WCAG 1.4.1): status is announced as TEXT (a visible
// label + a title), never colour alone; the check-verdict label is derived from the counts.
import { render, screen } from "@solidjs/testing-library";
import { describe, it, expect } from "vitest";
import { StatusPill, checkVerdictLabel } from "./StatusPill";

describe("StatusPill — pr-state", () => {
  it("renders a visible label AND a title for each state (never colour-only)", () => {
    for (const [state, label] of [
      ["open", "Open"],
      ["draft", "Draft"],
      ["merged", "Merged"],
      ["closed", "Closed"],
    ] as const) {
      const { unmount } = render(() => <StatusPill kind="pr-state" state={state} />);
      // The state is announced both as a title AND as visible label text (a greyscale reader still
      // reads it — status is never colour-only).
      const el = screen.getByTitle(`State: ${label}`);
      expect(el.textContent).toContain(label);
      unmount();
    }
  });
});

describe("StatusPill — check-verdict", () => {
  it("labels each verdict as text derived from the counts", () => {
    expect(checkVerdictLabel({ kind: "check-verdict", verdict: "pass", total: 5, passing: 5 })).toBe("all passing");
    expect(checkVerdictLabel({ kind: "check-verdict", verdict: "pass", merged: true, total: 3, passing: 3 })).toBe("merged green");
    expect(checkVerdictLabel({ kind: "check-verdict", verdict: "fail", failing: 2, total: 5 })).toBe("2 failing");
    expect(checkVerdictLabel({ kind: "check-verdict", verdict: "running", passing: 4, total: 5 })).toBe("1 running");
    expect(checkVerdictLabel({ kind: "check-verdict", verdict: "none" })).toBe("no checks");
    expect(checkVerdictLabel({ kind: "check-verdict", verdict: "unavailable" })).toBe("checks unavailable");
  });

  it("renders the label as visible text with a title", () => {
    render(() => <StatusPill kind="check-verdict" verdict="running" passing={4} total={5} />);
    const el = screen.getByText("1 running");
    expect(el).toBeTruthy();
    expect(el.getAttribute("title")).toBe("Checks: 1 running");
  });
});

describe("StatusPill — issue-state", () => {
  it("renders each workflow category with the project's visible state label", () => {
    for (const [category, label] of [
      ["unstarted", "Todo"],
      ["started", "In progress"],
      ["completed", "Done"],
      ["cancelled", "Cancelled"],
    ] as const) {
      const { unmount } = render(() => (
        <StatusPill kind="issue-state" category={category} label={label} />
      ));
      const el = screen.getByTitle(`State: ${label}`);
      expect(el.textContent).toContain(label);
      unmount();
    }
  });
});
