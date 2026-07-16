// Chip gate (R3.3 / reference-chip §5): a no-access reference withholds its title + renders as a
// non-link span (non-leak by construction); a live reference with an href is a real link; the state
// word is TEXT, never colour alone.
import { render, screen } from "@solidjs/testing-library";
import { describe, it, expect } from "vitest";
import { Chip } from "./Chip";

describe("Chip — reference states", () => {
  it("a live reference with an href renders as a real link", () => {
    render(() => <Chip type="issue" label="ISS-482" state="live" href="/issues/ISS-482" />);
    const el = screen.getByRole("link", { name: /issue, ISS-482/ });
    expect(el.getAttribute("href")).toBe("/issues/ISS-482");
  });

  it("a no-access reference is a non-link span (nothing to leak) even if an href was passed", () => {
    render(() => <Chip type="run" label="Restricted" state="no_access" href="/ci/runs/4117" />);
    // No link role — a withheld reference is never navigable.
    expect(screen.queryByRole("link")).toBeNull();
    const el = screen.getByLabelText(/run, Restricted, restricted/);
    expect(el.tagName.toLowerCase()).toBe("span");
  });

  it("renders the state word as TEXT for a non-live reference", () => {
    render(() => <Chip type="doc" label="ADR-05" state="outdated" />);
    expect(screen.getByText("outdated")).toBeTruthy();
  });
});
