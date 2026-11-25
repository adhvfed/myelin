import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { systemClient } from "../src/context.js";
import { integer, record, string, type JsonRecord } from "../src/json.js";
import { linkIn, mailSoFar, mailTo } from "../src/mailpit.js";
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

async function expectSignedOut(token: string): Promise<void> {
  await systemClient.json("/v1/whoami", { token, tokenScheme: "session", expectedStatus: 401 });
}

async function asPerson(token: string, path: string, body: unknown, expectedStatus = 200) {
  return systemClient.json(path, {
    method: "POST",
    token,
    tokenScheme: "session",
    body,
    expectedStatus,
  });
}

describe("a person's account", () => {
  test("is created from an invitation, signs in with a password, invites a teammate, and is reset by the operator", async () => {
    const config = await systemClient.json("/v1/auth/config", anonymous);
    expect(config.body).toMatchObject({ password_login_enabled: true, mail_delivery_enabled: true });
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
    expect(invited.body).toMatchObject({ delivery: "email", email: graceEmail });
    expect(invited.body).not.toHaveProperty("url");
    integer(invited.body.expires_at, "teammate invitation expiry");
    // The link goes only to Grace's inbox, so opening it shows she holds the address.
    const [invitationMail] = await mailTo(graceEmail);
    expect(invitationMail!.subject).toBe(`Join ${string(invitation.body.organization, "organization")} on Myelin`);
    expect(invitationMail!.html).not.toMatch(/<img/i);
    const graceCode = codeOf(linkIn(invitationMail!));

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

  test("forgets a password, resets it from the emailed link, and is told it changed", async () => {
    const email = uniqueEmail("lamarr");
    const code = codeOf(await operatorLink("invite", email));
    await systemClient.json(`/v1/auth/invitations/${code}/accept`, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Hedy Lamarr", password: "frequency hopping" },
    });

    // Hedy is signed in on two devices when she forgets her password.
    const laptop = expectSession(await signIn(email, "frequency hopping"));
    const phone = expectSession(await signIn(email, "frequency hopping"));

    // Every address gets the same answer, whether or not it has an account.
    const forgot = (address: string) =>
      systemClient.json("/v1/auth/password/forgot", {
        method: "POST",
        ...anonymous,
        body: { email: address },
        expectedStatus: 202,
      });
    const nobody = uniqueEmail("nobody");
    const unknown = await forgot(nobody);
    const known = await forgot(` ${email.toUpperCase()} `);
    expect(known.body).toEqual(unknown.body);
    expect(known.body).toEqual({ status: "accepted" });
    expectNoStore(known);

    const [resetMail] = await mailTo(email);
    expect(resetMail!.subject).toBe("Reset your Myelin password");
    const resetLink = linkIn(resetMail!);
    expect(resetMail!.text).toMatch(/expires on \d{1,2} \w+ \d{4} at \d{2}:\d{2} UTC/);
    const resetCode = codeOf(resetLink);
    const reset = await systemClient.json(`/v1/auth/invitations/${resetCode}`, anonymous);
    expect(reset.body).toMatchObject({ kind: "reset", email });
    const session = expectSession(
      await systemClient.json(`/v1/auth/invitations/${resetCode}/accept`, {
        method: "POST",
        ...anonymous,
        body: { password: "spread spectrum" },
      }),
    );
    expect(await whoami(session)).toMatchObject({ email, display_name: "Hedy Lamarr" });
    await expectSignedOut(laptop);
    await expectSignedOut(phone);
    await signIn(email, "frequency hopping", 401);
    expectSession(await signIn(email, "spread spectrum"));

    const [, notice] = await mailTo(email, 2);
    expect(notice!.subject).toBe("Your Myelin password was changed");
    expect(notice!.text).toContain(email);
    expect(notice!.text).toContain("All other sessions were signed out.");
    expect(notice!.text).toMatch(/If this wasn't you, reset your password:\s+https?:\/\/\S+\/login\?help=password/);
    expect(await mailSoFar(nobody)).toEqual([]);

    // Three reset mails an hour reach one address; further requests are dropped unseen.
    for (const expected of [3, 4]) {
      await forgot(email);
      await mailTo(email, expected);
    }
    const dropped = await forgot(email);
    expect(dropped.body).toEqual(unknown.body);
    await new Promise((resolve) => setTimeout(resolve, 5_000));
    expect(await mailSoFar(email)).toHaveLength(4);
  });

  test("changes the password while signed in, and signs out of one device or all of them", async () => {
    const email = uniqueEmail("noether");
    const code = codeOf(await operatorLink("invite", email));
    const laptop = expectSession(
      await systemClient.json(`/v1/auth/invitations/${code}/accept`, {
        method: "POST",
        ...anonymous,
        body: { display_name: "Emmy Noether", password: "symmetry and conservation" },
      }),
    );
    const phone = expectSession(await signIn(email, "symmetry and conservation"));

    const wrong = await asPerson(
      laptop,
      "/v1/auth/password/change",
      { current_password: "not the password", new_password: "invariant theory" },
      403,
    );
    expect(wrong.body).toMatchObject({ error: { code: "wrong_password" } });
    await whoami(phone);

    const changed = await asPerson(laptop, "/v1/auth/password/change", {
      current_password: "symmetry and conservation",
      new_password: "invariant theory",
    });
    const fresh = expectSession(changed);
    expect(await whoami(fresh)).toMatchObject({ email, display_name: "Emmy Noether" });
    await expectSignedOut(laptop);
    await expectSignedOut(phone);
    await signIn(email, "symmetry and conservation", 401);
    // The operator's link was handed over, not mailed, so the notice is the first mail.
    const [notice] = await mailTo(email);
    expect(notice!.subject).toBe("Your Myelin password was changed");
    expect(notice!.text).toContain("All other sessions were signed out.");

    // Signing out ends only the session that asks, and an ended session cannot ask again.
    const tablet = expectSession(await signIn(email, "invariant theory"));
    const signedOut = await asPerson(tablet, "/v1/auth/logout", {});
    expect(signedOut.body).toEqual({ signed_out: "session" });
    expectNoStore(signedOut);
    await expectSignedOut(tablet);
    await asPerson(tablet, "/v1/auth/logout", {}, 401);
    await whoami(fresh);

    // Signing out everywhere ends every session, the asking one included.
    const desktop = expectSession(await signIn(email, "invariant theory"));
    const everywhere = await asPerson(desktop, "/v1/auth/logout/everywhere", {});
    expect(everywhere.body).toEqual({ signed_out: "everywhere" });
    await expectSignedOut(desktop);
    await expectSignedOut(fresh);
    expect(await whoami(expectSession(await signIn(email, "invariant theory")))).toMatchObject({
      email,
    });

    // An operator credential is not a person's session and cannot sign anyone out.
    await systemClient.json("/v1/auth/logout/everywhere", {
      method: "POST",
      body: {},
      expectedStatus: 403,
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
