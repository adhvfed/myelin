import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

// Browser coverage for repository and cross-repository PR lists, including keyboard navigation,
// empty and restricted states, and accessibility.

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

test.describe("R3.1 PR list + navigation front door — real browser", () => {
  test("the repo-home 'Pull requests' link is the front door into the per-repo list", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin");
    // The nav link mirrors the Commits link (ux-git critical #1: PRs were unreachable).
    await page.getByRole("link", { name: "Pull requests" }).first().click();
    await page.waitForURL("**/git/repos/myelin/prs");

    await expect(page.getByRole("heading", { name: "Pull requests", level: 1 })).toBeVisible();
    const rows = page.getByTestId("pr-row");
    await expect(rows.first()).toBeVisible();
    // Includes a titled row, an agent-authored row, and the #number fallback.
    await expect(page.getByText("R2.4 MCP HITL server-side verdicts")).toBeVisible();
    await expect(page.getByText("AuthzScanner: eliminate 2 residual reach-arounds")).toBeVisible();
    // Status includes a visible label.
    await expect(page.getByTitle("State: Open").first()).toBeVisible();
    await expect(page.getByText("all passing").first()).toBeVisible();
    await expect(page.getByText("1 running").first()).toBeVisible();

    await expectNoAxeViolations(page, "per-repo PR list");
  });

  test("the list is a roving-tabindex composite: focus a row, arrow to the next, Enter opens it", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs?state=all");
    const rows = page.getByTestId("pr-row");
    await expect(rows.first()).toBeVisible();
    await page.waitForLoadState("networkidle"); // let hydration settle before driving the keyboard
    // `press` focuses the row then dispatches the key → our roving handler moves focus to the next.
    await rows.first().press("ArrowDown"); // re-rove to the next row
    await expect(rows.nth(1)).toBeFocused();
    await rows.nth(1).press("j"); // j/k alias
    await expect(rows.nth(2)).toBeFocused();
    await rows.nth(2).press("Enter"); // Enter opens the focused PR (the row is a real link)
    await page.waitForURL(/\/git\/repos\/myelin\/prs\/\d+$/);
  });

  test("the tab filter narrows the list; Merged shows the merged PR", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs");
    await page.getByRole("tab", { name: /Merged/ }).click();
    await page.waitForURL("**/git/repos/myelin/prs?state=merged");
    // PR #39 is a legacy record with no title and uses the #number fallback.
    await expect(page.getByText("#39")).toBeVisible();
    await expect(page.getByText("merged green")).toBeVisible();
  });

  test("the empty state teaches the next action (distinct from a filtered no-result)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/sandbox/prs");
    const empty = page.getByTestId("prs-empty");
    await expect(empty).toContainText("No open pull requests");
    await expect(empty).toContainText("git switch -c my-change");
    await expectNoAxeViolations(page, "PR list empty state");
  });

  test("a repo the viewer cannot access is the dignified restricted state (no leak)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/secret/prs");
    await expect(page.getByTestId("prs-restricted")).toContainText("not available to you");
    await expectNoAxeViolations(page, "PR list no-access");
  });

  test("the cross-repo front door shows both buckets across repos", async ({ page }) => {
    await devLogin(page);
    // Reached from the Code landing header link.
    await page.goto("/git/repos");
    await page.getByRole("link", { name: "Your pull requests" }).click();
    await page.waitForURL("**/prs");

    await expect(page.getByRole("heading", { name: "Your pull requests", level: 1 })).toBeVisible();
    const review = page.getByTestId("bucket-needs-review");
    await expect(review).toContainText("Needs your review");
    await expect(review.getByText("AuthzScanner: eliminate 2 residual reach-arounds")).toBeVisible();
    await expect(review.getByText("review requested").first()).toBeVisible();
    const yours = page.getByTestId("bucket-yours");
    await expect(yours).toContainText("Your PRs");
    await expect(yours.getByText("R2.4 MCP HITL server-side verdicts")).toBeVisible();

    await expectNoAxeViolations(page, "cross-repo PR front door");
  });
});
