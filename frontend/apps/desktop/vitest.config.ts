import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";

export default defineConfig({
  plugins: [solid()],
  resolve: {
    // Solid needs its dev/browser conditions to compile + run under jsdom.
    conditions: ["development", "browser"],
  },
  test: {
    environment: "jsdom",
    globals: true,
    // Explicit jest-dom setup (mirrors the design-system). vite-plugin-solid auto-injects this
    // when a /jest-dom/ path is absent; naming it here keeps the behaviour visible + resolvable.
    setupFiles: ["@testing-library/jest-dom/vitest"],
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
