import { test, expect, request as pwRequest, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;

async function resetPrFixtures() {
  const context = await pwRequest.newContext();
  const response = await context.post(`${EDGE}/__test/config`, {
    data: { resetPrFixtures: true },
  });
  expect(response.ok(), "dev-edge PR fixture reset must be accepted").toBeTruthy();
  await context.dispose();
}

// Browser coverage for PR files: split/unified rendering, comments, stale anchors, keyboard
// navigation, deep links, binary files, the shared commit viewer, and accessibility.

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
  await page.waitForLoadState("networkidle");
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
}

test.describe("R3.2 PR diff / files-changed — real browser", () => {
  test.use({ viewport: { width: 1440, height: 900 } });
  test.beforeEach(resetPrFixtures);
  test.afterEach(resetPrFixtures);

  test("renders the three-dot diff (split), the binary row, and the tab strip", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1/diff");

    // The PR section navigation (Files changed is current and carries the count badge).
    await expect(page.getByRole("heading", { level: 1, name: /R3.3 PR overview/ })).toBeVisible();
    const filesChanged = page.getByRole("link", { name: /Files changed/ });
    await expect(filesChanged).toHaveAttribute("aria-current", "page");
    await expect(filesChanged).toContainText("(2)");

    // The modified file renders with the added clamp line; the binary file is a no-text-diff row.
    await expect(page.getByText("src/list_filter.rs").first()).toBeVisible();
    await expect(page.getByText(/let cap = self.limit.min\(100\)/)).toBeVisible();
    await expect(page.getByTestId("binary-row")).toContainText("Binary file");

    // The screen-reader prefix announces the change kind.
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

  test("expands a collapsed gap from the immutable new-side blob", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1/diff");
    const expand = page.getByTestId("expand-all");
    await expect(expand).toBeVisible();
    await expand.click();
    await expect(page.getByText("// context line 5").first()).toBeVisible();
    await expect(expand).toHaveCount(0);
    await expect(page.getByTestId("diff-live")).toContainText("Expanded 15 unchanged lines");
    await expectNoAxeViolations(page, "PR diff with expanded context");
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

  test("a line-comment draft belongs to one repository and pull request", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1/diff");
    await page.waitForSelector("[data-rowkey]");
    await page.waitForTimeout(1_200);

    await page.getByText(/let cap = self.limit.min\(100\)/).click();
    await page.getByRole("textbox", { name: /Comment on src\/list_filter.rs line 2/ })
      .fill("This draft belongs to the first repository.");

    await page.evaluate(() => {
      window.history.pushState({}, "", "/git/repos/platform%2Fmyelin/prs/1/diff");
      window.dispatchEvent(new PopStateEvent("popstate"));
    });
    await page.waitForURL("**/git/repos/platform%2Fmyelin/prs/1/diff");
    await expect(page.getByRole("link", { name: "platform/myelin", exact: true })).toBeVisible();
    await expect(page.getByText("This draft belongs to the first repository.")).toHaveCount(0);
    await expect(page.getByRole("textbox", { name: /Comment on src\/list_filter.rs line 2/ }))
      .toHaveCount(0);
  });

  test("commenting on a DELETED (old-side) line in split view opens the composer + posts (finding #16)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1/diff");
    await page.waitForSelector("[data-rowkey]"); await page.waitForTimeout(1200); // hydrate
    // Split view is the default at 1440px. Click the DELETED line's LEFT (old-side) cell — before the
    // fix the widget row anchored to the NEW side only, so this click silently no-op'd.
    const deleted = page.locator('[data-side="old"]', { hasText: "let cap = 50;" });
    await expect(deleted).toBeVisible();
    await deleted.click();
    const box = page.getByRole("textbox", { name: /Comment on src\/list_filter.rs line 2/ });
    await expect(box).toBeVisible();
    await box.fill("Why drop the old default here?");
    await page.getByRole("button", { name: "Add single comment" }).click();
    await expect(page.getByText("Why drop the old default here?")).toBeVisible();
  });

  test("a >50-file PR pages to the next set via 'Load remaining files' (finding #15)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/4/diff");
    await page.waitForSelector("[data-rowkey]"); await page.waitForTimeout(600);
    // Page 1 renders files 0–49 and reports the ten remaining files.
    await expect(page.getByText("src/paged/file_000.txt").first()).toBeVisible();
    await expect(page.getByTestId("load-remaining")).toContainText("10 more files weren't rendered");
    // A file from page 2 is absent until the next page loads.
    await expect(page.getByText("src/paged/file_055.txt")).toHaveCount(0);
    // Click the link → the cursor threads into getPrDiff and page 2 loads (files 50–59).
    await page.getByRole("link", { name: "Load remaining files" }).click();
    await expect(page).toHaveURL(/cursor=c50/);
    await expect(page.getByText("src/paged/file_055.txt").first()).toBeVisible();
    // Page 2 is the last page — no further paging link.
    await expect(page.getByTestId("load-remaining")).toHaveCount(0);
  });

  test("an oversized PR diff renders a calm capacity state on a direct load", async ({ page }) => {
    await devLogin(page);
    const response = await page.goto("/git/repos/myelin/prs/5/diff");
    expect(response?.status()).toBe(200);
    const state = page.locator('[data-testid="repo-error"][data-kind="diff-too-large"]');
    await expect(state).toBeVisible();
    await expect(state.getByRole("heading", { level: 2 }))
      .toHaveText("This diff is too large for browser review");
    await expect(state.getByRole("link", { name: "Pull request overview" }))
      .toHaveAttribute("href", "/git/repos/myelin/prs/5");
    await expectNoAxeViolations(page, "oversized PR diff");
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
    // A missing line reports an older head instead of selecting the nearest line.
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
