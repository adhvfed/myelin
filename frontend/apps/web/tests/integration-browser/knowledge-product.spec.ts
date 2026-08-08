import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required; run this test with fed test:integration`);
  return value;
}

const edgeUrl = requiredEnvironment("MYELIN_INTEGRATION_EDGE_URL").replace(/\/$/, "");
const token = requiredEnvironment("MYELIN_BROWSER_EDGE_TOKEN");

test("a signed-in engineer creates, edits, and resumes an encrypted durable Knowledge page", async ({ page, request }) => {
  const suffix = `${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const title = `EU service runbook ${suffix}`;
  const edited = `Confirm regional placement ${suffix}, then establish an incident lead.`;

  await page.goto("/login"); await page.waitForLoadState("networkidle");
  await page.getByTestId("dev-login").click(); await page.waitForURL("**/git/repos");
  await page.goto("/knowledge");
  await page.getByRole("button", { name: "Create a page" }).click();
  await page.getByRole("textbox", { name: "Page title" }).fill(title);
  await page.getByText("Runbook", { exact: true }).click();
  await page.getByLabel("Visibility").selectOption("team");
  await page.getByRole("button", { name: "Create page" }).click();
  await page.waitForURL(/\/knowledge\?page=[0-9A-HJKMNP-TV-Z]{26}$/);

  const response = page.getByRole("textbox", { name: "Numbered list block 4" });
  await response.fill(edited);
  await expect(page.locator(".knowledge-save-state")).toHaveText("Saved", { timeout: 10_000 });
  await page.reload();
  await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue(title);
  await expect(page.getByRole("textbox", { name: "Numbered list block 4" })).toContainText(edited);

  const pages = await request.get(`${edgeUrl}/v1/knowledge/pages?limit=100`, { headers: { authorization: `Bearer ${token}`, "x-myelin-token-scheme": "agent" } });
  const pageText = await pages.text(); expect(pages.status(), pageText).toBe(200);
  const created = (JSON.parse(pageText) as { items: Array<{ id: string; title: string; version: number }> }).items.find((item) => item.title === title);
  expect(created).toMatchObject({ title, version: 2 });

  const document = await request.get(`${edgeUrl}/v1/knowledge/pages/${created!.id}`, { headers: { authorization: `Bearer ${token}`, "x-myelin-token-scheme": "agent" } });
  const documentText = await document.text(); expect(document.status(), documentText).toBe(200);
  expect(JSON.parse(documentText).page.blocks).toEqual(expect.arrayContaining([expect.objectContaining({ markdown: edited, state: "active" })]));
});
