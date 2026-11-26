import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { browserApprovedCliClient, systemClient, uniqueName } from "../src/context.js";
import { array, integer, record, string, type JsonRecord } from "../src/json.js";
import { linkIn, mailSoFar, mailTo } from "../src/mailpit.js";
import { operatorLink, operatorRole } from "../src/operator.js";

// One organization of its own, so its owners are exactly the ones this journey makes.
const tenant = `members-${randomUUID().slice(0, 8)}`;
const anonymous = { authenticated: false } as const;
const password = "a members journey passphrase";

function uniqueEmail(name: string): string {
  return `${name}-${randomUUID().slice(0, 8)}@example.com`;
}

function codeOf(link: string): string {
  return string(new URL(link).pathname.split("/join/")[1], "invitation code");
}

interface Person {
  email: string;
  token: string;
  id: string;
}

async function join(code: string, name: string, email: string, secret = password): Promise<Person> {
  const joined = await systemClient.json(`/v1/auth/invitations/${code}/accept`, {
    method: "POST",
    ...anonymous,
    body: { display_name: name, password: secret },
  });
  const token = string(joined.body.access_token, "session token");
  const who = await whoami(token);
  return { email, token, id: string(who.principal_id, "principal id") };
}

async function whoami(token: string, expectedStatus = 200): Promise<JsonRecord> {
  return (await systemClient.json("/v1/whoami", { token, tokenScheme: "session", expectedStatus })).body;
}

function as(person: Person) {
  return { token: person.token, tokenScheme: "session" } as const;
}

async function invite(by: Person, email: string, role?: string, expectedStatus = 201) {
  return systemClient.json("/v1/invitations", {
    method: "POST",
    ...as(by),
    body: role ? { email, role } : { email },
    expectedStatus,
  });
}

async function setRole(by: Person, target: { id: string }, role: string, expectedStatus = 200) {
  return systemClient.json(`/v1/members/${target.id}/role`, {
    method: "POST",
    ...as(by),
    body: { role },
    expectedStatus,
  });
}

async function remove(by: Person, target: { id: string }, expectedStatus = 200) {
  return systemClient.json(`/v1/members/${target.id}`, {
    method: "DELETE",
    ...as(by),
    expectedStatus,
  });
}

async function revoke(by: Person, id: string, expectedStatus = 200) {
  return systemClient.json(`/v1/invitations/${id}`, { method: "DELETE", ...as(by), expectedStatus });
}

async function members(by: Person): Promise<JsonRecord> {
  return (await systemClient.json("/v1/members", as(by))).body;
}

function rolesIn(roster: JsonRecord): Record<string, string> {
  return Object.fromEntries(
    array(roster.people, "people").map((entry) => {
      const person = record(entry, "person");
      return [string(person.email, "email"), string(person.role, "role")];
    }),
  );
}

async function pendingId(by: Person, email: string): Promise<string | undefined> {
  const pending = array((await members(by)).invitations, "invitations").map((entry) => record(entry, "invitation"));
  const found = pending.find((invitation) => invitation.email === email);
  return found ? string(found.id, "invitation id") : undefined;
}

function refusedFor(response: { body: JsonRecord }, code: string): void {
  expect(record(response.body.error, "error").code).toBe(code);
}

async function signIn(email: string, secret: string, expectedStatus = 200) {
  return systemClient.json("/v1/auth/password/login", {
    method: "POST",
    ...anonymous,
    body: { email, password: secret },
    expectedStatus,
  });
}

/** The newest invitation link mailed to `email`, once `count` messages have arrived. */
async function mailedCode(email: string, count = 1): Promise<string> {
  const mail = await mailTo(email, count);
  return codeOf(linkIn(mail[count - 1]!));
}

describe("an organization's people", () => {
  test("are invited, promoted, demoted and removed by role, and always keep an owner", async () => {
    // The operator's first invitation makes the owner; once someone is here, a member.
    const oliviaEmail = uniqueEmail("olivia");
    const olivia = await join(codeOf(await operatorLink("invite", oliviaEmail, tenant)), "Olivia Owner", oliviaEmail);
    expect(await whoami(olivia.token)).toMatchObject({ role: "owner", tenant });
    const opalEmail = uniqueEmail("opal");
    const opal = await join(codeOf(await operatorLink("invite", opalEmail, tenant)), "Opal Operator-Invited", opalEmail);
    expect(await whoami(opal.token)).toMatchObject({ role: "member" });

    // The owner invites an admin and a member; each invitation says what it grants.
    const adamEmail = uniqueEmail("adam");
    expect((await invite(olivia, adamEmail, "admin")).body).toMatchObject({ email: adamEmail, role: "admin" });
    const miaEmail = uniqueEmail("mia");
    expect((await invite(olivia, miaEmail)).body).toMatchObject({ email: miaEmail, role: "member" });
    refusedFor(await invite(olivia, uniqueEmail("nobody"), "superuser", 400), "bad_request");
    const pending = array((await members(olivia)).invitations, "invitations").map((entry) => record(entry, "invitation"));
    expect(pending).toEqual(expect.arrayContaining([
      expect.objectContaining({ email: adamEmail, role: "admin", expires_at: expect.any(Number) }),
      expect.objectContaining({ email: miaEmail, role: "member" }),
    ]));
    const adam = await join(await mailedCode(adamEmail), "Adam Admin", adamEmail);
    const mia = await join(await mailedCode(miaEmail), "Mia Member", miaEmail);
    expect(await whoami(adam.token)).toMatchObject({ role: "admin" });
    expect(await whoami(mia.token)).toMatchObject({ role: "member" });

    const roster = await members(olivia);
    expect(roster).toMatchObject({ role: "owner", owners: 1, invitations: [] });
    expect(rolesIn(roster)).toEqual({
      [oliviaEmail]: "owner", [opalEmail]: "member", [adamEmail]: "admin", [miaEmail]: "member",
    });
    const listedMia = array(roster.people, "people").map((entry) => record(entry, "person")).find((person) => person.id === mia.id);
    expect(listedMia).toMatchObject({ display_name: "Mia Member", email: miaEmail, role: "member" });
    integer(listedMia!.joined_at, "joined at");

    // A member sees the people but not the invitations, and can change nothing.
    const asMember = await members(mia);
    expect(asMember.role).toBe("member");
    expect(asMember).not.toHaveProperty("invitations");
    refusedFor(await invite(mia, uniqueEmail("friend"), undefined, 403), "role_required");
    refusedFor(await setRole(mia, mia, "admin", 403), "role_required");
    refusedFor(await remove(mia, adam, 403), "role_required");
    const patEmail = uniqueEmail("pat");
    await invite(olivia, patEmail, "admin");
    const patInvitation = string(await pendingId(olivia, patEmail), "Pat's invitation");
    refusedFor(await revoke(mia, patInvitation, 403), "role_required");

    // The admin invites members only, cannot replace an invitation only an owner could send, and
    // cannot touch the owner or raise themself.
    const maxEmail = uniqueEmail("max");
    expect((await invite(adam, maxEmail)).body).toMatchObject({ role: "member" });
    refusedFor(await invite(adam, uniqueEmail("second-admin"), "admin", 403), "role_required");
    refusedFor(await invite(adam, uniqueEmail("second-owner"), "owner", 403), "role_required");
    refusedFor(await invite(adam, patEmail, undefined, 409), "invitation_exists");
    expect(await pendingId(olivia, patEmail)).toBe(patInvitation);
    refusedFor(await setRole(adam, mia, "admin", 403), "role_required");
    refusedFor(await setRole(adam, adam, "owner", 403), "role_required");
    refusedFor(await setRole(adam, olivia, "member", 403), "role_required");
    refusedFor(await remove(adam, olivia, 403), "role_required");
    const max = await join(await mailedCode(maxEmail), "Max Member", maxEmail);

    // The owner promotes and demotes; each change holds from the person's very next request.
    // Admins cannot act on one another, but may step down themselves.
    await setRole(olivia, mia, "admin");
    expect(await whoami(mia.token)).toMatchObject({ role: "admin" });
    refusedFor(await setRole(adam, mia, "member", 403), "role_required");
    refusedFor(await remove(adam, mia, 403), "role_required");
    await invite(mia, uniqueEmail("by-mia"));
    await setRole(mia, mia, "member");
    expect(await whoami(mia.token)).toMatchObject({ role: "member" });
    refusedFor(await invite(mia, uniqueEmail("too-late"), undefined, 403), "role_required");
    await setRole(olivia, opal, "admin");
    expect(await whoami(opal.token)).toMatchObject({ role: "admin" });
    await setRole(olivia, opal, "member");
    expect(await whoami(opal.token)).toMatchObject({ role: "member" });
    await systemClient.json("/v1/members/person-unknown/role", {
      method: "POST",
      ...as(olivia),
      body: { role: "member" },
      expectedStatus: 404,
    });

    // An admin who removes themself is signed out, and the invitations they sent go with them.
    const avaEmail = uniqueEmail("ava");
    await invite(olivia, avaEmail, "admin");
    const ava = await join(await mailedCode(avaEmail), "Ava Admin", avaEmail);
    const zoeEmail = uniqueEmail("zoe");
    await invite(ava, zoeEmail);
    const zoeCode = await mailedCode(zoeEmail);
    await remove(ava, ava);
    await whoami(ava.token, 401);
    expect(await pendingId(olivia, zoeEmail)).toBeUndefined();
    await systemClient.json(`/v1/auth/invitations/${zoeCode}`, { ...anonymous, expectedStatus: 404 });

    // An admin removes a member, whose session ends at once and who cannot sign in again.
    await remove(adam, max);
    await whoami(max.token, 401);
    await signIn(maxEmail, password, 401);
    expect(rolesIn(await members(olivia))).not.toHaveProperty(maxEmail);

    // Invited back, the same person returns with a new password; old sessions stay ended.
    await invite(olivia, maxEmail);
    const returned = await join(await mailedCode(maxEmail, 2), "Max Again", maxEmail, "a brand new passphrase");
    expect(returned.id).toBe(max.id);
    expect(await whoami(returned.token)).toMatchObject({ role: "member", display_name: "Max Member" });
    await whoami(max.token, 401);
    await signIn(maxEmail, password, 401);
    await signIn(maxEmail, "a brand new passphrase");

    // Revoking an invitation kills the link it mailed.
    const ritaEmail = uniqueEmail("rita");
    await invite(olivia, ritaEmail);
    const ritaCode = await mailedCode(ritaEmail);
    const ritaInvitation = string(await pendingId(olivia, ritaEmail), "Rita's invitation");
    expect((await revoke(adam, ritaInvitation)).body).toMatchObject({ status: "revoked" });
    await systemClient.json(`/v1/auth/invitations/${ritaCode}`, { ...anonymous, expectedStatus: 404 });
    await systemClient.json(`/v1/auth/invitations/${ritaCode}/accept`, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Rita", password },
      expectedStatus: 404,
    });
    await revoke(olivia, ritaInvitation, 404);

    // Revoked before the mail worker's next pass, an invitation takes its unsent mail with it. A
    // control invitation sent after the revocation must arrive, so a pass has run since; if the
    // revoked one arrived too, the worker may have sent it before the revocation, so the attempt
    // is repeated with new addresses. Were revoking to leave the mail queued, every attempt would
    // deliver it.
    let withdrawn = false;
    for (let attempt = 0; attempt < 5 && !withdrawn; attempt += 1) {
      const revokedEmail = uniqueEmail("withdrawn");
      const controlEmail = uniqueEmail("control");
      await invite(olivia, revokedEmail);
      await revoke(olivia, string(await pendingId(olivia, revokedEmail), "revoked invitation"));
      await invite(olivia, controlEmail);
      await mailTo(controlEmail);
      await new Promise((resolve) => setTimeout(resolve, 2_000));
      withdrawn = (await mailSoFar(revokedEmail)).length === 0;
    }
    expect(withdrawn).toBe(true);

    // A pending invitation holds only while its sender could send it again: Olivia's invitation
    // to make Otto an owner dies when she stops being an owner.
    const ottoEmail = uniqueEmail("otto");
    await invite(olivia, ottoEmail, "owner");
    const ottoCode = await mailedCode(ottoEmail);
    await systemClient.json(`/v1/auth/invitations/${ottoCode}`, anonymous);
    await operatorRole(tenant, { principal: adam.id }, "owner");
    expect(await whoami(adam.token)).toMatchObject({ role: "owner" });
    await setRole(adam, olivia, "admin");
    await systemClient.json(`/v1/auth/invitations/${ottoCode}`, { ...anonymous, expectedStatus: 404 });
    await systemClient.json(`/v1/auth/invitations/${ottoCode}/accept`, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Otto", password },
      expectedStatus: 404,
    });

    // The last owner can be neither demoted nor removed, by anyone, themself included.
    refusedFor(await setRole(adam, adam, "admin", 409), "last_owner");
    refusedFor(await remove(adam, adam, 409), "last_owner");
    expect(await members(adam)).toMatchObject({ owners: 1 });
    await operatorRole(tenant, { email: oliviaEmail }, "owner");
    await setRole(olivia, adam, "member");
    refusedFor(await remove(olivia, olivia, 409), "last_owner");
    expect(rolesIn(await members(olivia))).toEqual({
      [oliviaEmail]: "owner",
      [opalEmail]: "member",
      [adamEmail]: "member",
      [miaEmail]: "member",
      [maxEmail]: "member",
    });

    // Nobody here can reach a person in another organization.
    const outsiderTenant = `members-${randomUUID().slice(0, 8)}`;
    const outsiderEmail = uniqueEmail("xavier");
    const outsider = await join(
      codeOf(await operatorLink("invite", outsiderEmail, outsiderTenant)),
      "Xavier Elsewhere",
      outsiderEmail,
    );
    await setRole(olivia, outsider, "member", 404);
    await remove(olivia, outsider, 404);
    expect(await whoami(outsider.token)).toMatchObject({ role: "owner", tenant: outsiderTenant });
  }, 120_000);

  test("answer only a person's own session", async () => {
    const founder = await browserApprovedCliClient();
    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: { name: uniqueName("members-probe"), tools: ["git.list_repositories"] },
      idempotencyKey: `members-probe-${randomUUID()}`,
      expectedStatus: 201,
    });
    const agentId = string(record(activated.body.agent, "agent").id, "agent id");
    const started = await founder.json(`/v1/agents/${encodeURIComponent(agentId)}/runs`, {
      method: "POST",
      body: {},
      idempotencyKey: `members-probe-run-${randomUUID()}`,
      expectedStatus: 201,
    });
    const runToken = string(record(started.body.credential, "run credential").token, "run token");

    const routes = [
      ["GET", "/v1/members", undefined],
      ["POST", "/v1/invitations", { email: uniqueEmail("probe") }],
      ["DELETE", "/v1/invitations/some-id", undefined],
      ["POST", "/v1/members/some-id/role", { role: "owner" }],
      ["DELETE", "/v1/members/some-id", undefined],
    ] as const;
    for (const [method, path, body] of routes) {
      // The operator's capability, and an agent run's, are refused before any role is read.
      await systemClient.json(path, { method, ...(body ? { body } : {}), expectedStatus: 403 });
      await systemClient.json(path, {
        method,
        ...(body ? { body } : {}),
        token: runToken,
        tokenScheme: "agent",
        expectedStatus: 403,
      });
    }
  });
});
