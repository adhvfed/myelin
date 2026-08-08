import { defineConfig } from "vitest/config";

// Unit tests run in node (the gateway-client core is pure, server-side logic). Component a11y is gated
// by the Playwright + real-browser axe harness (better than jsdom for the chrome + overlays).
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
