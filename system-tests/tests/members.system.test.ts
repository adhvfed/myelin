import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import { systemClient } from "../src/context.js";
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

async function join(code: string, name: string, email: string): Promise<Person> {
  const joined = await systemClient.json(`/v1/auth/invitations/${code}/accept`, {
    method: "POST",
    ...anonymous,
    body: { display_name: name, password },
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

async function setRole(by: Person, target: Person, role: string, expectedStatus = 200) {
  return systemClient.json(`/v1/members/${target.id}/role`, {
    method: "POST",
    ...as(by),
    body: { role },
    expectedStatus,
  });
}

async function remove(by: Person, target: Person, expectedStatus = 200) {
  return systemClient.json(`/v1/members/${target.id}`, {
    method: "DELETE",
    ...as(by),
    expectedStatus,
  });
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

function refusedFor(response: { body: JsonRecord }, code: string): void {
  expect(record(response.body.error, "error").code).toBe(code);
}

/** Joins the person an emailed invitation was sent to. */
async function joinFromMail(name: string, email: string): Promise<Person> {
  const [mail] = await mailTo(email);
  return join(codeOf(linkIn(mail!)), name, email);
}

describe("an organization's people", () => {
  test("are invited, promoted, demoted and removed by role, and always keep an owner", async () => {
    // The operator's invitation makes the organization's owner.
    const oliviaEmail = uniqueEmail("olivia");
    const olivia = await join(codeOf(await operatorLink("invite", oliviaEmail, tenant)), "Olivia Owner", oliviaEmail);
    expect(await whoami(olivia.token)).toMatchObject({ role: "owner", tenant });

    // The owner invites an admin and a member; each invitation says what it grants.
    const adamEmail = uniqueEmail("adam");
    expect((await invite(olivia, adamEmail, "admin")).body).toMatchObject({ email: adamEmail, role: "admin" });
    const miaEmail = uniqueEmail("mia");
    expect((await invite(olivia, miaEmail)).body).toMatchObject({ email: miaEmail, role: "member" });
    refusedFor(await invite(olivia, uniqueEmail("nobody"), "superuser", 400), "bad_request");
    const pending = record((await members(olivia)), "roster");
    expect(array(pending.invitations, "invitations").map((entry) => record(entry, "invitation"))).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ email: adamEmail, role: "admin", expires_at: expect.any(Number) }),
        expect.objectContaining({ email: miaEmail, role: "member" }),
      ]),
    );
    const adam = await joinFromMail("Adam Admin", adamEmail);
    const mia = await joinFromMail("Mia Member", miaEmail);
    expect(await whoami(adam.token)).toMatchObject({ role: "admin" });
    expect(await whoami(mia.token)).toMatchObject({ role: "member" });

    const roster = await members(olivia);
    expect(roster.role).toBe("owner");
    expect(rolesIn(roster)).toEqual({ [oliviaEmail]: "owner", [adamEmail]: "admin", [miaEmail]: "member" });
    expect(roster.invitations).toEqual([]);
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

    // The admin invites members only, and cannot touch the owner.
    const maxEmail = uniqueEmail("max");
    expect((await invite(adam, maxEmail)).body).toMatchObject({ role: "member" });
    refusedFor(await invite(adam, uniqueEmail("second-admin"), "admin", 403), "role_required");
    refusedFor(await invite(adam, uniqueEmail("second-owner"), "owner", 403), "role_required");
    refusedFor(await setRole(adam, mia, "admin", 403), "role_required");
    refusedFor(await setRole(adam, olivia, "member", 403), "role_required");
    refusedFor(await remove(adam, olivia, 403), "role_required");
    const max = await joinFromMail("Max Member", maxEmail);

    // The owner promotes and demotes; each change holds from the person's very next request.
    await setRole(olivia, mia, "admin");
    expect(await whoami(mia.token)).toMatchObject({ role: "admin" });
    const byMia = uniqueEmail("by-mia");
    await invite(mia, byMia);
    await setRole(olivia, mia, "member");
    expect(await whoami(mia.token)).toMatchObject({ role: "member" });
    refusedFor(await invite(mia, uniqueEmail("too-late"), undefined, 403), "role_required");
    await systemClient.json("/v1/members/person-unknown/role", {
      method: "POST",
      ...as(olivia),
      body: { role: "member" },
      expectedStatus: 404,
    });

    // An admin removes a member, whose session ends at once and who cannot sign in again.
    await remove(adam, max);
    await whoami(max.token, 401);
    await systemClient.json("/v1/auth/password/login", {
      method: "POST",
      ...anonymous,
      body: { email: maxEmail, password },
      expectedStatus: 401,
    });
    expect(rolesIn(await members(olivia))).not.toHaveProperty(maxEmail);

    // Revoking an invitation kills the link it mailed.
    const ritaEmail = uniqueEmail("rita");
    await invite(olivia, ritaEmail);
    const [ritaMail] = await mailTo(ritaEmail);
    const ritaCode = codeOf(linkIn(ritaMail!));
    const ritaInvitation = array((await members(olivia)).invitations, "invitations")
      .map((entry) => record(entry, "invitation"))
      .find((invitation) => invitation.email === ritaEmail);
    const revoked = await systemClient.json(`/v1/invitations/${string(ritaInvitation?.id, "invitation id")}`, {
      method: "DELETE",
      ...as(adam),
    });
    expect(revoked.body).toMatchObject({ status: "revoked" });
    await systemClient.json(`/v1/auth/invitations/${ritaCode}`, { ...anonymous, expectedStatus: 404 });
    await systemClient.json(`/v1/auth/invitations/${ritaCode}/accept`, {
      method: "POST",
      ...anonymous,
      body: { display_name: "Rita", password },
      expectedStatus: 404,
    });
    await systemClient.json(`/v1/invitations/${string(ritaInvitation?.id, "invitation id")}`, {
      method: "DELETE",
      ...as(olivia),
      expectedStatus: 404,
    });

    // Revoked before the mail worker's next pass (every two seconds), it takes the unsent mail
    // with it. The race with that pass is the test's, not the product's, so a revocation the
    // worker beat is tried again with a new address; if revoking left the mail queued, every
    // attempt would deliver it.
    let withdrawn = false;
    for (let attempt = 0; attempt < 5 && !withdrawn; attempt += 1) {
      const email = uniqueEmail("withdrawn");
      await invite(olivia, email);
      const id = array((await members(olivia)).invitations, "invitations")
        .map((entry) => record(entry, "invitation"))
        .find((invitation) => invitation.email === email)?.id;
      await systemClient.json(`/v1/invitations/${string(id, "invitation id")}`, { method: "DELETE", ...as(olivia) });
      await new Promise((resolve) => setTimeout(resolve, 6_000));
      withdrawn = (await mailSoFar(email)).length === 0;
    }
    expect(withdrawn).toBe(true);

    // The last owner can be neither demoted nor removed, by anyone, themself included.
    refusedFor(await setRole(olivia, olivia, "admin", 409), "last_owner");
    refusedFor(await remove(olivia, olivia, 409), "last_owner");
    expect(await whoami(olivia.token)).toMatchObject({ role: "owner" });

    // The operator can make another owner; then the first may step down, and the new one is last.
    await operatorRole(tenant, adamEmail, "owner");
    expect(await whoami(adam.token)).toMatchObject({ role: "owner" });
    await setRole(olivia, olivia, "member");
    refusedFor(await remove(adam, adam, 409), "last_owner");
    expect(rolesIn(await members(adam))).toEqual({
      [oliviaEmail]: "member",
      [adamEmail]: "owner",
      [miaEmail]: "member",
    });
  });
});
