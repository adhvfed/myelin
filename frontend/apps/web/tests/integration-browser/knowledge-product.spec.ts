import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";
import { navigateToApp, signIn, waitForAppHydration } from "./session";

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
  const headers = { authorization: `Bearer ${token}`, "x-myelin-token-scheme": "agent" };
  const companionResponse = await request.post(`${edgeUrl}/v1/knowledge/pages`, {
    headers: { ...headers, "idempotency-key": randomUUID() },
    data: { title: `Release evidence ${suffix}`, template: "blank", visibility: "team" },
  });
  const companionText = await companionResponse.text();
  expect(companionResponse.status(), companionText).toBe(201);
  const companion = (JSON.parse(companionText) as { page: { id: string; ref: string } }).page;

  await signIn(page);
  await navigateToApp(page, "/knowledge");
  await page.getByRole("button", { name: "Create a page" }).click();
  await page.getByRole("textbox", { name: "Page title" }).fill(title);
  await page.getByText("Runbook", { exact: true }).click();
  await page.getByLabel("Visibility").selectOption("team");
  await page.getByRole("button", { name: "Create page" }).click();
  await page.waitForURL(/\/knowledge\?page=[0-9A-HJKMNP-TV-Z]{26}$/);

  await expect(page.getByRole("textbox", { name: "Heading block 3" })).toContainText("Response");
  await expect(page.getByRole("textbox", { name: "Task block 8" }))
    .toContainText("Follow-up work has an owner and is linked to the incident.");
  const response = page.getByRole("textbox", { name: "Numbered list block 4" });
  await response.fill(edited);
  await page.getByRole("button", { name: "Link related work" }).click();
  await page.getByRole("textbox", { name: "Canonical Myelin reference" }).fill(companion.ref);
  await page.getByRole("button", { name: "Add", exact: true }).click();
  await expect(page.getByRole("link", { name: `Reference: Knowledge · ${companion.id.slice(-6)}` }))
    .toHaveAttribute("href", `/knowledge?page=${companion.id}`);
  await expect(page.locator(".knowledge-save-state")).toHaveText("Saved", { timeout: 10_000 });
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByRole("textbox", { name: "Page title" })).toHaveValue(title);
  await expect(page.getByRole("textbox", { name: "Numbered list block 4" })).toContainText(edited);
  await expect(page.getByRole("link", { name: `Reference: Knowledge · ${companion.id.slice(-6)}` })).toBeVisible();

  const pages = await request.get(`${edgeUrl}/v1/knowledge/pages?limit=100`, { headers });
  const pageText = await pages.text(); expect(pages.status(), pageText).toBe(200);
  const created = (JSON.parse(pageText) as { items: Array<{ id: string; ref: string; title: string; version: number }> }).items.find((item) => item.title === title);
  expect(created).toMatchObject({ title, version: 2 });

  const document = await request.get(`${edgeUrl}/v1/knowledge/pages/${created!.id}`, { headers });
  const documentText = await document.text(); expect(document.status(), documentText).toBe(200);
  expect(JSON.parse(documentText).page.blocks).toEqual(expect.arrayContaining([expect.objectContaining({ markdown: edited, state: "active" })]));
  expect(JSON.parse(documentText).page.blocks).toEqual(expect.arrayContaining([
    expect.objectContaining({ markdown: "Related work: \uFFFC", references: [companion.ref], state: "active" }),
  ]));

  await expect.poll(async () => {
    const backlinks = await request.get(
      `${edgeUrl}/v1/refs/backlinks?ref=${encodeURIComponent(companion.ref)}&limit=100`,
      { headers },
    );
    if (!backlinks.ok()) return false;
    const items = (JSON.parse(await backlinks.text()) as { items: Array<Record<string, unknown>> }).items;
    return items.some((item) => item.root_ref === created!.ref && item.target_ref === companion.ref && item.relation === "links");
  }, { message: "the related page should appear through the live reference projection" }).toBe(true);
});
