import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

const EDGE = `http://127.0.0.1:${process.env.DEV_EDGE_PORT ?? 8787}`;
async function devLogin(page: Page) { await page.goto("/login"); await page.waitForLoadState("networkidle"); await page.getByTestId("dev-login").click(); await page.waitForURL("**/git/repos"); }
async function expectAccessible(page: Page, context: string) { const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"]).analyze(); expect(results.violations, `${context}: ${JSON.stringify(results.violations, null, 2)}`).toEqual([]); }

test.describe("Knowledge workspace", () => {
  test.afterEach(async ({ request }) => { expect((await request.post(`${EDGE}/__test/config`, { data: { resetKnowledge: true, forceUnauthorized: false } })).ok()).toBe(true); });

  test("opens organisational context in a cohesive, accessible writing workspace", async ({ page }) => {
    await devLogin(page);
    await page.getByRole("navigation", { name: "Primary" }).getByRole("link", { name: "Knowledge" }).click();
    await expect(page.getByTestId("knowledge-screen")).toBeVisible();
    await expect(page.getByText("Under construction", { exact: true })).toHaveCount(0);
    await page.getByRole("link", { name: /Engineering principles/ }).click();
    await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue("Engineering principles");
    await expect(page.getByRole("textbox", { name: "Callout block 3" })).toContainText("Quality is a product feature");
    await expectAccessible(page, "seeded Knowledge page");
  });

  test("creates from a useful template and preserves an edited draft across reload", async ({ page, request }) => {
    expect((await request.post(`${EDGE}/__test/config`, { data: { emptyKnowledge: true } })).ok()).toBe(true);
    await devLogin(page); await page.goto("/knowledge");
    await expect(page.getByText("Your knowledge base is ready")).toBeVisible();
    await page.getByRole("button", { name: "Create the first page" }).click();
    await page.getByRole("textbox", { name: "Page title" }).fill("Payments incident runbook");
    await page.getByText("Runbook", { exact: true }).click();
    await page.getByLabel("Visibility").selectOption("team");
    await page.getByRole("button", { name: "Create page" }).click();
    await page.waitForURL(/\/knowledge\?page=[0-9A-HJKMNP-TV-Z]{26}$/);
    const responseBlock = page.getByRole("textbox", { name: "Numbered list block 4" });
    await expect(responseBlock).toContainText("Confirm the alert");
    await responseBlock.fill("Confirm the alert, open an incident topic, and assign an incident lead.");
    await page.getByRole("button", { name: "Link related work" }).click();
    await page.getByRole("textbox", { name: "Canonical Myelin reference" })
      .fill("myelin://acme/issue/issue/MYL-102");
    await page.getByRole("button", { name: "Add", exact: true }).click();
    await expect(page.getByRole("link", { name: "Reference: MYL-102" }))
      .toHaveAttribute("href", "/issues?state=all&key=MYL-102");
    const saveState = page.locator(".knowledge-save-state");
    await expect(saveState).toHaveText(/Saving|Saved|Up to date/, { timeout: 5_000 });
    await expect(saveState).toHaveText("Saved", { timeout: 10_000 });
    await page.reload();
    await expect(page.getByRole("textbox", { name: "Numbered list block 4" })).toContainText("open an incident topic");
    await expect(page.getByRole("link", { name: "Reference: MYL-102" })).toBeVisible();
    await expect(page.getByText("Team", { exact: true }).first()).toBeVisible();
  });

  test("a response-lost page creation retries without cloning the page", async ({ page, request }) => {
    const configured = await request.post(`${EDGE}/__test/config`, {
      data: { emptyKnowledge: true, knowledgeCreateResponseLosses: 1 },
    });
    expect(configured.ok()).toBe(true);
    await devLogin(page); await page.goto("/knowledge");

    await page.getByRole("button", { name: "Create the first page" }).click();
    await page.getByRole("textbox", { name: "Page title" }).fill("Retry-safe runbook");
    await page.getByText("Runbook", { exact: true }).click();
    await page.getByRole("button", { name: "Create page" }).click();
    await expect(page.getByRole("alert")).toContainText("page was not confirmed");

    await page.getByRole("button", { name: "Create page" }).click();
    await page.waitForURL(/\/knowledge\?page=[0-9A-HJKMNP-TV-Z]{26}$/);
    await page.reload();
    await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue("Retry-safe runbook");
    await expect(page.getByRole("link", { name: /Retry-safe runbook/ })).toHaveCount(1);
  });

  test("keeps edits with their pages and autosaves them after navigation", async ({ page }) => {
    await devLogin(page);
    await page.goto("/knowledge");

    await page.getByRole("link", { name: /Engineering principles/ }).click();
    const principlesTitle = "Engineering principles, locally refined";
    await page.getByRole("textbox", { name: "Page title" }).fill(principlesTitle);

    await page.getByRole("link", { name: /EU release runbook/ }).click();
    await expect(page.getByRole("textbox", { name: "Page title" }))
      .toHaveValue("EU release runbook");
    const runbookTitle = "EU release runbook, locally refined";
    await page.getByRole("textbox", { name: "Page title" }).fill(runbookTitle);

    await page.getByRole("link", { name: /Engineering principles/ }).click();
    await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue(principlesTitle);
    await expect(page.locator(".knowledge-save-state")).toHaveText("Saved", { timeout: 10_000 });

    await page.getByRole("link", { name: /EU release runbook/ }).click();
    await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue(runbookTitle);
    await expect(page.locator(".knowledge-save-state")).toHaveText("Saved", { timeout: 10_000 });

    await page.reload();
    await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue(runbookTitle);
    await page.getByRole("link", { name: /Engineering principles/ }).click();
    await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue(principlesTitle);
  });

  test("saves edits made while the previous revision is still being confirmed", async ({ page, request }) => {
    const configured = await request.post(`${EDGE}/__test/config`, {
      data: { knowledgeSaveResponseDelaysMs: [1_800] },
    });
    expect(configured.ok()).toBe(true);
    await devLogin(page);
    await page.goto("/knowledge");

    await page.getByRole("link", { name: /Engineering principles/ }).click();
    const title = page.getByRole("textbox", { name: "Page title" });
    await title.fill("Engineering principles, first revision");
    await expect(page.locator(".knowledge-save-state")).toHaveText("Saving…", { timeout: 5_000 });

    const finalTitle = "Engineering principles, refined while saving";
    await title.fill(finalTitle);
    await expect(page.locator(".knowledge-save-state")).toHaveText("Saved", { timeout: 10_000 });

    await page.reload();
    await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue(finalTitle);
  });

  test("uses page navigation first on a narrow viewport", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 760 }); await devLogin(page); await page.goto("/knowledge");
    await expect(page.getByRole("heading", { name: "Knowledge", level: 1 })).toBeVisible();
    await page.getByRole("link", { name: /EU release runbook/ }).click();
    await expect(page.getByRole("heading", { name: "Knowledge", level: 1 })).toBeHidden();
    await page.getByRole("link", { name: "Pages" }).click();
    await expect(page.getByRole("heading", { name: "Knowledge", level: 1 })).toBeVisible();
  });

  test("keeps typed content visible through an optimistic save conflict", async ({ page, request }) => {
    await devLogin(page); await page.goto("/knowledge");
    await page.getByRole("link", { name: /Engineering principles/ }).click();
    const title = page.getByRole("textbox", { name: "Page title" });
    await expect(title).toHaveValue("Engineering principles");
    const match = /page=([0-9A-HJKMNP-TV-Z]{26})/.exec(page.url());
    expect(match?.[1]).toBeTruthy();
    expect((await request.post(`${EDGE}/__test/config`, { data: { bumpKnowledgePage: match![1] } })).ok()).toBe(true);
    await title.fill("Engineering principles, refined");
    await expect(page.getByRole("alert").filter({ hasText: "This page changed elsewhere" })).toBeVisible({ timeout: 10_000 });
    await expect(title).toHaveValue("Engineering principles, refined");
    await page.getByRole("button", { name: "Keep my draft" }).click();
    await expect(page.locator(".knowledge-save-state")).toHaveText("Saved", { timeout: 10_000 });
    await page.reload();
    await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue("Engineering principles, refined");
  });

  test("an invalid page link explains itself and leads back to the knowledge base", async ({ page }) => {
    await devLogin(page);
    await page.setViewportSize({ width: 375, height: 760 });
    await page.goto("/knowledge?page=not-a-page");

    await expect(page.getByRole("heading", { name: "Page address invalid" })).toBeVisible();
    await expect(page.getByText("This link doesn’t contain a valid Myelin page address.")).toBeVisible();
    const back = page.getByRole("link", { name: "Back to pages" });
    await expect(back).toHaveAttribute("href", "/knowledge");
    await expectAccessible(page, "invalid Knowledge deep link");

    await back.click();
    await expect(page.getByRole("heading", { name: "Knowledge", level: 1 })).toBeVisible();
  });
});
