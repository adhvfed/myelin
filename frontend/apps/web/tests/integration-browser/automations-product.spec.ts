import { createHash, randomBytes, randomUUID } from "node:crypto";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { signIn, waitForAppHydration } from "./session";

type JsonObject = Record<string, unknown>;

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required; run this test with fed test:integration`);
  return value;
}

const edgeUrl = requiredEnvironment("MYELIN_INTEGRATION_EDGE_URL").replace(/\/$/, "");
const token = requiredEnvironment("MYELIN_BROWSER_EDGE_TOKEN");

async function post(
  request: APIRequestContext,
  path: string,
  body: unknown,
  options: { token?: string; scheme?: string; status: number },
): Promise<JsonObject> {
  const response = await request.post(`${edgeUrl}${path}`, {
    headers: {
      ...(options.token
        ? {
            authorization: `Bearer ${options.token}`,
            "x-myelin-token-scheme": options.scheme ?? "agent",
          }
        : {}),
      "idempotency-key": randomUUID(),
    },
    data: body,
    failOnStatusCode: false,
  });
  const text = await response.text();
  expect(response.status(), `POST ${path}: ${text}`).toBe(options.status);
  return JSON.parse(text) as JsonObject;
}

async function browserApprovedSession(request: APIRequestContext): Promise<string> {
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  const started = await post(
    request,
    "/v1/auth/device/authorization",
    { code_challenge: challenge },
    { status: 201 },
  );
  await post(
    request,
    "/v1/auth/device/approval",
    { user_code: String(started.user_code) },
    { token, scheme: "agent", status: 200 },
  );
  const claimed = await post(
    request,
    "/v1/auth/device/token",
    { device_code: String(started.device_code), code_verifier: verifier },
    { status: 200 },
  );
  expect(claimed).toMatchObject({ scheme: "session", token_type: "Bearer" });
  return String(claimed.access_token);
}

async function expectAccessible(page: Page, context: string): Promise<void> {
  const result = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(
    result.violations,
    `${context}: ${JSON.stringify(result.violations, null, 2)}`,
  ).toEqual([]);
}

test("an automation owner governs durable agent work without configuring an integration key", async ({
  page,
  request,
}) => {
  const suffix = `${Date.now().toString(36)}-${randomUUID().slice(0, 6)}`;
  const task = `Review ownership changes for ${suffix} and leave a concise handoff.`;
  const sessionToken = await browserApprovedSession(request);
  const owner = { token: sessionToken, scheme: "session" };
  const createdAgent = await post(request, "/v1/agents", {
    name: `ownership-companion-${suffix}`,
    runtime: "hosted",
    tools: ["ci.read_run"],
  }, { ...owner, status: 201 });
  const agent = createdAgent.agent as JsonObject;
  const agentId = String(agent.id);
  expect(agentId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);

  const createdAutomation = await post(request, "/v1/triggers", {
    event_type: "issue.issue.updated",
    filter: `payload.change_kind == '${suffix}'`,
    run_as_agent_id: agentId,
    task,
    budget_minor_units: 125_000,
    max_firings: 8,
    max_causal_depth: 3,
    delegation_caveats: ["issue:read"],
    require_human_approval: true,
  }, { ...owner, status: 201 });
  const automation = (createdAutomation.trigger as JsonObject);
  const automationId = String(automation.id);
  expect(automationId).toMatch(/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/);

  await signIn(page);
  await page.getByRole("link", { name: "Automations" }).click();
  await page.waitForURL("**/automations");
  const row = page.getByTestId("automation-row").filter({ hasText: task });
  await expect(row).toBeVisible();
  await expect(row).toContainText("Active");
  await expect(row).toContainText("0 / 8 firings");
  await expectAccessible(page, "automation list");

  await row.click();
  await page.waitForURL(`**/automations/${automationId}`);
  await expect(page.getByRole("heading", { level: 1, name: task })).toBeVisible();
  await expect(page.getByText("Required", { exact: true })).toBeVisible();
  await expect(page.getByText("Refused", { exact: true })).toBeVisible();
  await expect(page.getByText("No matching events have reserved work yet.")).toBeVisible();

  await page.getByRole("button", { name: "Pause" }).click();
  await expect(page.getByTitle("State: Paused")).toBeVisible();
  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByTitle("State: Paused")).toBeVisible();

  await page.getByRole("button", { name: "Resume" }).click();
  await expect(page.getByTitle("State: Active")).toBeVisible();
  await page.getByRole("button", { name: "Disable" }).click();
  const dialog = page.getByRole("alertdialog", { name: "Disable this automation?" });
  await expect(dialog).toContainText("irreversible");
  await dialog.getByRole("button", { name: "Disable automation" }).click();
  await expect(page.getByTitle("State: Disabled")).toBeVisible();
  await expect(page.getByRole("button", { name: "Resume" })).toHaveCount(0);

  await page.reload();
  await waitForAppHydration(page);
  await expect(page.getByTitle("State: Disabled")).toBeVisible();
  await expectAccessible(page, "disabled automation detail");
});
