import { describe, expect, it } from "vitest";

import {
  isEditableBranch,
  parseGitFileEditDraft,
  parseGitFileEditReceipt,
} from "./git-file-edit-contract";

const OID = "a".repeat(40);
const draft = {
  repo: "platform/api",
  ref: "main",
  path: ".myelin/ci.toml",
  baseOid: "",
  contents: "on = \"push\"\n",
  message: "Start CI",
  clientNonce: "one-file-edit",
};

describe("browser file-edit boundary", () => {
  it("accepts one bounded branch edit and its exact durable receipt", () => {
    expect(parseGitFileEditDraft(draft)).toEqual(draft);
    expect(isEditableBranch("refs/heads/feature/retry")).toBe(true);
    expect(parseGitFileEditReceipt({
      applied: { outcome: "committed", new_oid: OID },
      durable: true,
    })).toEqual({ newOid: OID });
  });

  it("refuses ambiguous coordinates, snapshots, surplus fields, and unbounded text", () => {
    expect(parseGitFileEditDraft({ ...draft, ref: OID })).toBeNull();
    expect(parseGitFileEditDraft({ ...draft, path: "../secrets" })).toBeNull();
    expect(parseGitFileEditDraft({ ...draft, message: " Start CI" })).toBeNull();
    expect(parseGitFileEditDraft({ ...draft, contents: "x".repeat(512 * 1024 + 1) })).toBeNull();
    expect(parseGitFileEditDraft({ ...draft, force: true })).toBeNull();
    expect(parseGitFileEditReceipt({
      applied: { outcome: "committed", new_oid: OID, hidden: true },
      durable: true,
    })).toBeNull();
  });
});
