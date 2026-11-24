import { expect, test, type Page } from "@playwright/test";

// A person's account, start to finish, against the real stack: the operator's first invitation
// link, a chosen password, an ordinary sign-in, and an invitation for a teammate.

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required; run this test with fed test:integration`);
  return value;
}

const inviteUrl = requiredEnvironment("MYELIN_BROWSER_INVITE_URL");
const inviteEmail = requiredEnvironment("MYELIN_BROWSER_INVITE_EMAIL");
const password = "a real-stack test password";

async function signIn(page: Page, email: string, secret: string) {
  await page.goto("/login");
  await page.waitForLoadState("networkidle");
  await page.getByTestId("email-input").fill(email);
  await page.getByTestId("password-input").fill(secret);
  await page.getByTestId("password-login").click();
}

test("an invited person creates an account, signs in with it, and invites a teammate", async ({
  page,
  browser,
}) => {
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
  const teammateLink = await page.getByTestId("invite-url").inputValue();
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
});
