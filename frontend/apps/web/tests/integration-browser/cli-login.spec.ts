import { createHash, randomBytes } from "node:crypto";

import AxeBuilder from "@axe-core/playwright";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

type JsonObject = Record<string, unknown>;

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required; run this test with fed test:integration`);
  return value;
}

function object(value: unknown): JsonObject {
  expect(value).not.toBeNull();
  expect(typeof value).toBe("object");
  expect(Array.isArray(value)).toBe(false);
  return value as JsonObject;
}

async function post(
  request: APIRequestContext,
  edgeUrl: string,
  path: string,
  data: unknown,
): Promise<{ status: number; body: JsonObject }> {
  const response = await request.post(`${edgeUrl}${path}`, {
    data,
    failOnStatusCode: false,
  });
  const text = await response.text();
  return { status: response.status(), body: object(JSON.parse(text)) };
}

const edgeUrl = requiredEnvironment("MYELIN_INTEGRATION_EDGE_URL").replace(/\/$/, "");

async function expectAccessible(page: Page, context: string) {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  expect(results.violations, `${context}: ${JSON.stringify(results.violations, null, 2)}`).toEqual([]);
}

test("a person signs in once and hands their CLI a fresh, separate session", async ({
  page,
  request,
}) => {
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");

  const start = await post(request, edgeUrl, "/v1/auth/device/authorization", {
    code_challenge: challenge,
  });
  expect(start.status).toBe(201);
  const deviceCode = String(start.body.device_code);
  const userCode = String(start.body.user_code);
  const approvalUrl = String(start.body.verification_uri_complete);
  expect(userCode).toMatch(/^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/);

  await page.goto(approvalUrl);
  await expect(page.getByRole("heading", { name: "Connect the Myelin CLI" })).toBeVisible();
  await expect(page.getByTestId("cli-user-code")).toHaveText(userCode);
  await expect(page.getByText("Sign in with your organization account")).toBeVisible();

  await page.getByTestId("cli-sign-in").click();
  await expect(page).toHaveURL(/\/login\?.*return_to=/);
  await page.getByTestId("dev-login").click();

  await expect(page).toHaveURL(new RegExp(`/cli/auth\\?code=${userCode}`));
  await expect(page.getByText("Myelin Developer", { exact: true })).toBeVisible();
  await expect(page.getByText("fr-par", { exact: true })).toBeVisible();
  await expect(page.getByTestId("cli-approve")).toBeVisible();
  await expectAccessible(page, "signed-in CLI approval");

  await page.getByTestId("cli-approve").click();
  await expect(page.getByTestId("cli-approval-approved")).toContainText("CLI connected");
  await expect(page.getByTestId("cli-approve")).toHaveCount(0);

  const claim = await post(request, edgeUrl, "/v1/auth/device/token", {
    device_code: deviceCode,
    code_verifier: verifier,
  });
  expect(claim.status).toBe(200);
  expect(claim.body).toMatchObject({ token_type: "Bearer", scheme: "session" });
  const cliSession = String(claim.body.access_token);
  expect(cliSession.length).toBeGreaterThan(32);

  const whoami = await request.get(`${edgeUrl}/v1/whoami`, {
    headers: {
      authorization: `Bearer ${cliSession}`,
      "x-myelin-token-scheme": "session",
    },
  });
  expect(whoami.status()).toBe(200);
  expect(object(await whoami.json())).toMatchObject({
    principal_id: requiredEnvironment("MYELIN_BROWSER_PRINCIPAL"),
    tenant: requiredEnvironment("MYELIN_BROWSER_TENANT"),
    region: "fr-par",
    kind: "human",
  });

  const replay = await post(request, edgeUrl, "/v1/auth/device/token", {
    device_code: deviceCode,
    code_verifier: verifier,
  });
  expect(replay.status).toBe(401);
  expect(replay.body).toMatchObject({
    error: { code: "unauthorized" },
  });
});
