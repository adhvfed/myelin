import { createHash, randomBytes } from "node:crypto";

import { describe, expect, test } from "vitest";

import { systemClient } from "../src/context.js";
import { integer, string } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

function verifier(): string {
  return randomBytes(32).toString("base64url");
}

function challenge(codeVerifier: string): string {
  return createHash("sha256").update(codeVerifier, "utf8").digest("base64url");
}

describe("human and CLI authentication", () => {
  test("a browser approves one short-lived CLI session without sharing its credential", async () => {
    const browserIdentity = await systemClient.json("/v1/whoami");
    const browserCredentialExpiry = integer(
      browserIdentity.body.expires_at,
      "approving credential expiry",
    );
    const authConfig = await systemClient.json("/v1/auth/config", { authenticated: false });
    expect(authConfig.body).toMatchObject({ cli_login_enabled: true });

    const codeVerifier = verifier();
    const started = await systemClient.json("/v1/auth/device/authorization", {
      method: "POST",
      authenticated: false,
      body: { code_challenge: challenge(codeVerifier) },
      expectedStatus: 201,
    });
    const deviceCode = string(started.body.device_code, "CLI device code");
    const userCode = string(started.body.user_code, "browser user code");
    const verificationUri = string(started.body.verification_uri, "browser verification URI");
    expect(userCode).toMatch(/^[A-HJ-NP-Z2-9]{4}-[A-HJ-NP-Z2-9]{4}$/);
    expect(started.body).toMatchObject({ expires_in: 600, interval: 2 });
    expect(started.body.verification_uri_complete).toBe(`${verificationUri}?code=${userCode}`);
    expect(started.headers.get("cache-control")).toBe("no-store");

    const pending = await systemClient.json("/v1/auth/device/token", {
      method: "POST",
      authenticated: false,
      body: { device_code: deviceCode, code_verifier: codeVerifier },
      expectedStatus: 202,
    });
    expect(pending.body).toEqual({ status: "authorization_pending", interval: 2 });

    const anotherCliCannotClaimIt = await systemClient.json("/v1/auth/device/token", {
      method: "POST",
      authenticated: false,
      body: { device_code: deviceCode, code_verifier: verifier() },
      expectedStatus: 401,
    });
    expect(anotherCliCannotClaimIt.body).toMatchObject({ error: { code: "unauthorized" } });

    const approved = await systemClient.json("/v1/auth/device/approval", {
      method: "POST",
      body: { user_code: userCode },
    });
    expect(approved.body).toEqual({ approved: true });

    const claimed = await systemClient.json("/v1/auth/device/token", {
      method: "POST",
      authenticated: false,
      body: { device_code: deviceCode, code_verifier: codeVerifier },
    });
    const cliToken = string(claimed.body.access_token, "fresh CLI session token");
    const cliCredentialExpiry = integer(claimed.body.expires_at, "CLI credential expiry");
    expect(claimed.body).toMatchObject({ token_type: "Bearer", scheme: "session" });
    expect(cliToken).not.toBe(systemTestConfig.token);
    expect(cliCredentialExpiry).toBeLessThanOrEqual(browserCredentialExpiry);
    expect(claimed.headers.get("cache-control")).toBe("no-store");

    const cliIdentity = await systemClient.json("/v1/whoami", {
      token: cliToken,
      tokenScheme: "session",
    });
    expect(cliIdentity.body).toMatchObject({
      principal_id: systemTestConfig.principal,
      tenant: systemTestConfig.tenant,
      region: systemTestConfig.region,
      kind: "human",
    });

    const replay = await systemClient.json("/v1/auth/device/token", {
      method: "POST",
      authenticated: false,
      body: { device_code: deviceCode, code_verifier: codeVerifier },
      expectedStatus: 401,
    });
    expect(replay.body).toMatchObject({ error: { code: "unauthorized" } });
  });
});
