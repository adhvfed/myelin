import { expect, request as pwRequest, test, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;
const OPEN_ID = "00000000-0000-4000-8000-000000000102";

async function setEdgeConfig(cfg: {
  resetIssues?: boolean;
  emptyIssues?: boolean;
  issuesUnavailable?: boolean;
  issueActivationPolls?: number;
}) {
  const context = await pwRequest.newContext();
  const response = await context.post(`${EDGE}/__test/config`, { data: cfg });
  expect(response.ok(), "dev-edge Issues config must be accepted").toBeTruthy();
  await context.dispose();
}

async function devLogin(page: Page) {
  await page.goto("/login");
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

test.beforeEach(async () => setEdgeConfig({ resetIssues: true }));
test.afterEach(async () => setEdgeConfig({ resetIssues: true }));

test.describe("R4.4 founder Issues web floor", () => {
  test("unauthenticated issue detail preserves the 401 to login floor", async ({ page }) => {
    await page.goto(`/issues/${OPEN_ID}`);
    await page.waitForURL("**/login");
    await expect(page.getByTestId("dev-login")).toBeVisible();
  });

  test("list, key search, state tabs, and roving rows stay authoritative and keyboard-operable", async ({ page }) => {
    await devLogin(page);
    await page.goto("/issues");
    await expect(page.getByRole("heading", { level: 1, name: "Issues" })).toBeVisible();
    await expect(page.getByText("Close the founder feedback loop")).toBeVisible();
    await expect(page.getByTitle("State: Todo").first()).toBeVisible();
    await expectNoAxeViolations(page, "Issues list");

    const rows = page.getByTestId("issue-row");
    await expect(rows.first()).toHaveAttribute("tabindex", "0");
    await rows.first().press("ArrowDown");
    await expect(rows.nth(1)).toBeFocused();
    await rows.nth(1).press("j");
    await expect(rows.nth(2)).toBeFocused();

    await page.getByRole("tab", { name: "Closed" }).click();
    await page.waitForURL("**/issues?state=closed");
    await expect(page.getByText("Retire the ledger workaround")).toBeVisible();
    await expect(page.getByTestId("issue-row").first()).toHaveAttribute("tabindex", "0");

    await page.getByLabel("Find by issue key").fill("myl-101");
    await page.getByRole("button", { name: "Find" }).click();
    await page.waitForURL("**/issues?state=closed&key=MYL-101");
    await expect(page.getByTestId("issues-no-results")).toBeVisible();

    await page.getByRole("tab", { name: "All" }).click();
    await page.waitForURL("**/issues?state=all&key=MYL-101");
    await expect(page.getByText("Verify encrypted issue titles")).toBeVisible();
    await expect(page.getByText("Close the founder feedback loop")).toHaveCount(0);
  });

  test("opaque load-more appends the final authorized row without totals", async ({ page }) => {
    await devLogin(page);
    await page.goto("/issues");
    await expect(page.getByTestId("issue-row")).toHaveCount(50);
    await page.getByRole("button", { name: "Load more" }).click();
    await expect(page.getByTestId("issue-row")).toHaveCount(51);
    await expect(page.getByText(/of \d+/)).toHaveCount(0);
  });

  test("quick capture is focus-safe from both the button and command palette", async ({ page }) => {
    await devLogin(page);
    await page.goto("/issues");
    await page.waitForLoadState("networkidle");
    const trigger = page.getByRole("button", { name: "New issue" });
    await trigger.focus();
    await trigger.press("Enter");
    const dialog = page.getByRole("dialog", { name: "New issue" });
    await expect(dialog).toBeVisible();
    await expect(page.getByLabel("Title")).toBeFocused();
    await expectNoAxeViolations(page, "New issue dialog");
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(trigger).toBeFocused();

    await page.keyboard.press("Meta+k");
    const command = page.getByRole("combobox", { name: /Search or run a command/ });
    await command.fill("Create issue");
    await command.press("Enter");
    await page.waitForURL("**/issues?new=1");
    await expect(page.getByRole("dialog", { name: "New issue" })).toBeVisible();
    await expect(page.getByLabel("Title")).toBeFocused();
  });

  test("202 create remains visibly pending until fresh polling observes active", async ({ page }) => {
    await setEdgeConfig({ issueActivationPolls: 2 });
    await devLogin(page);
    await page.goto("/issues");
    await page.getByRole("button", { name: "New issue" }).click();
    await page.getByLabel("Title").fill("Capture the browser rough edge");
    await page.getByRole("button", { name: "Create issue" }).click();

    const pending = page.getByTestId("pending-issue");
    await expect(pending).toContainText("Activating access");
    await expect(pending).not.toContainText("Capture the browser rough edge");
    await expect(page.getByText(/accepted — activating access/)).toBeVisible();

    await expect(page.getByText("Capture the browser rough edge")).toBeVisible();
    await expect(pending).toHaveCount(0);
    await expect(page.getByText(/is ready/)).toBeVisible();
  });

  test("detail closes only after a safe-focus confirmation", async ({ page }) => {
    await devLogin(page);
    await page.goto(`/issues/${OPEN_ID}`);
    await expect(page.getByRole("heading", { name: "Close the founder feedback loop" })).toBeVisible();
    await expectNoAxeViolations(page, "Issue detail");
    await page.getByRole("button", { name: "Close issue" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
    await dialog.getByRole("button", { name: "Close issue" }).click();
    await expect(page.getByTitle("State: Done")).toBeVisible();
    await expect(page.getByRole("button", { name: "Close issue" })).toHaveCount(0);
    await expect(page.getByText("MYL-102 closed")).toBeVisible();
  });

  test("empty, projection-unavailable, and leak-free not-available states stay distinct", async ({ page }) => {
    await setEdgeConfig({ emptyIssues: true });
    await devLogin(page);
    await page.goto("/issues");
    await expect(page.getByTestId("issues-empty")).toContainText("No issues yet");

    await setEdgeConfig({ issuesUnavailable: true });
    await page.reload();
    await expect(page.getByTestId("issues-error")).toContainText("authorization is catching up");

    await setEdgeConfig({ issuesUnavailable: false });
    await page.goto("/issues/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    await expect(page.getByTestId("issue-detail-error")).toContainText("not available to you");
  });

  test("375px layout retains key, title, state, and actions without horizontal overflow", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 812 });
    await devLogin(page);
    await page.goto("/issues");
    await expect(page.getByText("MYL-102", { exact: true })).toBeVisible();
    await expect(page.getByText("Close the founder feedback loop")).toBeVisible();
    await expect(page.getByTitle("State: Todo").first()).toBeVisible();
    await expect(page.getByRole("button", { name: "New issue" })).toBeVisible();
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(1);
    await expectNoAxeViolations(page, "Issues mobile layout");
  });
});
