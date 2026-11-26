import { describe, expect, it } from "vitest";
import { parseRoster, validMemberId } from "./member-roster";

const person = {
  id: "person-01m8a3502429xhd70vcz12yvn0",
  display_name: "Ada Lovelace",
  email: "ada@example.com",
  role: "owner",
  joined_at: 1_795_635_183,
};
const pending = { id: "7c9e6679-7425-40de-944b-e07fc1f90ae7", email: "grace@example.com", role: "admin", expires_at: 1_796_000_000 };

describe("parseRoster", () => {
  it("reads people, invitations and the owner count", () => {
    expect(parseRoster({ role: "owner", owners: 1, people: [person], invitations: [pending] })).toEqual({
      role: "owner",
      owners: 1,
      people: [{ id: person.id, displayName: "Ada Lovelace", email: "ada@example.com", role: "owner", joinedAt: person.joined_at }],
      invitations: [{ id: pending.id, email: "grace@example.com", role: "admin", expiresAt: pending.expires_at }],
    });
  });

  it("leaves invitations out for a member and keeps an unrecorded address as null", () => {
    expect(parseRoster({ role: "member", owners: 0, people: [person] })).not.toHaveProperty("invitations");
    expect(parseRoster({ role: "admin", owners: 1, people: [], invitations: [{ ...pending, email: null }] })?.invitations?.[0]?.email)
      .toBeNull();
  });

  it.each([
    ["a bad viewer role", { role: "root", owners: 1, people: [] }],
    ["a missing owner count", { role: "owner", people: [] }],
    ["a person with a bad role", { role: "owner", owners: 1, people: [{ ...person, role: "superuser" }] }],
    ["a float joining time", { role: "owner", owners: 1, people: [{ ...person, joined_at: 1.5 }] }],
    ["a person missing their email", { role: "owner", owners: 1, people: [{ ...person, email: undefined }] }],
    ["an invitation missing its email key", { role: "owner", owners: 1, people: [], invitations: [{ id: pending.id, role: "member", expires_at: 1 }] }],
    ["a float expiry", { role: "owner", owners: 1, people: [], invitations: [{ ...pending, expires_at: 2.5 }] }],
    ["an id of ..", { role: "owner", owners: 1, people: [{ ...person, id: ".." }] }],
    ["invitations that are not a list", { role: "owner", owners: 1, people: [], invitations: {} }],
  ])("refuses %s", (_, json) => {
    expect(parseRoster(json)).toBeNull();
  });
});

describe("validMemberId", () => {
  it("takes the edge's ids and refuses path tricks", () => {
    expect(validMemberId("person-01m8a3502429xhd70vcz12yvn0")).toBe(true);
    expect(validMemberId(pending.id)).toBe(true);
    for (const id of [".", "..", "", "a/b", "a%2Fb", "x".repeat(201), 7]) expect(validMemberId(id)).toBe(false);
  });
});
