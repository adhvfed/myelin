// Reference visibility, link behavior, and visible state labels.
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { describe, it, expect, vi } from "vitest";
import { Chip } from "./Chip";

describe("Chip — reference states", () => {
  it("a live reference with an href renders as a real link", () => {
    render(() => <Chip type="issue" label="ISS-482" state="live" href="/issues/ISS-482" />);
    const el = screen.getByRole("link", { name: /issue, ISS-482/ });
    expect(el.getAttribute("href")).toBe("/issues/ISS-482");
  });

  it("a no-access reference is a non-link span (nothing to leak) even if an href was passed", () => {
    render(() => <Chip type="run" label="Restricted" state="no_access" href="/ci/runs/4117" onActivate={vi.fn()} />);
    // No link role — a withheld reference is never navigable.
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
    const el = screen.getByLabelText(/run, Restricted, restricted/);
    expect(el.tagName.toLowerCase()).toBe("span");
  });

  it("a no-href action is a keyboard-native button", () => {
    const onActivate = vi.fn();
    render(() => <Chip type="agent" label="Open agent" onActivate={onActivate} />);

    fireEvent.click(screen.getByRole("button", { name: /agent, Open agent/ }));
    expect(onActivate).toHaveBeenCalledOnce();
  });

  it("renders the state word as TEXT for a non-live reference", () => {
    render(() => <Chip type="doc" label="ADR-05" state="outdated" />);
    expect(screen.getByText("outdated")).toBeTruthy();
  });
});
