// Rendering and accessibility smoke test for the styleguide demo.
import { render } from "@solidjs/testing-library";
import { axe } from "vitest-axe";
import { expect, it, describe } from "vitest";
import { Demo } from "./Demo";

describe("design-system Demo", () => {
  it("renders the token-only smoke component", () => {
    const { getByRole } = render(() => <Demo />);
    expect(getByRole("heading", { level: 1 })).toBeTruthy();
    expect(getByRole("button", { name: /primary action/i })).toBeTruthy();
  });

  it("has no axe accessibility violations", async () => {
    const { container } = render(() => <Demo />);
    // color-contrast is disabled here: jsdom has no canvas/layout, so axe can't compute real
    // contrast. Contrast is gated authoritatively by the design manual's CI ratios + (later) the
    // Playwright real-browser axe run. Every OTHER axe rule runs.
    const results = await axe(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results).toHaveNoViolations();
  });
});
