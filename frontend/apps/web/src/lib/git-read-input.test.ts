import { describe, expect, it } from "vitest";

import {
  parseGitBrowseInput,
  parseGitCommitInput,
  parseGitCommitsInput,
  parseGitMyPrsInput,
  parseGitPrCursorInput,
  parseGitPrDiffInput,
  parseGitPrInput,
  parseGitRepoInput,
  parseGitRepoPrsInput,
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
    expect(parseGitPrInput({ repo: "core", n: 42 })).toEqual({ repo: "core", n: 42 });
    expect(parseGitRepoPrsInput({ repo: "core", state: "merged", sort: "created", cursor: "50" }))
      .toEqual({ repo: "core", state: "merged", sort: "created", cursor: "50" });
    expect(parseGitMyPrsInput({ bucket: "needs-review" })).toEqual({ bucket: "needs-review" });
    expect(parseGitPrCursorInput({ repo: "core", n: 42, cursor: "50" }))
      .toEqual({ repo: "core", n: 42, cursor: "50" });
    expect(parseGitPrDiffInput({ repo: "core", n: 42, view: "split" }))
      .toEqual({ repo: "core", n: 42, view: "split" });
  });

  it.each([
    () => parseGitRepoInput("../core"),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "../secret" }, false),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "", surprise: true }, true),
    () => parseGitBrowseInput({ repo: "core", ref: "main\nnext", path: "x" }, false),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "" }, false),
    () => parseGitCommitsInput({ repo: "core", ref: "main", cursor: "x\nsmuggled" }),
    () => parseGitCommitInput({ repo: "core", oid: OID.toUpperCase() }),
    () => parseGitPrInput({ repo: "core", n: 0 }),
    () => parseGitPrInput({ repo: "core", n: 1, surprise: true }),
    () => parseGitRepoPrsInput({ repo: "core", state: "draft" }),
    () => parseGitRepoPrsInput({ repo: "core", sort: "oldest" }),
    () => parseGitMyPrsInput({ bucket: "all" }),
    () => parseGitPrCursorInput({ repo: "core", n: 1, cursor: "x\nsmuggled" }),
    () => parseGitPrDiffInput({ repo: "core", n: 1, view: "side-by-side" }),
  ])("rejects malformed, unsafe, or surplus input", (parse) => {
    expect(parse()).toBeNull();
  });
});
