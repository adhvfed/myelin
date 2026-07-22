import { describe, expect, it } from "vitest";

import {
  parseGitBrowseInput,
  parseGitCommitInput,
  parseGitCommitsInput,
  parseGitMyPrsInput,
  parseGitPrCursorInput,
  parseGitPrDiffInput,
  parseGitPrInput,
  parseGitRepoListInput,
  parseGitRepoInput,
  parseGitRepoPrsInput,
  parseGitRefsInput,
  parseGitTreeInput,
  gitTreeSearchParams,
  gitRefsSearchParams,
  gitRepoListSearchParams,
} from "./git-read-input";

const OID = "0123456789abcdef0123456789abcdef01234567";

describe("Git read RPC inputs", () => {
  it("admits canonical repository, browse, history, and commit coordinates", () => {
    expect(parseGitRepoInput("team/core")).toEqual({ repo: "team/core" });
    expect(parseGitBrowseInput({ repo: "core", ref: "feature/x", path: "src/x" }, false))
      .toEqual({ repo: "core", ref: "feature/x", path: "src/x" });
    expect(parseGitTreeInput({
      repo: "core", ref: "refs/heads/main", path: "", limit: 100,
      cursor: "gt1_a-b_c", q: "readme & docs",
    })).toEqual({
      repo: "core", ref: "refs/heads/main", path: "", limit: 100,
      cursor: "gt1_a-b_c", q: "readme & docs",
    });
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
    expect(parseGitRefsInput({
      repo: "core", limit: 25, cursor: "gr1_a-b_c", q: "feature & fixes",
      current: "refs/heads/feature/x",
    })).toEqual({
      repo: "core", limit: 25, cursor: "gr1_a-b_c", q: "feature & fixes",
      current: "refs/heads/feature/x",
    });
  });

  it("encodes refs query values only through URLSearchParams", () => {
    expect(gitRefsSearchParams({
      repo: "core", limit: 25, cursor: "gr1_a-b_c", q: "feature & fixes",
      current: "refs/heads/feature/x",
    }).toString()).toBe(
      "limit=25&cursor=gr1_a-b_c&q=feature+%26+fixes&current=refs%2Fheads%2Ffeature%2Fx",
    );
  });

  it("accepts only the exact bounded repository-list input and always selects the summary view", () => {
    const cursor = "rl1_YWNtZS9teWVsaW4";
    expect(parseGitRepoListInput({})).toEqual({});
    expect(parseGitRepoListInput({ limit: 1, cursor })).toEqual({ limit: 1, cursor });
    expect(gitRepoListSearchParams({ limit: 1, cursor }).toString())
      .toBe(`view=summary&limit=1&cursor=${cursor}`);

    for (const value of [
      "not-an-object",
      { limit: 0 },
      { limit: 101 },
      { limit: 1.5 },
      { cursor: "opaque" },
      { cursor: "rl1_YR" }, // decodes like `YQ`, but has non-canonical trailing bits
      { cursor: `rl1_${"a".repeat(512)}` },
      { surprise: true },
    ]) expect(parseGitRepoListInput(value), JSON.stringify(value)).toBeNull();
  });

  it("encodes tree pagination and search only through URLSearchParams", () => {
    expect(gitTreeSearchParams({
      repo: "core", ref: "refs/heads/main", path: "", limit: 100,
      cursor: "gt1_a-b_c", q: "readme & docs",
    }).toString()).toBe("limit=100&cursor=gt1_a-b_c&q=readme+%26+docs");
  });

  it.each([
    "refs/heads/feature@two",
    "refs/heads/topic/a.b",
    "refs/tags/release.locked",
  ])("accepts a Git-valid full current ref at the boundary: %s", (current) => {
    expect(parseGitRefsInput({ repo: "core", current })).toEqual({ repo: "core", current });
  });

  it.each([
    () => parseGitRepoInput("../core"),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "../secret" }, false),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "", surprise: true }, true),
    () => parseGitBrowseInput({ repo: "core", ref: "main\nnext", path: "x" }, false),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "" }, false),
    () => parseGitBrowseInput({ repo: "core", ref: "main", path: "", limit: 1 }, true),
    () => parseGitTreeInput({ repo: "core", ref: "main", path: "", limit: 0 }),
    () => parseGitTreeInput({ repo: "core", ref: "main", path: "", limit: 101 }),
    () => parseGitTreeInput({ repo: "core", ref: "main", path: "", cursor: "opaque" }),
    () => parseGitTreeInput({ repo: "core", ref: "main", path: "", cursor: "gt1_bad\n" }),
    () => parseGitTreeInput({ repo: "core", ref: "main", path: "", q: "x".repeat(257) }),
    () => parseGitTreeInput({ repo: "core", ref: "main", path: "", surprise: true }),
    () => parseGitCommitsInput({ repo: "core", ref: "main", cursor: "x\nsmuggled" }),
    () => parseGitCommitInput({ repo: "core", oid: OID.toUpperCase() }),
    () => parseGitPrInput({ repo: "core", n: 0 }),
    () => parseGitPrInput({ repo: "core", n: 1, surprise: true }),
    () => parseGitRepoPrsInput({ repo: "core", state: "draft" }),
    () => parseGitRepoPrsInput({ repo: "core", sort: "oldest" }),
    () => parseGitMyPrsInput({ bucket: "all" }),
    () => parseGitPrCursorInput({ repo: "core", n: 1, cursor: "x\nsmuggled" }),
    () => parseGitPrDiffInput({ repo: "core", n: 1, view: "side-by-side" }),
    () => parseGitRefsInput({ repo: "core", limit: 0 }),
    () => parseGitRefsInput({ repo: "core", limit: 101 }),
    () => parseGitRefsInput({ repo: "core", cursor: "x\nsmuggled" }),
    () => parseGitRefsInput({ repo: "core", cursor: "opaque" }),
    () => parseGitRefsInput({ repo: "core", q: "x".repeat(257) }),
    () => parseGitRefsInput({ repo: "core", current: "main" }),
    () => parseGitRefsInput({ repo: "core", current: "refs/remotes/origin/main" }),
    () => parseGitRefsInput({ repo: "core", current: "refs/heads/../secret" }),
    () => parseGitRefsInput({ repo: "core", current: "refs/heads/@" }),
    () => parseGitRefsInput({ repo: "core", current: "refs/heads/topic." }),
    () => parseGitRefsInput({ repo: "core", current: "refs/heads/.hidden" }),
    () => parseGitRefsInput({ repo: "core", current: "refs/heads/topic/.hidden" }),
    () => parseGitRefsInput({ repo: "core", current: "refs/heads/topic.lock" }),
    () => parseGitRefsInput({ repo: "core", current: "refs/heads/topic/final.lock" }),
    () => parseGitRefsInput({ repo: "core", surprise: true }),
  ])("rejects malformed, unsafe, or surplus input", (parse) => {
    expect(parse()).toBeNull();
  });
});
