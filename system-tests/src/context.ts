import { createHash, randomBytes } from "node:crypto";

import { SystemTestClient } from "./client.js";
import { string } from "./json.js";
import { systemTestConfig } from "./config.js";

export const systemClient = new SystemTestClient(systemTestConfig);
export const reviewerClient = new SystemTestClient({
  ...systemTestConfig,
  token: systemTestConfig.reviewerToken,
});

export async function browserApprovedCliClient(
  approver: SystemTestClient = systemClient,
): Promise<SystemTestClient> {
  const codeVerifier = randomBytes(32).toString("base64url");
  const codeChallenge = createHash("sha256").update(codeVerifier, "utf8").digest("base64url");
  const started = await systemClient.json("/v1/auth/device/authorization", {
    method: "POST",
    authenticated: false,
    body: { code_challenge: codeChallenge },
    expectedStatus: 201,
  });
  const deviceCode = string(started.body.device_code, "CLI device code");
  const userCode = string(started.body.user_code, "browser approval code");

  await approver.json("/v1/auth/device/approval", {
    method: "POST",
    body: { user_code: userCode },
  });
  const claimed = await systemClient.json("/v1/auth/device/token", {
    method: "POST",
    authenticated: false,
    body: { device_code: deviceCode, code_verifier: codeVerifier },
  });

  return new SystemTestClient({
    ...systemTestConfig,
    token: string(claimed.body.access_token, "browser-approved CLI token"),
    tokenScheme: "session",
  });
}

export function uniqueName(prefix: string): string {
  const run = systemTestConfig.runId.replaceAll("-", "").slice(0, 10).toLowerCase();
  return `${prefix}-${Date.now().toString(36)}-${run}`;
}
