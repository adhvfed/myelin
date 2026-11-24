import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { systemClient } from "../src/context.js";
import { integer, record, string, type JsonRecord } from "../src/json.js";
import { failedOperatorLink, operatorLink } from "../src/operator.js";
import { systemTestConfig } from "../src/config.js";
import type { SystemResponse } from "../src/client.js";

const anonymous = { authenticated: false } as const;

function uniqueEmail(name: string): string {
  return `${name}-${randomUUID().slice(0, 8)}@example.com`;
}

function codeOf(link: string): string {
  const code = new URL(link).pathname.split("/join/")[1];
  return string(code, "invitation code");
}

function expectNoStore(response: SystemResponse<unknown>): void {
  expect(response.headers.get("cache-control")).toBe("no-store");
}

function expectSession(response: SystemResponse<JsonRecord>): string {
  expect(response.body).toMatchObject({ token_type: "Bearer", scheme: "session" });
  const expiresAt = integer(response.body.expires_at, "session expiry");
  expect(expiresAt).toBeLessThanOrEqual(Math.floor(Date.now() / 1000) + 8 * 60 * 60 + 5);
  expectNoStore(response);
  return string(response.body.access_token, "session token");
}

async function signIn(email: string, password: string, expectedStatus = 200) {
  return systemClient.json("/v1/auth/password/login", {
    method: "POST",
    ...anonymous,
    body: { email, password },
    expectedStatus,
  });
}

async function whoami(token: string): Promise<JsonRecord> {
  return (await systemClient.json("/v1/whoami", { token, tokenScheme: "session" })).body;
}

describe("a person's account", () => {
  test("is created from an invitation, signs in with a password, invites a teammate, and is reset by the operator", async () => {
    const config = await systemClient.json("/v1/auth/config", anonymous);
    expect(config.body).toMatchObject({ password_login_enabled: true });
    expect(config.body).not.toHaveProperty("token_login_enabled");

    // The operator invites the organization's first person.
    const adaEmail = uniqueEmail("ada");
    const adaCode = codeOf(await operatorLink("invite", adaEmail.toUpperCase()));

    const invitation = await systemClient.json(`/v1/auth/invitations/${adaCode}`, anonymous);
    expect(invitation.body).toMatchObject({ kind: "join", email: adaEmail });
    expect(string(invitation.body.organization, "organization name")).not.toBe("");
    expect(integer(invitation.body.expires_at, "invitation expiry")).toBeGreaterThan(
      Math.floor(Date.now() / 1000) + 6 * 24 * 60 * 60,
    );
    expectNoStore(invitation);

    const accept = `/v1/auth/invitations/${adaCode}/accept`;
    const ambiguous = await systemClient.json(accept, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Ada Lovelace", password: "a long passphrase", role: "admin" },
      expectedStatus: 400,
    });
    expect(ambiguous.body).toMatchObject({ error: { code: "bad_request" } });
    const weak = await systemClient.json(accept, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Ada Lovelace", password: "too short" },
      expectedStatus: 422,
    });
    expect(weak.body).toMatchObject({ error: { code: "weak_password" } });

    const joined = await systemClient.json(accept, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Ada Lovelace", password: "analytical engine" },
    });
    const adaSession = expectSession(joined);
    const ada = await whoami(adaSession);
    expect(ada).toMatchObject({
      display_name: "Ada Lovelace",
      email: adaEmail,
      kind: "human",
      tenant: systemTestConfig.tenant,
      region: systemTestConfig.region,
    });

    // The link worked once and is now gone.
    await systemClient.json(`/v1/auth/invitations/${adaCode}`, { ...anonymous, expectedStatus: 404 });
    await systemClient.json(accept, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Eve", password: "analytical engine" },
      expectedStatus: 404,
    });

    // Ada signs in as herself; the email is matched however she types it.
    const signedIn = await signIn(` ${adaEmail.toUpperCase()} `, "analytical engine");
    expect(await whoami(expectSession(signedIn))).toMatchObject({ principal_id: ada.principal_id });

    const wrongPassword = await signIn(adaEmail, "difference engine", 401);
    const unknownEmail = await signIn(uniqueEmail("nobody"), "analytical engine", 401);
    expect(wrongPassword.body).toEqual(unknownEmail.body);
    expectNoStore(wrongPassword);
    await systemClient.json("/v1/auth/password/login", {
      method: "POST",
      ...anonymous,
      body: { email: adaEmail, password: "analytical engine", remember: true },
      expectedStatus: 400,
    });

    // Ada invites a teammate over HTTP; an operator token is not a person and cannot.
    const graceEmail = uniqueEmail("grace");
    await systemClient.json("/v1/invitations", {
      method: "POST",
      body: { email: graceEmail },
      expectedStatus: 403,
    });
    const asAda = { token: adaSession, tokenScheme: "session" } as const;
    await systemClient.json("/v1/invitations", {
      method: "POST",
      ...asAda,
      body: { email: "not an email" },
      expectedStatus: 400,
    });
    await systemClient.json("/v1/invitations", {
      method: "POST",
      ...asAda,
      body: { email: graceEmail, kind: "reset" },
      expectedStatus: 400,
    });
    const invited = await systemClient.json("/v1/invitations", {
      method: "POST",
      ...asAda,
      body: { email: graceEmail },
      expectedStatus: 201,
    });
    expect(invited.body).toMatchObject({ email: graceEmail });
    integer(invited.body.expires_at, "teammate invitation expiry");
    const graceCode = codeOf(string(invited.body.url, "teammate invitation link"));

    const graceInvitation = await systemClient.json(`/v1/auth/invitations/${graceCode}`, anonymous);
    expect(graceInvitation.body).toMatchObject({
      kind: "join",
      email: graceEmail,
      organization: invitation.body.organization,
    });
    const graceJoined = await systemClient.json(`/v1/auth/invitations/${graceCode}/accept`, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Grace Hopper", password: "compile the future" },
    });
    const grace = await whoami(expectSession(graceJoined));
    expect(grace).toMatchObject({
      display_name: "Grace Hopper",
      email: graceEmail,
      tenant: systemTestConfig.tenant,
    });
    expect(grace.principal_id).not.toBe(ada.principal_id);
    expectSession(await signIn(graceEmail, "compile the future"));

    // An existing account cannot be invited again, by a member or by the operator.
    const again = await systemClient.json("/v1/invitations", {
      method: "POST",
      ...asAda,
      body: { email: graceEmail.toUpperCase() },
      expectedStatus: 409,
    });
    expect(again.body).toMatchObject({ error: { code: "conflict" } });
    const operatorAgain = await failedOperatorLink("invite", graceEmail);
    expect(operatorAgain.code).not.toBe(0);
    expect(operatorAgain.stdout).toBe("");

    // The operator resets Ada's password.
    const missing = await failedOperatorLink("reset-link", uniqueEmail("nobody"));
    expect(missing.code).not.toBe(0);
    expect(missing.stdout).toBe("");
    const resetCode = codeOf(await operatorLink("reset-link", adaEmail));
    const reset = await systemClient.json(`/v1/auth/invitations/${resetCode}`, anonymous);
    expect(reset.body).toMatchObject({ kind: "reset", email: adaEmail });
    const resetAccept = `/v1/auth/invitations/${resetCode}/accept`;
    await systemClient.json(resetAccept, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Not Ada", password: "a fresh passphrase" },
      expectedStatus: 400,
    });
    const afterReset = await systemClient.json(resetAccept, {
      method: "POST",
      ...anonymous,
      body: { password: "a fresh passphrase" },
    });
    expect(await whoami(expectSession(afterReset))).toMatchObject({
      principal_id: ada.principal_id,
      display_name: "Ada Lovelace",
    });
    await signIn(adaEmail, "analytical engine", 401);
    expectSession(await signIn(adaEmail, "a fresh passphrase"));
    await systemClient.json(resetAccept, {
      method: "POST",
      ...anonymous,
      body: { password: "yet another passphrase" },
      expectedStatus: 404,
    });

    const unknown = await systemClient.json(`/v1/auth/invitations/${"A".repeat(43)}`, {
      ...anonymous,
      expectedStatus: 404,
    });
    expect(record(unknown.body.error, "unknown invitation error")).toMatchObject({
      code: "not_found",
    });
  });

  test("links are never echoed back and oversized sign-in bodies are refused", async () => {
    const code = codeOf(await operatorLink("invite", uniqueEmail("hedy")));
    for (const [method, path] of [
      ["POST", `/v1/auth/invitations/${code}`],
      ["DELETE", `/v1/auth/invitations/${code}`],
      ["GET", `/v1/auth/invitations/${code}/accept`],
    ] as const) {
      const response = await systemClient.json(path, { method, ...anonymous, expectedStatus: 404 });
      expect(JSON.stringify(response.body)).not.toContain(code);
    }
    await systemClient.json(`/v1/auth/invitations/${code}`, anonymous);

    await systemClient.json("/v1/auth/password/login", {
      method: "POST",
      ...anonymous,
      body: { email: uniqueEmail("big"), password: "x".repeat(17 * 1024) },
      expectedStatus: 413,
    });
  });
});
