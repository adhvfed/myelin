import { createHash, randomBytes, randomUUID } from "node:crypto";
import { expect, type APIRequestContext } from "@playwright/test";

export type JsonObject = Record<string, unknown>;

function requiredEnvironment(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required; run this test with fed test:integration`);
  return value;
}

export const integrationEdgeUrl = requiredEnvironment("MYELIN_INTEGRATION_EDGE_URL")
  .replace(/\/$/, "");
const operatorToken = requiredEnvironment("MYELIN_BROWSER_EDGE_TOKEN");

export async function postProductJson(
  request: APIRequestContext,
  path: string,
  body: unknown,
  options: { token?: string; scheme?: string; status: number },
): Promise<JsonObject> {
  const response = await request.post(`${integrationEdgeUrl}${path}`, {
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

export async function browserApprovedSession(request: APIRequestContext): Promise<string> {
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  const started = await postProductJson(
    request,
    "/v1/auth/device/authorization",
    { code_challenge: challenge },
    { status: 201 },
  );
  await postProductJson(
    request,
    "/v1/auth/device/approval",
    { user_code: String(started.user_code) },
    { token: operatorToken, scheme: "agent", status: 200 },
  );
  const claimed = await postProductJson(
    request,
    "/v1/auth/device/token",
    { device_code: String(started.device_code), code_verifier: verifier },
    { status: 200 },
  );
  expect(claimed).toMatchObject({ scheme: "session", token_type: "Bearer" });
  return String(claimed.access_token);
}
