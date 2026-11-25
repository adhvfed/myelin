import { test, expect, type Page, request as pwRequest } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

// The person-facing account journey against the local edge double: sign in with an email and
// password, join through an invitation link, reset through one, and invite a teammate.

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;
const ADA = { email: "ada@acme.dev", password: "correct horse battery", name: "Ada Lovelace" };
const JOIN_CODE = "e2e-join-invitation-000000000000";
const RESET_CODE = "e2e-reset-invitation-00000000000";

async function setEdgeConfig(config: Record<string, unknown>): Promise<void> {
  const context = await pwRequest.newContext();
  const response = await context.post(`${EDGE}/__test/config`, { data: config });
  expect(response.ok(), "dev-edge test-control must accept the config").toBeTruthy();
  await context.dispose();
}

/** The links the double has "emailed" to `to`, oldest first. */
async function mailTo(to: string): Promise<string[]> {
  const context = await pwRequest.newContext();
  const response = await context.get(`${EDGE}/__test/mail`);
  const { mail } = (await response.json()) as { mail: { to: string; url: string }[] };
  await context.dispose();
  return mail.filter((letter) => letter.to === to).map((letter) => letter.url);
}

async function expectNoAxeViolations(page: Page, where: string) {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(results.violations, `axe violations on ${where}: ${JSON.stringify(results.violations, null, 2)}`).toEqual([]);
}

async function signIn(page: Page, email: string, password: string) {
  await page.goto("/login");
  // A click before the router hydrates the form is a no-op.
  await page.waitForLoadState("networkidle");
  await page.getByTestId("email-input").fill(email);
  await page.getByTestId("password-input").fill(password);
  await page.getByTestId("password-login").click();
}

test.beforeEach(async () => {
  await setEdgeConfig({ resetAccounts: true, devLoginEnabled: true });
});
test.afterEach(async () => {
  await setEdgeConfig({ resetAccounts: true, devLoginEnabled: true });
});

test.describe("signing in", () => {
  test("a person signs in with their email and password and is greeted by name", async ({ page }) => {
    await signIn(page, ADA.email, ADA.password);
    await page.waitForURL("**/git/repos");
    await page.goto("/profile");
    await expect(page.getByRole("heading", { level: 1, name: ADA.name })).toBeVisible();
  });

  test("a wrong password says so without naming which half was wrong; axe green", async ({ page }) => {
    await signIn(page, ADA.email, "not the password");
    await page.waitForURL(/\/login\?error=credentials/);
    const alert = page.getByTestId("login-error");
    await expect(alert).toContainText(/email and password don't match/i);
    await expect(page.getByTestId("password-input")).toHaveAttribute("aria-invalid", "true");
    await expectNoAxeViolations(page, "/login?error=credentials");
  });

  test("an unknown email gets exactly the same answer as a wrong password", async ({ page }) => {
    await signIn(page, "nobody@acme.dev", ADA.password);
    await page.waitForURL(/\/login\?error=credentials/);
    await expect(page.getByTestId("login-error")).toContainText(/email and password don't match/i);
  });

  test("too many attempts asks the person to wait", async ({ page }) => {
    await setEdgeConfig({ loginRateLimited: true });
    await signIn(page, ADA.email, ADA.password);
    await page.waitForURL(/\/login\?error=rate_limited/);
    await expect(page.getByTestId("login-error")).toContainText(/wait a minute/i);
  });

  test("a failed sign-in keeps the page the person was heading to", async ({ page }) => {
    await page.goto("/login?return_to=%2Fprofile");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("email-input").fill(ADA.email);
    await page.getByTestId("password-input").fill("not the password");
    await page.getByTestId("password-login").click();
    await page.waitForURL(/error=credentials/);
    await page.waitForLoadState("networkidle");
    await page.getByTestId("email-input").fill(ADA.email);
    await page.getByTestId("password-input").fill(ADA.password);
    await page.getByTestId("password-login").click();
    await page.waitForURL("**/profile");
  });

  test("without mail, forgot password says to ask for a reset link; axe green", async ({ page }) => {
    await setEdgeConfig({ mailDeliveryEnabled: false });
    await page.goto("/login");
    await page.getByTestId("forgot-password").click();
    await expect(page.getByTestId("password-help")).toContainText(/reset link/i);
    await expect(page.getByTestId("reset-form")).toHaveCount(0);
    await expectNoAxeViolations(page, "/login?help=password");
  });

  test("forgot password emails a link without saying whether the account exists; axe green", async ({ page }) => {
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("forgot-password").click();
    await expect(page.getByRole("heading", { name: "Reset your password" })).toBeVisible();
    await expectNoAxeViolations(page, "/login?help=password (mail on)");
    await page.waitForLoadState("networkidle");

    await page.getByTestId("reset-email").fill("nobody@acme.dev");
    await page.getByTestId("reset-submit").click();
    await expect(page.getByTestId("reset-sent")).toHaveText(
      "If an account exists for nobody@acme.dev, a reset link is on its way.",
    );
    expect(await mailTo("nobody@acme.dev")).toEqual([]);

    await page.goto("/login?help=password");
    await page.waitForLoadState("networkidle");
    await page.getByTestId("reset-email").fill(ADA.email);
    await page.getByTestId("reset-submit").click();
    await expect(page.getByTestId("reset-sent")).toHaveText(
      `If an account exists for ${ADA.email}, a reset link is on its way.`,
    );
    const [link] = await mailTo(ADA.email);
    await page.goto(new URL(link!).pathname);
    await expect(page.getByRole("heading", { name: "Choose a new password" })).toBeVisible();
    await page.waitForLoadState("networkidle");
    await page.getByTestId("join-password").fill("a password from email");
    await page.getByTestId("join-submit").click();
    await page.waitForURL("**/git/repos");

    await page.context().clearCookies();
    await page.goto("/login?help=password");
    await page.getByTestId("reset-back").click();
    await page.waitForURL(/\/login$/);
    await signIn(page, ADA.email, "a password from email");
    await page.waitForURL("**/git/repos");
  });
});

test.describe("invitation links", () => {
  test("an invited person names themselves, picks a password, and is signed in; axe green", async ({ page }) => {
    await page.goto(`/join/${JOIN_CODE}`);
    await expect(page.getByRole("heading", { name: "Join acme" })).toBeVisible();
    await expect(page.getByTestId("join-email")).toHaveValue("grace@acme.dev");
    await expectNoAxeViolations(page, "/join (invitation)");
    await page.waitForLoadState("networkidle");

    await page.getByTestId("join-name").fill("Grace Hopper");
    await page.getByTestId("join-password").fill("short");
    await expect(page.getByTestId("join-password-count")).toHaveText("5 / 10");
    await page.getByTestId("join-password").fill("a much longer password");
    await expect(page.getByTestId("join-password-count")).toHaveText("10 / 10");
    await page.getByTestId("join-submit").click();

    await page.waitForURL("**/git/repos");
    await page.goto("/profile");
    await expect(page.getByRole("heading", { level: 1, name: "Grace Hopper" })).toBeVisible();

    // The link worked once. The account it made signs in the ordinary way.
    await page.getByRole("button", { name: "Sign out of Myelin" }).click();
    await page.waitForURL("**/login");
    await page.goto(`/join/${JOIN_CODE}`);
    await expect(page.getByTestId("join-invalid")).toBeVisible();
    await signIn(page, "grace@acme.dev", "a much longer password");
    await page.waitForURL("**/git/repos");
  });

  test("a reset link sets a new password; the old one stops working", async ({ page }) => {
    await page.goto(`/join/${RESET_CODE}`);
    await expect(page.getByRole("heading", { name: "Choose a new password" })).toBeVisible();
    await expect(page.getByTestId("join-name")).toHaveCount(0);
    await page.waitForLoadState("networkidle");
    await page.getByTestId("join-password").fill("a brand new password");
    await page.getByTestId("join-submit").click();
    await page.waitForURL("**/git/repos");

    await page.context().clearCookies();
    await signIn(page, ADA.email, ADA.password);
    await page.waitForURL(/error=credentials/);
    await signIn(page, ADA.email, "a brand new password");
    await page.waitForURL("**/git/repos");
  });

  test("an unknown or used link says so and points back to sign-in; axe green", async ({ page }) => {
    await page.goto("/join/this-link-does-not-exist-0000");
    await expect(page.getByTestId("join-invalid")).toContainText(/expired or was already used/i);
    await expectNoAxeViolations(page, "/join (invalid)");
    await page.getByRole("link", { name: "Go to sign in" }).click();
    await page.waitForURL("**/login");
  });

  test("a member invites a teammate by email; the invitee opens the emailed link", async ({ page }) => {
    await signIn(page, ADA.email, ADA.password);
    await page.waitForURL("**/git/repos");
    await page.goto("/profile");
    await page.waitForLoadState("networkidle");

    await page.getByTestId("invite-email").fill("linus@acme.dev");
    await page.getByTestId("invite-submit").click();
    await expect(page.getByTestId("invite-sent")).toContainText("Invitation sent to linus@acme.dev");
    await expect(page.getByTestId("invite-url")).toHaveCount(0);

    const [link] = await mailTo("linus@acme.dev");
    await page.context().clearCookies();
    await page.goto(new URL(link!).pathname);
    await expect(page.getByTestId("join-email")).toHaveValue("linus@acme.dev");
  });

  test("without mail, a member invites a teammate and gets a link to pass on", async ({ page }) => {
    await setEdgeConfig({ mailDeliveryEnabled: false });
    await signIn(page, ADA.email, ADA.password);
    await page.waitForURL("**/git/repos");
    await page.goto("/profile");
    await page.waitForLoadState("networkidle");

    await page.getByTestId("invite-email").fill("linus@acme.dev");
    await page.getByTestId("invite-submit").click();
    const link = await page.getByTestId("invite-url").inputValue();
    expect(link).toMatch(/\/join\/[A-Za-z0-9_-]+$/);

    // Inviting someone who already has an account is refused, so it can't reset their password.
    await page.getByTestId("invite-email").fill(ADA.email);
    await page.getByTestId("invite-submit").click();
    await expect(page.getByTestId("invite-error")).toContainText(/already has an account/i);

    await page.context().clearCookies();
    await page.goto(new URL(link).pathname);
    await expect(page.getByTestId("join-email")).toHaveValue("linus@acme.dev");
  });
});
