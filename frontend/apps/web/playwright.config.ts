import { defineConfig, devices } from "@playwright/test";

// Run the web app and its contract backend together in a real browser. The ports are the suite's
// own, not the app's usual 3000, so a running `fed` stack is never mistaken for the contract
// server. Specs and the dev edge read them back from the environment.
process.env.PORT ??= "3170";
process.env.DEV_EDGE_PORT ??= "8787";
const PORT = Number(process.env.PORT);
const EDGE_PORT = Number(process.env.DEV_EDGE_PORT);
// Playwright owns the browser revision pin. Keep an override for environments that provide their
// own Chromium, but never guess the package manager's cache layout or revision.
const chromiumPath = process.env.CHROMIUM_PATH?.trim() || undefined;

export default defineConfig({
  testDir: "./tests/e2e",
  timeout: 30_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL: `http://localhost:${PORT}`,
    headless: true,
    trace: "on-first-retry",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        launchOptions: chromiumPath ? { executablePath: chromiumPath } : {},
      },
    },
  ],
  webServer: [
    {
      command: "node dev-edge/server.mjs",
      url: `http://127.0.0.1:${EDGE_PORT}/healthz`,
      reuseExistingServer: !process.env.CI,
      timeout: 30_000,
    },
    {
      command: "pnpm exec vinxi dev",
      url: `http://localhost:${PORT}`,
      reuseExistingServer: !process.env.CI,
      timeout: 180_000,
      env: {
        PORT: String(PORT),
        MYELIN_EDGE_URL: `http://127.0.0.1:${EDGE_PORT}`,
        MYELIN_DEV_LOGIN: "1",
      },
    },
  ],
});
