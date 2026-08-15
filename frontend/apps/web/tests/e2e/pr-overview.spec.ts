import { test, expect, request as pwRequest, type Locator, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { DEV_ACCESS_TOKEN } from "../../dev-edge/dev-contract.mjs";

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;

async function resetPrFixtures() {
  const context = await pwRequest.newContext();
  const response = await context.post(`${EDGE}/__test/config`, {
    data: { resetPrFixtures: true },
  });
  expect(response.ok(), "dev-edge PR fixture reset must be accepted").toBeTruthy();
  await context.dispose();
}

async function configurePr(data: {
  prCommitContinuationFailures?: number;
  prCommitContinuationMalformedPages?: number;
  prMutationResponseLosses?: number;
}) {
  const context = await pwRequest.newContext();
  const response = await context.post(`${EDGE}/__test/config`, { data });
  expect(response.ok(), "dev-edge PR config must be accepted").toBeTruthy();
  const body = await response.json() as {
    state: { prCommitContinuationRequests: number };
  };
  await context.dispose();
  return body.state;
}

async function prContinuationRequestCount() {
  return (await configurePr({})).prCommitContinuationRequests;
}

async function commitOids(list: Locator) {
  return list.locator("li").evaluateAll((rows) =>
    rows.map((row) => row.getAttribute("data-commit-oid")),
  );
}

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
  await page.waitForLoadState("networkidle");
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
}

test.describe("R3.3 PR overview + context pane — real browser", () => {
  test.use({ viewport: { width: 1440, height: 900 } });
  test.beforeEach(resetPrFixtures);
  test.afterEach(resetPrFixtures);

  test("the overview renders header, checks panel, blocked merge card, and the shell context pane", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1");

    // The header uses the title and visible state label.
    await expect(page.getByRole("heading", { level: 1, name: /R3.3 PR overview \+ context pane/ })).toBeVisible();
    await expect(page.getByTitle("State: Open").first()).toBeVisible();

    // The checks panel lists the required contexts + the fork-trust note (a fork green never gates).
    await expect(page.getByTestId("checks-panel")).toBeVisible();
    await expect(page.getByTestId("fork-trust")).toBeVisible();

    // The merge card is BLOCKED (gate not admitted) — no merge button, the blocked reasons listed.
    await expect(page.getByTestId("merge-blocked")).toBeVisible();
    await expect(page.getByTestId("merge-button")).toHaveCount(0);

    // The shell-owned context pane renders as the 4th region (wide viewport) with its landmark.
    const contextPane = page.getByTestId("context-pane");
    await expect(contextPane).toBeVisible();
    await expect(page.getByRole("complementary", { name: "Pull request context" })).toBeVisible();
    const issue = contextPane.getByRole("link", { name: /MYL-102.*Close the collaboration feedback loop/ });
    await expect(issue).toHaveAttribute("href", "/issues/00000000-0000-4000-8000-000000000102");
    const document = contextPane.getByRole("link", { name: /Engineering principles/ });
    await expect(document).toHaveAttribute("href", /\/knowledge\?page=[0-9A-HJKMNP-TV-Z]{26}$/);

    await expectNoAxeViolations(page, "PR overview");
  });

  test("PR commits append a distinct snapshot page and reset across navigation", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1");

    const list = page.getByTestId("pr-commits-list");
    await expect(list.locator("li")).toHaveCount(20);
    const loadOlder = page.getByTestId("load-older-commits");
    await expect(loadOlder).toHaveText("Load older commits");
    await loadOlder.click();

    await expect(list.locator("li")).toHaveCount(23);
    await expect(list.getByText("PR continuation commit 1", { exact: true })).toBeVisible();
    await expect(loadOlder).toHaveCount(0);
    const completed = page.getByTestId("commits-pagination-complete");
    await expect(completed).toHaveText("All commits loaded.");
    await expect(completed).toBeFocused();
    const oids = await list.locator("li").evaluateAll((rows) =>
      rows.map((row) => row.getAttribute("data-commit-oid")),
    );
    expect(new Set(oids).size).toBe(23);

    await page.goto("/git/repos/myelin/prs/2");
    await expect(page.getByTestId("pr-commits-list").locator("li")).toHaveCount(1);
    await expect(page.getByTestId("load-older-commits")).toHaveCount(0);
    await expect(page.getByTestId("commits-pagination-complete")).toHaveCount(0);
    await page.goto("/git/repos/myelin/prs/1");
    await expect(page.getByTestId("pr-commits-list").locator("li")).toHaveCount(20);
    await expect(page.getByTestId("load-older-commits")).toBeVisible();
    await expect(page.getByTestId("commits-pagination-complete")).toHaveCount(0);
  });

  test("a failed continuation retry evicts only that rejected page and preserves the first 20 commits", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1");
    const list = page.getByTestId("pr-commits-list");
    await expect(list.locator("li")).toHaveCount(20);
    const firstTwentyOids = await commitOids(list);
    await configurePr({ prCommitContinuationFailures: 1 });

    await page.getByTestId("load-older-commits").click();
    await expect(page.getByText("Older commits could not be loaded.", { exact: false })).toBeVisible();
    await expect(list.locator("li")).toHaveCount(20);
    expect(await commitOids(list)).toEqual(firstTwentyOids);
    expect(await prContinuationRequestCount()).toBe(1);

    await page.getByRole("button", { name: "Retry loading older commits" }).click();
    await expect(list.locator("li")).toHaveCount(23);
    expect((await commitOids(list)).slice(0, 20)).toEqual(firstTwentyOids);
    expect(await prContinuationRequestCount()).toBe(2);
    await expect(page.getByTestId("commits-pagination-complete")).toBeFocused();
  });

  test("a retry evicts a cached successful continuation that fails local page validation", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/1");
    const list = page.getByTestId("pr-commits-list");
    await expect(list.locator("li")).toHaveCount(20);
    const firstTwentyOids = await commitOids(list);
    await configurePr({ prCommitContinuationMalformedPages: 1 });

    await page.getByTestId("load-older-commits").click();
    await expect(page.getByText("Older commits could not be loaded.", { exact: false })).toBeVisible();
    await expect(list.locator("li")).toHaveCount(20);
    expect(await commitOids(list)).toEqual(firstTwentyOids);
    expect(await prContinuationRequestCount()).toBe(1);

    await page.getByRole("button", { name: "Retry loading older commits" }).click();
    await expect(list.locator("li")).toHaveCount(23);
    expect((await commitOids(list)).slice(0, 20)).toEqual(firstTwentyOids);
    expect(await prContinuationRequestCount()).toBe(2);
    await expect(page.getByTestId("commits-pagination-complete")).toBeFocused();
  });

  test("the dev edge distinguishes malformed and scope-mismatched cursors from expired snapshots", async () => {
    const context = await pwRequest.newContext({
      extraHTTPHeaders: { authorization: `Bearer ${DEV_ACCESS_TOKEN}` },
    });
    const first = await context.get(`${EDGE}/v1/git/repos/myelin/prs/1/commits?limit=20`);
    expect(first.status()).toBe(200);
    const firstBody = await first.json() as { page: { next_cursor: string } };
    const cursor = firstBody.page.next_cursor;
    const frame = Buffer.from(cursor.slice(4), "base64url");
    frame[54] = (frame[54] ?? 0) ^ 0xff;
    const expiredCursor = `pc1_${frame.toString("base64url")}`;

    const expired = await context.get(
      `${EDGE}/v1/git/repos/myelin/prs/1/commits?cursor=${expiredCursor}&limit=20`,
    );
    expect(expired.status()).toBe(409);
    await expect(expired.json()).resolves.toEqual({
      error: { message: "pull request commit cursor expired", code: "conflict" },
    });
    const wrongScope = await context.get(
      `${EDGE}/v1/git/repos/myelin/prs/2/commits?cursor=${cursor}&limit=20`,
    );
    expect(wrongScope.status()).toBe(400);
    await context.dispose();
  });

  test("a checks-projection failure degrades LOCALLY — the PR stays live", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/3"); // PR #3's checks route 404s (fixture).

    // The PR title and discussion remain available.
    await expect(page.getByRole("heading", { level: 1, name: /Checks-degrade fixture/ })).toBeVisible();
    await expect(page.getByTestId("discussion")).toBeVisible();
    // Checks and merge controls each show their scoped unavailable state.
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

  test("a response-lost discussion retry keeps one durable comment", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/4");
    await page.waitForLoadState("networkidle");
    await configurePr({ prMutationResponseLosses: 1 });

    const discussion = page.getByTestId("discussion");
    const box = discussion.getByLabel("New comment");
    const comment = discussion.getByText("One comment after uncertain delivery.");
    await box.fill("One comment after uncertain delivery.");
    await page.getByTestId("post-thread").click();

    await expect(page.getByText("Comment not confirmed — retrying this unchanged draft is safe"))
      .toBeVisible();
    await expect(box).toHaveText("One comment after uncertain delivery.");
    await page.getByTestId("post-thread").click();
    await expect(comment).toHaveCount(1);

    await page.reload();
    await page.waitForLoadState("networkidle");
    await expect(page.getByTestId("discussion").getByText("One comment after uncertain delivery."))
      .toHaveCount(1);
  });

  test("an in-progress review batch RESUMES on reload — 'Start a review' does not double-create (finding #18)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/4"); // isolated fixture — no other test drives PR #4's reviews.
    await page.waitForLoadState("networkidle");

    // Start a review → the server records an in_progress batch owned by the viewer.
    await page.getByTestId("start-review").click();
    await expect(page.getByTestId("review-batch")).toBeVisible();

    // Reload: the draft is a LOCAL signal, but the server still returns the in_progress batch. The page
    // must rehydrate it (resume) instead of showing "Start a review" again (which would double-create).
    await page.reload();
    await page.waitForLoadState("networkidle");
    await expect(page.getByTestId("review-batch")).toBeVisible();
    await expect(page.getByTestId("start-review")).toHaveCount(0);

    // Clean up the shared per-process fixture so it can't leak into another test.
    await page.getByRole("button", { name: "Discard" }).click();
    await expect(page.getByTestId("start-review")).toBeVisible();
  });

  test("a discussion composer CLEARS after a successful post — no duplicate on a second submit (finding #19)", async ({ page }) => {
    await devLogin(page);
    await page.goto("/git/repos/myelin/prs/4"); // isolated fixture.
    await page.waitForLoadState("networkidle");

    const box = page.getByLabel("New comment");
    const matchingComments = page.getByTestId("discussion").getByText("One-shot comment.");
    const baseline = await matchingComments.count();
    await box.fill("One-shot comment.");
    const submit = page.getByTestId("post-thread");
    await submit.click();
    await expect(matchingComments).toHaveCount(baseline + 1);

    // The controlled composer is now empty (the signal cleared AND the DOM followed) — a stale DOM value
    // was the finding: a second click would duplicate-post. Empty field ⇒ the second post is a no-op.
    await expect(box).toHaveText("");
    await expect(submit).toBeDisabled();
    await submit.evaluate((button) => {
      if (!(button instanceof HTMLButtonElement)) {
        throw new Error("post-thread must resolve to a button");
      }
      button.click();
    });
    await expect(matchingComments).toHaveCount(baseline + 1);
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
