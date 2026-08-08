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
    setupFiles: ["./vitest.setup.ts"],
    include: ["styleguide/**/*.test.{ts,tsx}", "src/**/*.test.{ts,tsx}"],
  },
});
