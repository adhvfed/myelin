import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.MYELIN_INTEGRATION_WEB_URL?.trim();
if (!baseURL) {
  throw new Error(
    "MYELIN_INTEGRATION_WEB_URL is required; run this test with fed test:integration",
  );
}

const chromiumPath =
  process.env.CHROMIUM_PATH ??
  (process.env.CI
    ? undefined
    : `${process.env.HOME}/.cache/ms-playwright/chromium-1208/chrome-linux64/chrome`);

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
