import { expect, test, type Page } from "@playwright/test";

// A person's account, start to finish, against the real stack: the operator's first invitation
// link, a chosen password, an ordinary sign-in, an emailed invitation for a teammate, and a
// forgotten password reset from an emailed link, a password change on one of two devices, and
// signing out of all of them, and a stranger's guesses that lock out only new browsers. Mail is
// read from the stack's Mailpit inbox.

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required; run this test with fed test:integration`);
  return value;
}

const inviteUrl = requiredEnvironment("MYELIN_BROWSER_INVITE_URL");
const inviteEmail = requiredEnvironment("MYELIN_BROWSER_INVITE_EMAIL");
const mailpit = requiredEnvironment("MYELIN_BROWSER_MAILPIT_URL");
const password = "a real-stack test password";

/** Waits for the `count`th message to `to` in Mailpit and returns the Myelin link it carries. */
async function emailedLink(to: string, count = 1): Promise<string> {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    const query = new URLSearchParams({ query: `to:"${to}"` });
    const found = await (await fetch(`${mailpit}/api/v1/search?${query}`)).json() as {
      messages: { ID: string }[];
    };
    if (found.messages.length >= count) {
      // Search lists the newest first.
      const id = found.messages[found.messages.length - count]!.ID;
      const message = await (await fetch(`${mailpit}/api/v1/message/${id}`)).json() as { Text: string };
      const link = /https?:\/\/\S+\/join\/[A-Za-z0-9_-]{43}/.exec(message.Text)?.[0];
      if (link) return link;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`no message ${count} to ${to} arrived in Mailpit`);
}

async function signIn(page: Page, email: string, secret: string) {
  await page.goto("/login");
  await page.waitForLoadState("networkidle");
  await page.getByTestId("email-input").fill(email);
  await page.getByTestId("password-input").fill(secret);
  await page.getByTestId("password-login").click();
}

test("an invited person creates an account, invites a teammate by email, and resets a forgotten password", async ({
  page,
  browser,
}) => {
  test.setTimeout(60_000);
  await page.goto(new URL(inviteUrl).pathname);
  await page.waitForLoadState("networkidle");
  await expect(page.getByTestId("join-email")).toHaveValue(inviteEmail);
  await page.getByTestId("join-name").fill("Ada Lovelace");
  await page.getByTestId("join-password").fill(password);
  await expect(page.getByTestId("join-password-count")).toHaveText("10 / 10");
  await page.getByTestId("join-submit").click();
  await page.waitForURL("**/git/repos");

  // The session belongs to a named person, not a token.
  await page.goto("/profile");
  await page.waitForLoadState("networkidle");
  await expect(page.getByRole("heading", { level: 1, name: "Ada Lovelace" })).toBeVisible();

  const teammate = inviteEmail.replace("account-", "teammate-");
  await page.getByTestId("invite-email").fill(teammate);
  await page.getByTestId("invite-submit").click();
  await expect(page.getByTestId("invite-sent")).toContainText(`We're emailing an invitation to ${teammate}`);
  const teammateLink = await emailedLink(teammate);
  await page.getByTestId("invite-email").fill(inviteEmail);
  await page.getByTestId("invite-submit").click();
  await expect(page.getByTestId("invite-error")).toContainText(/already has an account/i);

  // The link worked once; from here on the password is the way in.
  await page.getByRole("button", { name: "Sign out of Myelin" }).click();
  await page.waitForURL("**/login");
  await page.goto(new URL(inviteUrl).pathname);
  await expect(page.getByTestId("join-invalid")).toBeVisible();

  await signIn(page, inviteEmail, "not the password");
  await page.waitForURL(/error=credentials/);
  await expect(page.getByTestId("login-error")).toBeVisible();
  await signIn(page, inviteEmail.toUpperCase(), password);
  await page.waitForURL("**/git/repos");

  const other = await browser.newContext();
  const teammatePage = await other.newPage();
  await teammatePage.goto(new URL(teammateLink).pathname);
  await teammatePage.waitForLoadState("networkidle");
  await expect(teammatePage.getByTestId("join-email")).toHaveValue(teammate);
  await teammatePage.getByTestId("join-name").fill("Grace Hopper");
  await teammatePage.getByTestId("join-password").fill("another real password");
  await teammatePage.getByTestId("join-submit").click();
  await teammatePage.waitForURL("**/git/repos");
  await other.close();

  // Ada forgets her password and resets it from the emailed link.
  const forgetful = await browser.newContext();
  const resetPage = await forgetful.newPage();
  await resetPage.goto("/login");
  await resetPage.waitForLoadState("networkidle");
  await resetPage.getByTestId("forgot-password").click();
  await resetPage.waitForLoadState("networkidle");
  await resetPage.getByTestId("reset-email").fill(inviteEmail);
  await resetPage.getByTestId("reset-submit").click();
  await expect(resetPage.getByTestId("reset-sent")).toHaveText(
    `If an account exists for ${inviteEmail}, a reset link is on its way.`,
  );
  await resetPage.goto(new URL(await emailedLink(inviteEmail)).pathname);
  await resetPage.waitForLoadState("networkidle");
  await expect(resetPage.getByRole("heading", { name: "Choose a new password" })).toBeVisible();
  await resetPage.getByTestId("join-password").fill("a password from the inbox");
  await resetPage.getByTestId("join-submit").click();
  await resetPage.waitForURL("**/git/repos");
  await forgetful.close();

  // The reset ended the session Ada had open before it.
  await page.goto("/profile");
  await page.waitForURL(/\/login\?signed_out=session$/);
  await expect(page.getByTestId("signed-out")).toHaveText("You were signed out.");

  await signIn(page, inviteEmail, password);
  await page.waitForURL(/error=credentials/);
  await signIn(page, inviteEmail, "a password from the inbox");
  await page.waitForURL("**/git/repos");

  // Signed in on a second device, Ada changes her password on the first. The first stays signed
  // in; the second is signed out.
  const phoneContext = await browser.newContext();
  const phone = await phoneContext.newPage();
  await signIn(phone, inviteEmail, "a password from the inbox");
  await phone.waitForURL("**/git/repos");

  await page.goto("/profile");
  await page.waitForLoadState("networkidle");
  await page.getByTestId("current-password").fill("a password from the inbox");
  await page.getByTestId("new-password").fill("a password changed in place");
  await page.getByTestId("confirm-password").fill("a password changed in place");
  await page.getByTestId("password-submit").click();
  await expect(page.getByText("Password changed. Your other sessions were signed out.")).toBeVisible();
  await page.reload();
  await expect(page.getByRole("heading", { level: 1, name: "Ada Lovelace" })).toBeVisible();

  await phone.goto("/profile");
  await phone.waitForURL(/\/login\?signed_out=session$/);
  await expect(phone.getByTestId("signed-out")).toHaveText("You were signed out.");
  await signIn(phone, inviteEmail, "a password changed in place");
  await phone.waitForURL("**/git/repos");

  // A stranger's ten wrong guesses lock out new browsers, not the phone that signed in before.
  const strangerContext = await browser.newContext();
  const stranger = await strangerContext.newPage();
  for (let attempt = 0; attempt < 10; attempt += 1) {
    await signIn(stranger, inviteEmail, "a stranger's guess");
    await stranger.waitForURL(/error=credentials/);
  }
  await signIn(stranger, inviteEmail, "a password changed in place");
  await stranger.waitForURL(/error=rate_limited/);
  await expect(stranger.getByTestId("login-error")).toContainText(
    "sign in from a device you've used before, or reset your password.",
  );
  await strangerContext.close();
  await phoneContext.clearCookies({ name: /myelin_session$/ });
  await signIn(phone, inviteEmail, "a password changed in place");
  await phone.waitForURL("**/git/repos");

  // Signing out of all devices ends both.
  await page.waitForLoadState("networkidle");
  await page.getByTestId("signout-everywhere").click();
  await page.getByTestId("signout-everywhere-submit").click();
  await page.waitForURL(/\/login\?signed_out=everywhere$/);
  await expect(page.getByTestId("signed-out")).toHaveText("You were signed out of all devices.");
  await phone.goto("/profile");
  await phone.waitForURL(/\/login\?signed_out=session$/);
  await phoneContext.close();
});
