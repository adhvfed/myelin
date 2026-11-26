import { describe, expect, it } from "vitest";
import { grantableRoles, mayActOn, roleChoices } from "./member-rules";

describe("member rules", () => {
  it("lets only owners hand out admin and owner", () => {
    expect(grantableRoles("owner")).toEqual(["owner", "admin", "member"]);
    expect(grantableRoles("admin")).toEqual(["member"]);
    expect(grantableRoles("member")).toEqual([]);
  });

  it("lets admins act on members and themselves only", () => {
    expect(mayActOn("owner", "owner", false)).toBe(true);
    expect(mayActOn("admin", "member", false)).toBe(true);
    expect(mayActOn("admin", "admin", false)).toBe(false);
    expect(mayActOn("admin", "owner", false)).toBe(false);
    expect(mayActOn("admin", "admin", true)).toBe(true);
    expect(mayActOn("member", "member", true)).toBe(false);
  });

  it("offers a person's own role plus what the viewer may change it to", () => {
    expect(roleChoices("owner", "member", false)).toEqual(["owner", "admin", "member"]);
    expect(roleChoices("admin", "member", false)).toEqual(["member"]);
    expect(roleChoices("admin", "admin", true)).toEqual(["admin", "member"]);
    expect(roleChoices("admin", "owner", false)).toEqual(["owner"]);
  });
});
