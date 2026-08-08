import { test, expect, type Page, request as pwRequest } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

// R3.5 first-run flow — real-browser + axe proofs for the login states, the empty-tenant onboarding,
// and the honest inbox. Drives the SAME single harness (dev edge + SolidStart) and uses the dev
// edge's test-control seam (`POST /__test/config`, a dev-double-only route) to model a fresh empty
// tenant + a dev-seam-off deployment. Every test resets that state in afterEach so the rest of the
// serial suite (shell.spec, git-browse.spec, prs.spec) sees the default populated posture.

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;

async function setEdgeConfig(cfg: { emptyRepos?: boolean; devLoginEnabled?: boolean }) {
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
  await setEdgeConfig({ emptyRepos: false, devLoginEnabled: true });
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
  test("a fresh empty tenant TEACHES push-to-create with copyable CTAs + a dismissable checklist; axe green", async ({ page }) => {
    await setEdgeConfig({ emptyRepos: true, devLoginEnabled: true });
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("dev-login").click();
    await page.waitForURL("**/git/repos");

    // The onboarding empty state (not the old one-liner): teaches remote + push, blames nothing.
    const empty = page.getByTestId("repos-empty");
    await expect(empty).toBeVisible();
    await expect(page.getByTestId("cmd-remote")).toContainText("git remote add myelin");
    await expect(page.getByTestId("cmd-push")).toContainText("git push -u myelin main");
    await expect(empty).toContainText(/the push is the create/i);
    // Copy CTAs are labelled.
    await expect(page.getByRole("button", { name: "Copy: git remote add" })).toBeVisible();
    // The live waiting affordance + manual Refresh fallback.
    await expect(page.getByTestId("waiting-first-push")).toBeVisible();
    await expect(page.getByTestId("repos-refresh")).toBeVisible();

    // The first-run checklist is present and DISMISSABLE.
    await expect(page.getByTestId("first-run-checklist")).toBeVisible();
    await page.getByTestId("checklist-dismiss").click();
    await expect(page.getByTestId("first-run-checklist")).toHaveCount(0);

    await expectNoAxeViolations(page, "the empty-tenant onboarding");
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
});
