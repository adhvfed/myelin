import { expect, request as pwRequest, test, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

// FRONTEND-CONTRACT: ci-read-dev-edge-parity
// Browser proof for the shared production-Rust/dev-edge CI read vectors.
const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;
const FAILED_RUN = "91000000-0000-4000-8000-000000000001";

async function setCiConfig(config: {
  resetCi?: boolean;
  emptyCi?: boolean;
  ciUnavailable?: boolean;
  ciLogUnavailable?: boolean;
  addCiVisibleRepo?: boolean;
}) {
  const context = await pwRequest.newContext();
  const response = await context.post(`${EDGE}/__test/config`, { data: config });
  expect(response.ok(), "dev-edge CI config must be accepted").toBeTruthy();
  await context.dispose();
}

async function devLogin(page: Page) {
  await page.goto("/login");
  await page.waitForLoadState("networkidle");
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
}

async function expectNoAxeViolations(page: Page, context: string) {
  const result = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(
    result.violations,
    `axe violations on ${context}: ${JSON.stringify(result.violations, null, 2)}`,
  ).toEqual([]);
}

test.beforeEach(async () => setCiConfig({ resetCi: true }));
test.afterEach(async () => setCiConfig({ resetCi: true }));

test.describe("CT-005 CI web read surface", () => {
  test("an unauthenticated deep run preserves the 401 to login floor", async ({ page }) => {
    await page.goto(`/ci/runs/${FAILED_RUN}`);
    await page.waitForURL("**/login");
    await expect(page.getByTestId("dev-login")).toBeVisible();
  });

  test("run list, state filter, detail, steps, and archived bytes use the durable contract", async ({ page }) => {
    await devLogin(page);
    await page.goto("/ci");

    await expect(page.getByRole("heading", { level: 1, name: "CI runs" })).toBeVisible();
    await expect(page.getByTestId("ci-run-row")).toHaveCount(2);
    await expect(page.getByTitle("State: Failed")).toBeVisible();
    await expect(page.getByTitle("State: Running")).toBeVisible();
    await expect(page.getByText("alpha", { exact: true }).first()).toBeVisible();
    await expectNoAxeViolations(page, "CI run list");

    await page.getByLabel("Run state").selectOption("failed");
    await page.getByRole("button", { name: "Apply filter" }).click();
    await expect(page).toHaveURL(/\/ci\?state=failed$/);
    await expect(page.getByTestId("ci-run-row")).toHaveCount(1);
    await expect(page.getByTitle("State: Running")).toHaveCount(0);

    await page.getByTestId("ci-run-row").click();
    await page.waitForURL(`**/ci/runs/${FAILED_RUN}`);
    await expect(page.getByRole("heading", { level: 1, name: "Run 91000000" })).toBeVisible();
    await expect(page.getByRole("heading", { level: 3, name: "contract" })).toBeVisible();
    await expect(page.getByText("byte 0")).toBeVisible();
    await expect(page.getByText("Live updates are not available yet.")).toBeVisible();
    await expect(page.getByText("Archived", { exact: true })).toBeVisible();
    await expectNoAxeViolations(page, "CI run detail");

    await page.getByRole("link", { name: "Read archived output" }).click();
    await expect(page).toHaveURL(/job=92000000-0000-4000-8000-000000000001/);
    await expect(page.getByTestId("ci-archived-log")).toHaveValue("prep\ncafé\nfailed\n");
    await expect(page.getByText("Bytes 0–18 of 18")).toBeVisible();
    await expectNoAxeViolations(page, "CI archived log");
  });

  test("opaque next-page navigation replaces the page and browser Back restores newest", async ({ page }) => {
    await devLogin(page);
    await page.goto("/ci?limit=1");

    await expect(page.getByTestId("ci-run-row")).toHaveCount(1);
    await expect(page.getByTitle("State: Failed")).toBeVisible();
    const next = page.getByTestId("ci-runs-next");
    await expect(next).toHaveAttribute("href", /cursor=cr1_[A-Za-z0-9_-]+/);
    await next.click();

    await expect(page).toHaveURL(/\/ci\?limit=1&cursor=cr1_[A-Za-z0-9_-]+/);
    await expect(page.getByTestId("ci-run-row")).toHaveCount(1);
    await expect(page.getByTitle("State: Running")).toBeVisible();
    await expect(page.getByTitle("State: Failed")).toHaveCount(0);

    await page.goBack();
    await expect(page).toHaveURL(/\/ci\?limit=1$/);
    await expect(page.getByTitle("State: Failed")).toBeVisible();
  });

  test("a visibility-stale cursor returns 409 and reload restarts at the latest authorized run", async ({ page }) => {
    await devLogin(page);
    await page.goto("/ci?limit=1");
    await expect(page.getByTestId("ci-runs-next")).toBeVisible();

    await setCiConfig({ addCiVisibleRepo: true });
    await page.getByTestId("ci-runs-next").click();
    await expect(page.getByTestId("ci-error")).toHaveAttribute("data-kind", "stale");
    await expect(page.getByText("The run list changed")).toBeVisible();

    await page.getByRole("link", { name: "Reload latest runs" }).click();
    await expect(page).toHaveURL(/\/ci\?limit=1$/);
    await expect(page).not.toHaveURL(/(?:\?|&)cursor=/);
    await expect(page.getByTitle("State: Failed")).toBeVisible();
  });

  test("empty, unavailable, log-unavailable, and leak-free absent states remain distinct", async ({ page }) => {
    await setCiConfig({ emptyCi: true });
    await devLogin(page);
    await page.goto("/ci");
    await expect(page.getByTestId("ci-runs-empty")).toContainText("No authorized runs yet");

    await setCiConfig({ emptyCi: false, ciUnavailable: true });
    await page.reload();
    await expect(page.getByTestId("ci-error")).toHaveAttribute("data-kind", "unavailable");
    await expect(page.getByText("CI data is unavailable")).toBeVisible();

    await setCiConfig({ ciUnavailable: false });
    await page.goto("/ci/runs/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa");
    await expect(page.getByTestId("ci-error")).toHaveAttribute("data-kind", "not-found");
    await expect(page.getByText("This run is not available to you")).toBeVisible();

    await setCiConfig({ ciLogUnavailable: true });
    await page.goto(
      `/ci/runs/${FAILED_RUN}?job=92000000-0000-4000-8000-000000000001#archived-log`,
    );
    await expect(page.getByRole("heading", { level: 3, name: "contract" })).toBeVisible();
    await expect(page.getByTestId("ci-error")).toHaveAttribute("data-kind", "unavailable");
    await expect(page.getByText("CI data is unavailable")).toBeVisible();
  });

  test("375px layout keeps state, repository, filter, and archived output without page overflow", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await devLogin(page);
    await page.goto("/ci");
    await expect(page.getByTitle("State: Failed")).toBeVisible();
    await expect(page.getByText("alpha", { exact: true }).first()).toBeVisible();
    await expect(page.getByLabel("Run state")).toBeVisible();
    const overflow = await page.evaluate(() =>
      document.documentElement.scrollWidth - document.documentElement.clientWidth
    );
    expect(overflow).toBeLessThanOrEqual(1);
    await expectNoAxeViolations(page, "CI mobile run list");
  });
});
