import { expect, type Page } from "@playwright/test";

export async function waitForAppHydration(page: Page): Promise<void> {
  await expect(page.locator('.app-shell[data-shortcuts-ready="true"]')).toBeVisible();
}

export async function signIn(page: Page): Promise<void> {
  await page.goto("/login");
  await page.waitForLoadState("networkidle");
  await expect(page.getByTestId("dev-login")).toBeVisible();
  await page.getByTestId("dev-login").click();
  await page.waitForURL("**/git/repos");
  await waitForAppHydration(page);
}

export async function navigateToApp(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await waitForAppHydration(page);
}
