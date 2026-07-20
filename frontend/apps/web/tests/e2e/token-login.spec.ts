import { test, expect, type Page, request as pwRequest } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { DEV_ACCESS_TOKEN } from "../../dev-edge/dev-contract.mjs";

// R4.0 operator-token login — real-browser proofs for the paste-your-bootstrap-token card. Drives the
// SAME single harness (dev edge + SolidStart); flips the dev edge's `token_login_enabled` on via the
// `POST /__test/config` control seam, then exercises the card. The dev edge's `/v1/whoami` verifies a
// pasted Bearer (=== DEV_ACCESS_TOKEN), so paste→verify→session→/git/repos runs end-to-end here.
// Every test resets `tokenLoginEnabled` off in afterEach so the rest of the serial suite is unchanged.

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;

async function setEdgeConfig(cfg: {
  emptyRepos?: boolean;
  devLoginEnabled?: boolean;
  tokenLoginEnabled?: boolean;
}) {
  const ctx = await pwRequest.newContext();
  const res = await ctx.post(`${EDGE}/__test/config`, { data: cfg });
  expect(res.ok(), "dev-edge test-control must accept the config").toBeTruthy();
  await ctx.dispose();
}

async function expectNoAxeViolations(page: Page, context?: string) {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(
    results.violations,
    `axe violations${context ? ` on ${context}` : ""}: ${JSON.stringify(results.violations, null, 2)}`,
  ).toEqual([]);
}

test.afterEach(async () => {
  // Reset the token-login flag off for the rest of the serial suite (default posture).
  await setEdgeConfig({ tokenLoginEnabled: false });
});

test.describe("R4.0 operator-token login", () => {
  test("renders the token card when the edge flag is on: password input, associated label + helper, primary when SSO unconfigured; axe green", async ({ page }) => {
    await setEdgeConfig({ tokenLoginEnabled: true, devLoginEnabled: true });
    await page.goto("/login");

    const form = page.getByTestId("token-login-form");
    await expect(form).toBeVisible();

    // The token is a SECRET → a password input (not stored in autocomplete history), labelled.
    const input = page.getByTestId("token-input");
    await expect(input).toHaveAttribute("type", "password");
    await expect(input).toHaveAttribute("autocomplete", "off");
    // The control is associated by NESTING inside its label (the label carries the visible text).
    const label = page.locator('label:has([data-testid="token-input"])');
    await expect(label).toBeVisible();
    await expect(label).toContainText(/operator token/i);
    // Honest helper text names the source command and is referenced by aria-describedby.
    await expect(page.getByTestId("token-help")).toContainText(/edge bootstrap/i);
    await expect(input).toHaveAttribute("aria-describedby", /token-help/);

    // SSO is unconfigured on this deployment → the token submit is the PRIMARY affordance (rides the
    // derived primary button token, never raw accent).
    const submit = page.getByTestId("token-login");
    const { got, token } = await page.evaluate(() => {
      const btn = document.querySelector('[data-testid="token-login"]') as HTMLElement;
      const probe = document.createElement("div");
      document.body.appendChild(probe);
      probe.style.background = "var(--c-btn-primary-bg)";
      const t = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return { got: getComputedStyle(btn).backgroundColor, token: t };
    });
    expect(got).toBe(token);
    await expect(submit).toBeVisible();

    await expectNoAxeViolations(page, "/login (token card)");
  });

  test("shows the honest, token-specific error on ?error=token_invalid (blames the token, not the user); axe green", async ({ page }) => {
    await setEdgeConfig({ tokenLoginEnabled: true });
    await page.goto("/login?error=token_invalid");

    const err = page.getByTestId("login-error-token");
    await expect(err).toBeVisible();
    await expect(err).toContainText(/invalid or expired/i);
    await expect(err).toContainText(/edge bootstrap/i);
    await expect(err).toContainText(/nothing's wrong on your end/i);
    // The SSO-specific error copy must NOT be what we show for a token failure.
    await expect(err).not.toContainText(/identity provider/i);
    // The input flags itself invalid and points at the error message.
    await expect(page.getByTestId("token-input")).toHaveAttribute("aria-invalid", "true");
    await expect(page.getByTestId("token-input")).toHaveAttribute("aria-describedby", /login-error-msg/);

    await expectNoAxeViolations(page, "/login?error=token_invalid");
  });

  test("a valid pasted token verifies against the edge and lands in the app (end-to-end)", async ({ page }) => {
    await setEdgeConfig({ tokenLoginEnabled: true });
    await page.goto("/login");
    // Wait for the SolidStart router to hydrate the form before submitting — a click before the
    // progressive-enhancement handler attaches is a no-op (the action never fires).
    await page.waitForLoadState("networkidle");

    await page.getByTestId("token-input").fill(DEV_ACCESS_TOKEN);
    await page.getByTestId("token-login").click();

    // The dev edge verifies the pasted Bearer via whoami(200) → session minted → redirect into the app.
    await page.waitForURL("**/git/repos");
    await expect(page).toHaveURL(/\/git\/repos/);
  });

  test("a direct token action fails closed when the edge disables the mode after render", async ({ page }) => {
    // Render the real action-backed form while enabled, then disable the edge flag before submit.
    // This models both a stale open login tab and a caller invoking the registered action directly:
    // security must live in the action, not in the conditional that painted the form.
    await setEdgeConfig({ tokenLoginEnabled: true });
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await expect(page.getByTestId("token-login-form")).toBeVisible();

    await setEdgeConfig({ tokenLoginEnabled: false });
    await page.getByTestId("token-input").fill(DEV_ACCESS_TOKEN);
    await page.getByTestId("token-login").click();

    // The action re-reads auth/config, refuses before whoami/session issuance, and redirects to the
    // logged-out chooser. A protected navigation remains logged out (no latent Set-Cookie/session).
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByTestId("token-login-form")).toHaveCount(0);
    await page.goto("/git/repos");
    await expect(page).toHaveURL(/\/login$/);
  });

  test("the token card is HIDDEN when the edge flag is off", async ({ page }) => {
    await setEdgeConfig({ tokenLoginEnabled: false });
    await page.goto("/login");
    await expect(page.getByTestId("token-login-form")).toHaveCount(0);
  });
});
