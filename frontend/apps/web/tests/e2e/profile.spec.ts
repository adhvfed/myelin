import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

async function devLogin(page: Page) { await page.goto("/login"); await page.waitForLoadState("networkidle"); await page.getByTestId("dev-login").click(); await page.waitForURL("**/git/repos"); }

test.describe("Profile", () => {
  test("opens from the account menu and explains identity, residency, and session security", async ({ page }) => {
    await devLogin(page);
    await page.getByRole("button", { name: "Dev Operator" }).click();
    await page.getByRole("menuitem", { name: "Profile" }).click();
    await page.waitForURL("**/profile");
    await expect(page.getByRole("heading", { name: "Dev Operator", level: 1 })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Data residency" })).toBeVisible();
    await expect(page.locator("#main").getByText("eu-west", { exact: true })).toBeVisible();
    await expect(page.getByText("Browser JavaScript never receives it.", { exact: false })).toBeVisible();
    const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"]).analyze();
    expect(results.violations).toEqual([]);
  });

  test("persists appearance and offers a real sign-out action", async ({ page }) => {
    await devLogin(page); await page.goto("/profile");
    await page.getByRole("radio", { name: /Light/ }).click();
    await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
    await page.reload();
    await expect(page.getByRole("radio", { name: /Light/ })).toHaveAttribute("aria-checked", "true");
    await page.getByRole("button", { name: "Sign out of Myelin" }).click();
    await page.waitForURL("**/login");
    await expect(page.getByRole("heading", { name: "Sign in to Myelin" })).toBeVisible();
  });
});
