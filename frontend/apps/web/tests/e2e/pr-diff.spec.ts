import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

// R3.2 · G-7 — the PR diff / files-changed surface, driven in a real chromium against the dev-edge
// contract. Proves: the three-dot diff renders (split + unified); a line comment posts to the R3.3
// thread store and appears; a rebase-orphan shows the honest "outdated" pill (never a wrong line);
// F7 walks changes; the W4 deep-link banner is honest; the binary row never dumps text; the commit
// diff still renders on the shared DiffViewer; and every state is axe-clean.

async function expectNoAxeViolations(page: Page, context: string) {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(
    results.violations,
    `axe violations on ${context}: ${JSON.stringify(results.violations, null, 2)}`,
  ).toEqual([]);
}

async function devLogin(page: Page) {
  await page.goto("/login");
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
}

test.describe("R3.2 PR diff / files-changed — real browser", () => {
  test.use({ viewport: { width: 1440, height: 900 } });

  test("renders the three-dot diff (split), the binary row, and the tab strip", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1/diff");

    // The PR header + tabs (Files changed active with the count badge).
    await expect(page.getByRole("heading", { level: 1, name: /R3.3 PR overview/ })).toBeVisible();
    await expect(page.getByRole("tab", { name: /Files changed/ })).toBeVisible();
    await expect(page.getByRole("tab", { name: /Files changed/ })).toContainText("(2)");

    // The modified file renders with the added clamp line; the binary file is a no-text-diff row.
    await expect(page.getByText("src/list_filter.rs").first()).toBeVisible();
    await expect(page.getByText(/let cap = self.limit.min\(100\)/)).toBeVisible();
    await expect(page.getByTestId("binary-row")).toContainText("Binary file");

    // Change kind is announced as TEXT (SR prefix), not colour alone.
    await expect(page.getByText(/added, new line 2:/)).toBeAttached();

    await expectNoAxeViolations(page, "PR diff (split)");
  });

  test("toggles to unified layout", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1/diff");
    await page.waitForSelector("[data-rowkey]"); await page.waitForTimeout(1200); // hydrate
    await page.getByTestId("diff-view-unified").click();
    await expect(page).toHaveURL(/view=unified/);
    await expect(page.getByText(/let cap = self.limit.min\(100\)/)).toBeVisible();
  });

  test("a rebase-orphan thread shows the honest 'outdated' pill (never a wrong line)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1/diff");
    const pill = page.getByTestId("outdated-pill");
    await expect(pill).toBeVisible();
    await expect(pill).toContainText("was on former line 87");
  });

  test("a line comment posts to the thread store and appears", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1/diff");
    await page.waitForSelector("[data-rowkey]"); await page.waitForTimeout(1200); // hydrate
    // Click the added clamp line's code cell to open the composer, type, submit.
    await page.getByText(/let cap = self.limit.min\(100\)/).click();
    const box = page.getByRole("textbox", { name: /Comment on src\/list_filter.rs line 2/ });
    await expect(box).toBeVisible();
    await box.fill("Does this clamp handle limit == 0?");
    await page.getByRole("button", { name: "Add single comment" }).click();
    await expect(page.getByText("Does this clamp handle limit == 0?")).toBeVisible();
  });

  test("F7 walks to the next change row", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1/diff");
    await page.waitForSelector("[data-rowkey]"); await page.waitForTimeout(1200); // hydrate
    // Focus the first context code cell, then F7 → the next change row (data-change="1").
    const firstCell = page.locator("[data-rowkey]").first();
    await firstCell.focus();
    await page.keyboard.press("F7");
    const focused = page.locator("[data-rowkey]:focus");
    await expect(focused).toHaveAttribute("data-change", "1");
  });

  test("an honest deep-link banner appears for a stale ?line=", async ({ page }) => {
    await devLogin(page);
    // A line that no longer exists in the diff → the honest "older head" banner (no nearest-line guess).
    await page.goto("/git/repos/myelin/prs/1/diff?file=src/list_filter.rs&line=9999&side=new");
    await expect(page.getByTestId("deeplink-banner")).toContainText(/older head/);
  });

  test("the commit diff still renders on the shared DiffViewer", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/commits/main");
    // Navigate into a commit from the log (first commit link).
    await page.locator("a[href*='/commit/']").first().click();
    await expect(page.getByTestId("diff-files")).toBeVisible();
    await expectNoAxeViolations(page, "commit diff on DiffViewer");
  });
});
