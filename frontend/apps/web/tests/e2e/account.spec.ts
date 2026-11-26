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
  await setEdgeConfig({
    resetAccounts: true,
    devLoginEnabled: true,
    forceUnauthorized: false,
    whoamiUnavailable: false,
  });
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
    await expect(page.getByTestId("invite-sent")).toContainText("We're emailing an invitation to linus@acme.dev");
    await expect(page.getByTestId("invite-url")).toHaveCount(0);

    const [link] = await mailTo("linus@acme.dev");
    await page.context().clearCookies();
    await page.goto(new URL(link!).pathname);
    await expect(page.getByTestId("join-email")).toHaveValue("linus@acme.dev");
  });

  test("one address can be invited only a few times an hour", async ({ page }) => {
    await signIn(page, ADA.email, ADA.password);
    await page.waitForURL("**/git/repos");
    await page.goto("/profile");
    await page.waitForLoadState("networkidle");
    for (let sent = 1; sent <= 3; sent += 1) {
      await page.getByTestId("invite-email").fill("linus@acme.dev");
      await page.getByTestId("invite-submit").click();
      await expect(page.getByTestId("invite-sent")).toBeVisible();
      expect(await mailTo("linus@acme.dev")).toHaveLength(sent);
    }
    await page.getByTestId("invite-email").fill("linus@acme.dev");
    await page.getByTestId("invite-submit").click();
    await expect(page.getByTestId("invite-error")).toHaveText(
      "That address has had several invitations recently. Try again in 42 minutes.",
    );
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

/** The sign-outs the web app asked the double for, oldest first. */
async function signOuts(): Promise<string[]> {
  const context = await pwRequest.newContext();
  const response = await context.get(`${EDGE}/__test/sessions`);
  const { signOuts } = (await response.json()) as { signOuts: string[] };
  await context.dispose();
  return signOuts;
}

async function openProfileSignedIn(page: Page, password = ADA.password) {
  await signIn(page, ADA.email, password);
  await page.waitForURL("**/git/repos");
  await page.goto("/profile");
  await page.waitForLoadState("networkidle");
}

async function changePassword(page: Page, current: string, next: string, confirm = next) {
  await page.getByTestId("current-password").fill(current);
  await page.getByTestId("new-password").fill(next);
  await page.getByTestId("confirm-password").fill(confirm);
  await page.getByTestId("password-submit").click();
}

test.describe("password and sessions", () => {
  test("a person changes their password and stays signed in; axe green", async ({ page }) => {
    await openProfileSignedIn(page);
    await expect(page.getByRole("heading", { name: "Password" })).toBeVisible();
    await expectNoAxeViolations(page, "/profile (password card)");

    await changePassword(page, "not my password", "a newer passphrase");
    await expect(page.getByTestId("password-error")).toHaveText("That isn't your current password.");
    await expect(page.getByTestId("current-password")).toHaveAttribute("aria-invalid", "true");

    await changePassword(page, ADA.password, "a newer passphrase", "a different one!");
    await expect(page.getByTestId("password-error")).toHaveText("The two new passwords don't match.");

    await changePassword(page, ADA.password, "a newer passphrase");
    await expect(page.getByText("Password changed. Your other sessions were signed out.")).toBeVisible();
    await expect(page.getByTestId("password-error")).toHaveCount(0);
    await page.reload();
    await expect(page.getByRole("heading", { level: 1, name: ADA.name })).toBeVisible();

    await page.context().clearCookies();
    await signIn(page, ADA.email, ADA.password);
    await page.waitForURL(/error=credentials/);
    await signIn(page, ADA.email, "a newer passphrase");
    await page.waitForURL("**/git/repos");
  });

  test("a change another device beat says so and keeps the person signed in", async ({ page }) => {
    await openProfileSignedIn(page);
    await setEdgeConfig({ changePasswordConflict: true });
    await changePassword(page, ADA.password, "a newer passphrase");
    await expect(page.getByTestId("password-error")).toHaveText(
      "Your password was just changed somewhere else, so it wasn't changed here.",
    );
    await page.reload();
    await expect(page.getByRole("heading", { level: 1, name: ADA.name })).toBeVisible();
  });

  test("a change refused because the session ended sends the person to sign in", async ({ page }) => {
    await openProfileSignedIn(page);
    await setEdgeConfig({ forceUnauthorized: true });
    await changePassword(page, ADA.password, "a newer passphrase");
    await page.waitForURL(/\/login\?signed_out=session$/);
    await expect(page.getByTestId("signed-out")).toHaveText("You were signed out.");
  });

  test("when the new session cannot be set up, the person is asked to sign in again", async ({ page }) => {
    await openProfileSignedIn(page);
    await setEdgeConfig({ whoamiFailsAfterPasswordChange: true });
    await changePassword(page, ADA.password, "a newer passphrase");
    await page.waitForURL(/\/login\?signed_out=password_changed$/);
    await expect(page.getByTestId("signed-out")).toHaveText(
      "Your password was changed. Sign in again.",
    );
    await setEdgeConfig({ whoamiUnavailable: false });
    await signIn(page, ADA.email, "a newer passphrase");
    await page.waitForURL("**/git/repos");
  });

  for (const lost of ["malformed", "server_error"] as const) {
    test(`a change whose answer is lost (${lost}) asks the person to sign in again`, async ({ page }) => {
      await openProfileSignedIn(page);
      await setEdgeConfig({ changePasswordAnswerLost: lost });
      await changePassword(page, ADA.password, "a newer passphrase");
      await page.waitForURL(/\/login\?signed_out=password_changed_maybe$/);
      await expect(page.getByTestId("signed-out")).toHaveText(
        "Your password may have changed. Sign in with your new password, or your old one if it didn't.",
      );
      await signIn(page, ADA.email, "a newer passphrase");
      await page.waitForURL("**/git/repos");
    });
  }

  test("a cross-site link cannot run sign-out everywhere", async ({ page }) => {
    await openProfileSignedIn(page);
    await page.getByTestId("signout-everywhere").click();
    const action = await page.getByTestId("signout-everywhere-confirm").getAttribute("action");
    expect(action).toMatch(/\/_server\?id=/);
    // A top-level GET carries the cookie, as a link from another site would. h3 routes every one
    // of these spellings to the server-function handler.
    const query = action!.slice(action!.indexOf("?"));
    for (const path of ["/_server/", "/%5Fserver/", "/_%73erver", "/_server/x", "/_serverx"]) {
      const response = await page.request.get(`${path}${query}`, { maxRedirects: 0 });
      expect(response.status(), path).toBe(403);
    }
    expect(await signOuts()).toEqual([]);
    await page.reload();
    await expect(page.getByRole("heading", { level: 1, name: ADA.name })).toBeVisible();
  });

  test("the sign-out confirmation takes focus, gives it back, and starts clean", async ({ page }) => {
    await openProfileSignedIn(page);
    await setEdgeConfig({ signOutUnavailable: true });
    await page.getByTestId("signout-everywhere").click();
    await expect(page.getByTestId("signout-everywhere-submit")).toBeFocused();
    await page.getByTestId("signout-everywhere-submit").click();
    await expect(page.getByTestId("signout-everywhere-error")).toBeVisible();
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByTestId("signout-everywhere")).toBeFocused();
    await page.getByTestId("signout-everywhere").click();
    await expect(page.getByTestId("signout-everywhere-error")).toHaveCount(0);
  });

  test("without JavaScript no password ever reaches the flash cookie", async ({ page, baseURL }) => {
    // Sign-in answers a form post, as a browser without JavaScript sends it, with a redirect,
    // which SolidStart never flashes. (Accepting an invitation does too.)
    await page.goto("/login");
    await page.waitForLoadState("networkidle");
    const action = await page.getByTestId("password-login-form").getAttribute("action");
    const posted = await page.request.post(action!, {
      form: { email: ADA.email, password: "not the password", return_to: "/git/repos" },
      headers: { origin: baseURL! },
      maxRedirects: 0,
    });
    expect(posted.status()).toBe(302);
    expect(posted.headers()["set-cookie"] ?? "").not.toContain("flash=");

    // The password form cannot be posted until the page is interactive: every submit button the
    // server renders for it, before any script runs, is disabled.
    await signIn(page, ADA.email, ADA.password);
    await page.waitForURL("**/git/repos");
    const html = await (await page.request.get("/profile")).text();
    const buttons = html.match(/<button[^>]*data-testid="password-submit"[^>]*>/g) ?? [];
    expect(buttons.length, "the server renders the Password card").toBeGreaterThan(0);
    for (const button of buttons) expect(button).toMatch(/\sdisabled(=|\s|>)/);
  });

  test("password errors point at the field they are about", async ({ page }) => {
    await openProfileSignedIn(page);
    await changePassword(page, ADA.password, "a newer passphrase", "a different one!");
    await expect(page.getByTestId("confirm-password")).toHaveAttribute(
      "aria-describedby",
      "password-change-error",
    );
    await expect(page.getByTestId("current-password")).not.toHaveAttribute(
      "aria-describedby",
      /password-change-error/,
    );
    await expect(page.getByTestId("new-password")).toHaveAttribute(
      "aria-describedby",
      "new-password-hint",
    );
  });

  test("signing out ends the session at the edge", async ({ page }) => {
    await openProfileSignedIn(page);
    await page.getByRole("button", { name: "Sign out of Myelin" }).click();
    await page.waitForURL(/\/login$/);
    expect(await signOuts()).toEqual(["session"]);
    await page.goto("/profile");
    await page.waitForURL(/\/login$/);
  });

  test("signing out still signs this browser out when the edge cannot be reached", async ({ page }) => {
    await openProfileSignedIn(page);
    await setEdgeConfig({ signOutUnavailable: true });
    await page.getByRole("button", { name: "Sign out of Myelin" }).click();
    await page.waitForURL(/\/login$/);
    await page.goto("/profile");
    await page.waitForURL(/\/login$/);
  });

  test("signing out of all devices asks first, then signs out everywhere; axe green", async ({ page }) => {
    await openProfileSignedIn(page);
    await page.getByTestId("signout-everywhere").click();
    await expect(page.getByTestId("signout-everywhere-confirm")).toContainText(
      "Sign out on every device, including this one?",
    );
    await expectNoAxeViolations(page, "/profile (sign out everywhere)");
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByTestId("signout-everywhere-confirm")).toHaveCount(0);
    expect(await signOuts()).toEqual([]);

    await page.getByTestId("signout-everywhere").click();
    await page.getByTestId("signout-everywhere-submit").click();
    await page.waitForURL(/\/login\?signed_out=everywhere$/);
    await expect(page.getByTestId("signed-out")).toHaveText("You were signed out of all devices.");
    expect(await signOuts()).toEqual(["everywhere"]);
  });

  test("when the edge cannot sign out everywhere, the person stays signed in and is told", async ({ page }) => {
    await openProfileSignedIn(page);
    await setEdgeConfig({ signOutUnavailable: true });
    await page.getByTestId("signout-everywhere").click();
    await page.getByTestId("signout-everywhere-submit").click();
    await expect(page.getByTestId("signout-everywhere-error")).toContainText("couldn't be signed out");
    await page.reload();
    await expect(page.getByRole("heading", { level: 1, name: ADA.name })).toBeVisible();
  });

  test("a session the edge refuses sends the person to sign-in, saying so; axe green", async ({ page }) => {
    await openProfileSignedIn(page);
    await setEdgeConfig({ forceUnauthorized: true });
    await page.goto("/profile");
    await page.waitForURL(/\/login\?signed_out=session$/);
    await expect(page.getByTestId("signed-out")).toHaveText("You were signed out.");
    await expectNoAxeViolations(page, "/login?signed_out=session");
    await setEdgeConfig({ forceUnauthorized: false });
  });
});
