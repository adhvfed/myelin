import { expect, test } from "@playwright/test";

test("development login opens a real edge-backed session", async ({ page }) => {
  await page.goto("/login");
  await page.waitForLoadState("networkidle");

  await expect(page.getByTestId("dev-login")).toBeVisible();
  await page.getByTestId("dev-login").click();

  await page.waitForURL("**/git/repos");
  await expect(page).toHaveTitle("Code · Myelin");
  await expect(page.getByRole("heading", { name: "Repositories" })).toBeVisible();
  await expect(page.getByText("Myelin Developer", { exact: true })).toBeVisible();
  await expect(page.getByText("fr-par", { exact: true })).toBeVisible();
  await expect(page.getByTestId("repos-empty")).toBeVisible();
  expect(await page.evaluate(() => document.cookie)).not.toContain("myelin_session");
});
