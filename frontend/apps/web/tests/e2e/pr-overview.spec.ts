import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

// R3.3 — the PR overview + context pane (G-6) + review verdicts (G-8) + checks panel (G-9), driven in
// a real chromium against the dev-edge contract. Proves: the overview renders the pane + checks +
// discussion + merge card; a checks-projection failure degrades LOCALLY (the PR stays live, never
// "PR not available"); the batched review submits; the merge ConfirmDialog is an alertdialog.

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

test.describe("R3.3 PR overview + context pane — real browser", () => {
  test.use({ viewport: { width: 1440, height: 900 } });

  test("the overview renders header, checks panel, blocked merge card, and the shell context pane", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1");

    // Header: the title (not the #number fallback) + the state pill (TEXT, not colour-only).
    await expect(page.getByRole("heading", { level: 1, name: /R3.3 PR overview \+ context pane/ })).toBeVisible();
    await expect(page.getByTitle("State: Open").first()).toBeVisible();

    // The checks panel lists the required contexts + the fork-trust note (a fork green never gates).
    await expect(page.getByTestId("checks-panel")).toBeVisible();
    await expect(page.getByTestId("fork-trust")).toBeVisible();

    // The merge card is BLOCKED (gate not admitted) — no merge button, the blocked reasons listed.
    await expect(page.getByTestId("merge-blocked")).toBeVisible();
    await expect(page.getByTestId("merge-button")).toHaveCount(0);

    // The shell-owned context pane renders as the 4th region (wide viewport) with its landmark.
    await expect(page.getByTestId("context-pane")).toBeVisible();
    await expect(page.getByRole("complementary", { name: "Pull request context" })).toBeVisible();

    await expectNoAxeViolations(page, "PR overview");
  });

  test("a checks-projection failure degrades LOCALLY — the PR stays live", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/3"); // PR #3's checks route 404s (fixture).

    // The PR itself is fully rendered (title, discussion) — it is NOT "PR not available".
    await expect(page.getByRole("heading", { level: 1, name: /Checks-degrade fixture/ })).toBeVisible();
    await expect(page.getByTestId("discussion")).toBeVisible();
    // The checks region degrades to a scoped "Checks unavailable"; the merge card degrades honestly.
    await expect(page.getByTestId("checks-unavailable")).toBeVisible();
    await expect(page.getByText(/Checks unavailable/)).toBeVisible();
    await expect(page.getByTestId("merge-degraded")).toBeVisible();
    // The scoped-failure page is still axe-clean.
    await expectNoAxeViolations(page, "PR overview (checks degraded)");
  });

  test("the batched review bar submits a verdict (G-8)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1");
    await page.waitForLoadState("networkidle"); // let the reviews section hydrate before driving it

    await page.getByTestId("start-review").click();
    await expect(page.getByTestId("review-batch")).toBeVisible();
    await page.getByLabel("Pending review comment").click();
    await page.getByLabel("Pending review comment").pressSequentially("Please rename this symbol.");
    await page.getByRole("button", { name: "Add comment" }).click();
    await page.getByTestId("open-verdict").click();
    await expect(page.getByTestId("verdict-panel")).toBeVisible();
    await page.getByTestId("verdict-comment").click();
    // The submitted review appears in the list (the batch closed).
    await expect(page.getByTestId("reviews").getByText("Commented")).toBeVisible();
  });

  test("posting to the discussion appends the comment", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1");
    await page.waitForLoadState("networkidle");
    await page.getByLabel("New comment").click();
    await page.getByLabel("New comment").pressSequentially("Kicking off the discussion.");
    await page.getByTestId("post-thread").click();
    await expect(page.getByTestId("discussion").getByText("Kicking off the discussion.")).toBeVisible();
  });

  test("a mergeable PR opens the merge ConfirmDialog (alertdialog, safe-action focus)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/2"); // gate admitted.
    await page.waitForLoadState("networkidle");
    await expect(page.getByTestId("merge-ready")).toBeVisible();
    await page.getByTestId("merge-button").click();
    // The confirm is an alertdialog (consequential) with the safe Cancel default-focused.
    const dlg = page.getByRole("alertdialog");
    await expect(dlg).toBeVisible();
    await expect(dlg.getByRole("button", { name: "Cancel" })).toBeFocused();
  });
});
