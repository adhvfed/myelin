import { describe, expect, it } from "vitest";

import {
  parseGitBrowseInput,
  parseGitCommitInput,
  parseGitCommitsInput,
  parseGitRepoInput,
} from "./git-read-input";

const OID = "0123456789abcdef0123456789abcdef01234567";

describe("Git read RPC inputs", () => {
  it("admits canonical repository, browse, history, and commit coordinates", () => {
    expect(parseGitRepoInput("team/core")).toEqual({ repo: "team/core" });
    expect(parseGitBrowseInput({ repo: "core", ref: "feature/x", path: "src/x" }, false))
      .toEqual({ repo: "core", ref: "feature/x", path: "src/x" });
    expect(parseGitCommitsInput({ repo: "core", ref: "main", cursor: "opaque" }))
      .toEqual({ repo: "core", ref: "main", cursor: "opaque" });
    expect(parseGitCommitInput({ repo: "core", oid: OID })).toEqual({ repo: "core", oid: OID });
  });

  it.each([
    () => parseGitRepoInput("../core"),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "../secret" }, false),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "", surprise: true }, true),
    () => parseGitBrowseInput({ repo: "core", ref: "main\nnext", path: "x" }, false),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "" }, false),
    () => parseGitCommitsInput({ repo: "core", ref: "main", cursor: "x\nsmuggled" }),
    () => parseGitCommitInput({ repo: "core", oid: OID.toUpperCase() }),
  ])("rejects malformed, unsafe, or surplus input", (parse) => {
    expect(parse()).toBeNull();
  });
});
