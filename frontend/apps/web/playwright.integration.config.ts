import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.MYELIN_INTEGRATION_WEB_URL?.trim();
if (!baseURL) {
  throw new Error(
    "MYELIN_INTEGRATION_WEB_URL is required; run this test with fed test:integration",
  );
}

// The installed Playwright package is the browser revision authority. An explicit override is
// useful for managed environments; inferring a cache path would become stale on every upgrade.
const chromiumPath = process.env.CHROMIUM_PATH?.trim() || undefined;

export default defineConfig({
  testDir: "./tests/integration-browser",
  timeout: 30_000,
  expect: { timeout: 10_000 },
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL,
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
});
