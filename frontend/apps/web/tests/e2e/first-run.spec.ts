import { test, expect, type Page, request as pwRequest } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

// Browser coverage for login, empty-tenant onboarding, inbox, and accessibility. Tests configure
// the local edge through `POST /__test/config` and restore its defaults after each case.

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;

async function setEdgeConfig(cfg: {
  emptyRepos?: boolean;
  repoCreateResponseLosses?: number;
  devLoginEnabled?: boolean;
  whoamiUnavailable?: boolean;
  seedInboxAgentApproval?: boolean;
  inboxMutationUnavailable?: boolean;
  inboxPagination?: boolean;
  inboxListCursorDelaysMs?: number[];
  inboxListCursorFailures?: number;
}): Promise<void> {
  const ctx = await pwRequest.newContext();
  const res = await ctx.post(`${EDGE}/__test/config`, { data: cfg });
  expect(res.ok(), "dev-edge test-control must accept the config").toBeTruthy();
  await ctx.dispose();
}

async function expectNoAxeViolations(page: Page, context?: string) {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(results.violations, `axe violations${context ? ` on ${context}` : ""}: ${JSON.stringify(results.violations, null, 2)}`).toEqual([]);
}

test.afterEach(async () => {
  // Reset the dev-double to the default populated / dev-seam-on posture for the rest of the suite.
  await setEdgeConfig({
    emptyRepos: false,
    devLoginEnabled: true,
    whoamiUnavailable: false,
    seedInboxAgentApproval: false,
    inboxMutationUnavailable: false,
    inboxPagination: false,
  });
});

test.describe("R3.5 first-run — login", () => {
  test("login shows the OIDC/SSO PRIMARY (rides --c-btn-primary-bg) with a visible unavailable reason; dev seam relegated; axe green", async ({ page }) => {
    await setEdgeConfig({ devLoginEnabled: true });
    await page.goto("/login");

    // The SSO button is THE primary affordance and rides the derived button token (never raw accent).
    const sso = page.getByTestId("sso-login");
    await expect(sso).toBeVisible();
    const { got, token } = await page.evaluate(() => {
      const btn = document.querySelector('[data-testid="sso-login"]') as HTMLElement;
      const probe = document.createElement("div");
      document.body.appendChild(probe);
      probe.style.background = "var(--c-btn-primary-bg)";
      const token = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return { got: getComputedStyle(btn).backgroundColor, token };
    });
    expect(got).toBe(token); // primary rides --c-btn-primary-bg

    // SSO is not configured on this deployment → the reason is VISIBLE text (not a title tooltip),
    // referenced by aria-describedby on the disabled button.
    await expect(sso).toHaveAttribute("aria-disabled", "true");
    const reasonId = await sso.getAttribute("aria-describedby");
    expect(reasonId).toBe("sso-reason");
    await expect(page.getByTestId("sso-reason")).toBeVisible();
    await expect(page.getByTestId("sso-reason")).toContainText(/administrator/i);

    // The dev seam is present (relegated) — the harness has it enabled.
    await expect(page.getByTestId("dev-seam")).toBeVisible();
    await expect(page.getByTestId("dev-login")).toBeVisible();

    await expectNoAxeViolations(page, "/login (first-run)");
  });

  test("the dev seam is HIDDEN when the server flag is off (dev_login_enabled=false)", async ({ page }) => {
    await setEdgeConfig({ devLoginEnabled: false });
    await page.goto("/login");
    // The SSO primary still renders; the dev seam does NOT.
    await expect(page.getByTestId("sso-login")).toBeVisible();
    await expect(page.getByTestId("dev-seam")).toHaveCount(0);
    await expect(page.getByTestId("dev-login")).toHaveCount(0);
  });
});

test.describe("R3.5 first-run — empty tenant onboarding", () => {
  test("a fresh tenant creates its repository before showing exact login, push, and CI guidance", async ({ page }) => {
    await setEdgeConfig({ emptyRepos: true, devLoginEnabled: true });
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("dev-login").click();
    await page.waitForURL("**/git/repos");

    const empty = page.getByTestId("repos-empty");
    await expect(empty).toBeVisible();
    await expect(empty.getByRole("button", { name: "Create repository" })).toBeVisible();
    await expect(empty).toContainText("exact tenant, region, and Edge URL");
    await expect(empty).toContainText("without a pasted API key");
    await expect(empty).toContainText(".myelin/ci.toml");
    await expect(empty).not.toContainText("git.eu.myelin.dev");
    await expect(page.getByTestId("waiting-first-push")).toBeVisible();
    await expect(page.getByTestId("repos-refresh")).toBeVisible();

    await expectNoAxeViolations(page, "the empty-tenant onboarding");

    await empty.getByRole("button", { name: "Create repository" }).click();
    const create = page.getByRole("dialog", { name: "New repository" });
    await create.getByLabel("Name or namespace/name").fill("first-repository");
    await create.getByRole("button", { name: "Create repository" }).click();
    await page.waitForURL("**/git/repos/first-repository");
    await expect(page.getByRole("button", { name: "Copy reference" }))
      .toHaveAttribute("title", "myelin://acme/git/repo/first-repository");

    const setup = page.getByTestId("git-setup");
    await setup.getByText("Set up Git").click();
    await expect(setup.getByTestId("git-setup-commands")).toContainText("myelin auth login");
    await expect(setup.getByTestId("git-setup-commands")).toContainText("myelin auth configure-git");
    await expect(setup.getByTestId("git-setup-commands")).toContainText("git clone");
    await expect(setup.getByTestId("git-setup-commands")).toContainText("git push -u origin 'main'");
    await expectNoAxeViolations(page, "the first repository setup");
  });

  test("a lost creation response retries to the one repository that was already committed", async ({ page }) => {
    await setEdgeConfig({ emptyRepos: true, repoCreateResponseLosses: 1 });
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("dev-login").click();
    await page.waitForURL("**/git/repos");

    await page.getByTestId("repos-empty").getByRole("button", { name: "Create repository" }).click();
    const dialog = page.getByRole("dialog", { name: "New repository" });
    await dialog.getByLabel("Name or namespace/name").fill("retry-safe");
    await dialog.getByRole("button", { name: "Create repository" }).click();

    await expect(dialog.getByRole("alert")).toContainText("Retrying this unchanged name is safe");
    await expect(dialog.getByLabel("Name or namespace/name")).toHaveValue("retry-safe");
    await dialog.getByRole("button", { name: "Create repository" }).click();
    await page.waitForURL("**/git/repos/retry-safe");

    await page.goto("/git/repos");
    await expect(page.getByRole("link", { name: /retry-safe/ })).toHaveCount(1);
    await page.reload();
    await expect(page.getByRole("link", { name: /retry-safe/ })).toHaveCount(1);
  });
});

test.describe("R3.5 first-run — honest inbox", () => {
  test("the inbox is honest at zero: no '2 unread' badge, and an 'all caught up' empty state", async ({ page }) => {
    await setEdgeConfig({ emptyRepos: false, devLoginEnabled: true });
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("dev-login").click();
    await page.waitForURL("**/git/repos");

    // The topbar inbox button says "no unread" (never "2 unread") and shows NO count badge at zero.
    const inboxBtn = page.getByRole("button", { name: /Inbox/ });
    await expect(inboxBtn).toBeVisible();
    await expect(inboxBtn).toHaveAttribute("aria-label", /no unread notifications/i);
    await expect(page.getByTestId("inbox-badge")).toHaveCount(0);

    // Opening it shows the calm inbox-zero state (no fabricated rows).
    await inboxBtn.click();
    const dialog = page.getByRole("dialog", { name: "Inbox" });
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId("inbox-empty")).toContainText(/all caught up/i);
    await expect(dialog).not.toContainText("CI passed on acme/myelin");

    await expectNoAxeViolations(page, "the honest inbox (inbox-zero)");
  });

  test("an approval failure stays visible and safely retryable until the decision is saved", async ({ page }) => {
    await setEdgeConfig({
      seedInboxAgentApproval: true,
      inboxMutationUnavailable: true,
    });
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("dev-login").click();
    await page.waitForURL("**/git/repos");

    const inboxButton = page.getByRole("button", { name: /Inbox/ });
    await expect(inboxButton).toHaveAttribute("aria-label", /1 unread notification/i);
    await inboxButton.click();
    const dialog = page.getByRole("dialog", { name: "Inbox" });

    await dialog.getByRole("button", { name: "Approve" }).click();
    await expect(dialog.getByTestId("inbox-mutation-error")).toContainText(
      /couldn’t save that change/i,
    );
    await expect(page.getByText("Inbox change wasn’t saved")).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Approve" })).toBeEnabled();

    await setEdgeConfig({ inboxMutationUnavailable: false });
    await dialog.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByText("Approval saved")).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Approve" })).toHaveCount(0);
    await expect(dialog.getByTestId("inbox-mutation-error")).toHaveCount(0);
    await expectNoAxeViolations(page, "a recovered inbox approval");
  });

  test("keeps a long inbox intact and opens its namespaced Git destination", async ({ page }) => {
    await setEdgeConfig({ inboxPagination: true });
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("dev-login").click();
    await page.waitForURL("**/git/repos");

    const inboxButton = page.getByRole("button", { name: /Inbox/ });
    await expect(inboxButton).toHaveAttribute("aria-label", /1 unread notification/i);
    await inboxButton.click();
    const dialog = page.getByRole("dialog", { name: "Inbox" });
    const firstPullRequest = dialog.getByRole("link", { name: "platform/myelin #1" });
    await expect(firstPullRequest).toHaveAttribute("href", "/git/repos/platform%2Fmyelin/prs/1");
    await expect(firstPullRequest).toHaveAttribute(
      "title",
      "myelin://acme/git/pr/platform/myelin:1",
    );
    await expect(dialog.getByRole("link", { name: "platform/myelin #2" })).toHaveCount(0);

    await dialog.getByRole("button", { name: "Load more" }).click();
    await expect(firstPullRequest).toBeVisible();
    await expect(dialog.getByRole("link", { name: "platform/myelin #2" }))
      .toHaveAttribute("href", "/git/repos/platform%2Fmyelin/prs/2");
    await expect(dialog.getByRole("button", { name: "Load more" })).toHaveCount(0);
    await expect(inboxButton).toHaveAttribute("aria-label", /2 unread notifications/i);
    await expectNoAxeViolations(page, "a paged inbox");

    await firstPullRequest.click();
    await page.waitForURL("**/git/repos/platform%2Fmyelin/prs/1");
    await expect(dialog).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "R3.3 PR overview + context pane" }))
      .toBeVisible();
    const breadcrumb = page.getByRole("navigation", { name: "Breadcrumb" });
    const repository = breadcrumb.getByRole("link", { name: "platform/myelin" });
    await expect(repository).toHaveAttribute("href", "/git/repos/platform%2Fmyelin");

    await repository.click();
    await page.waitForURL("**/git/repos/platform%2Fmyelin");
    await expect(page.getByRole("heading", { name: "acme/platform/myelin" })).toBeVisible();
    await page.getByRole("link", { name: "README.md" }).click();
    await page.waitForURL("**/git/repos/platform%2Fmyelin/blob/**/README.md");
    await expect(page.getByRole("heading", { name: /README\.md/ })).toBeVisible();
  });

  test("a mutation supersedes an older inbox page without inheriting its failure", async ({ page, request }) => {
    await setEdgeConfig({
      inboxPagination: true,
      inboxListCursorDelaysMs: [3_000, 4_500],
      inboxListCursorFailures: 1,
    });
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("dev-login").click();
    await page.waitForURL("**/git/repos");

    await page.getByRole("button", { name: /Inbox/ }).click();
    const dialog = page.getByRole("dialog", { name: "Inbox" });
    await dialog.getByRole("button", { name: "Load more" }).click();
    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.inboxListCursorRequests;
    }).toBe(1);

    await dialog.getByRole("button", { name: "Mark read" }).click();
    await expect(dialog.getByRole("button", { name: "Mark read" })).toHaveCount(0);
    await expect(dialog.getByRole("button", { name: "Load more" })).toBeEnabled();
    await dialog.getByRole("button", { name: "Load more" }).click();
    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.inboxListCursorRequests;
    }).toBe(2);

    await expect.poll(async () => {
      const response = await request.post(`${EDGE}/__test/config`, { data: {} });
      return (await response.json()).state.inboxListCursorResponses;
    }).toBe(1);
    await expect(dialog.getByTestId("inbox-more-error")).toHaveCount(0);
    await expect(dialog.getByRole("button", { name: "Loading more…" })).toBeDisabled();

    await expect(dialog.getByRole("link", { name: "platform/myelin #2" }))
      .toBeVisible({ timeout: 7_000 });
    await expect(dialog.getByRole("button", { name: "Load more" })).toHaveCount(0);
  });
});

test.describe("R3.5 signed-in shell recovery", () => {
  test("a transient viewer-verification failure keeps the user’s place and recovers on retry", async ({ page }) => {
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("dev-login").click();
    await page.waitForURL("**/git/repos");

    await setEdgeConfig({ whoamiUnavailable: true });
    await page.goto("/git/repos?recovery=1");
    const unavailable = page.getByTestId("app-unavailable");
    await expect(unavailable).toBeVisible();
    await expect(unavailable).toContainText(/your place is kept/i);
    await expect(page.getByRole("navigation", { name: "Primary" })).toHaveCount(0);

    await setEdgeConfig({ whoamiUnavailable: false });
    await unavailable.getByRole("link", { name: "Try again" }).click();
    await expect(page).toHaveURL(/\/git\/repos\?recovery=1$/);
    await expect(page.getByRole("navigation", { name: "Primary" })).toBeVisible();
    await expectNoAxeViolations(page, "the recovered signed-in shell");
  });
});
