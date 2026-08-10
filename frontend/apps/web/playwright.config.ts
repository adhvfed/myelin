import { defineConfig, devices } from "@playwright/test";

// Run the web app and its contract backend together in a real browser.
const PORT = Number(process.env.PORT ?? 3000);
const EDGE_PORT = Number(process.env.DEV_EDGE_PORT ?? 8787);
// CI installs the exact Playwright-pinned browser and should let Playwright resolve it. Local
// development keeps using the already-cached full Chromium binary unless explicitly overridden.
const CHROMIUM_PATH =
  process.env.CHROMIUM_PATH ??
  (process.env.CI
    ? undefined
    : `${process.env.HOME}/.cache/ms-playwright/chromium-1208/chrome-linux64/chrome`);

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
        // Use the CACHED chromium (no download): point at the full chrome binary so Playwright does
        // not reach for the version-pinned chrome-headless-shell it would otherwise fetch. Override
        // with CHROMIUM_PATH if the cache layout differs.
        launchOptions: CHROMIUM_PATH ? { executablePath: CHROMIUM_PATH } : {},
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
