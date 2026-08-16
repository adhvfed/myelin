import { describe, expect, it } from "vitest";

import {
  parseGitBrowseInput,
  parseGitCommitInput,
  parseGitCommitsInput,
  parseGitMyPrsInput,
  parseGitPrCommitsInput,
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
  gitPrCommitsSearchParams,
  gitPrCommitsPath,
} from "./git-read-input";

const OID = "0123456789abcdef0123456789abcdef01234567";

function prCommitCursor(position = 1): string {
  const frame = new Uint8Array(78);
  frame[0] = 1;
  frame.set(Uint8Array.from({ length: 32 }, (_, index) => index), 1);
  frame[33] = 0;
  frame.set(Uint8Array.from(OID.match(/../g)!, (byte) => Number.parseInt(byte, 16)), 54);
  new DataView(frame.buffer).setUint32(74, position, false);
  return `pc1_${Buffer.from(frame).toString("base64url")}`;
}

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
    const cursor = prCommitCursor(20);
    expect(parseGitPrCommitsInput({ repo: "core", n: 42, limit: 20, cursor }))
      .toEqual({ repo: "core", n: 42, limit: 20, cursor });
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

  it("accepts only the exact bounded repository-list input", () => {
    const cursor = "rl2_AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGgAGMDFKMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDJteWVsaW4";
    expect(parseGitRepoListInput({})).toEqual({});
    expect(parseGitRepoListInput({ limit: 1, cursor })).toEqual({ limit: 1, cursor });
    expect(gitRepoListSearchParams({ limit: 1, cursor }).toString())
      .toBe(`limit=1&cursor=${cursor}`);

    for (const value of [
      "not-an-object",
      { limit: 0 },
      { limit: 101 },
      { limit: 1.5 },
      { cursor: "opaque" },
      { cursor: "rl2_YR" }, // decodes like `YQ`, but has non-canonical trailing bits
      { cursor: `rl2_${"a".repeat(512)}` },
      { surprise: true },
    ]) expect(parseGitRepoListInput(value), JSON.stringify(value)).toBeNull();
  });

  it("encodes tree pagination and search only through URLSearchParams", () => {
    expect(gitTreeSearchParams({
      repo: "core", ref: "refs/heads/main", path: "", limit: 100,
      cursor: "gt1_a-b_c", q: "readme & docs",
    }).toString()).toBe("limit=100&cursor=gt1_a-b_c&q=readme+%26+docs");
  });

  it("accepts and forwards only exact canonical PR commit pagination", () => {
    const cursor = prCommitCursor(20);
    expect(gitPrCommitsSearchParams({ repo: "core", n: 7, limit: 20, cursor }).toString())
      .toBe(`limit=20&cursor=${cursor}`);
    expect(gitPrCommitsPath({ repo: "team/core", n: 7, limit: 20, cursor }))
      .toBe(`/v1/git/repos/team%2Fcore/prs/7/commits?limit=20&cursor=${cursor}`);
    expect(parseGitPrCommitsInput({ repo: "core", n: 7, limit: 1 })).toEqual({
      repo: "core", n: 7, limit: 1,
    });

    const wrongVersion = prCommitCursor();
    const wrongVersionFrame = Buffer.from(wrongVersion.slice(4), "base64url");
    wrongVersionFrame[0] = 2;
    const positionZeroFrame = Buffer.from(prCommitCursor().slice(4), "base64url");
    positionZeroFrame.fill(0, 74, 78);
    const tooDeepFrame = Buffer.from(prCommitCursor().slice(4), "base64url");
    tooDeepFrame.writeUInt32BE(100_001, 74);
    for (const value of [
      { repo: "core", n: 7, limit: 0 },
      { repo: "core", n: 7, limit: 101 },
      { repo: "core", n: 7, limit: 20.5 },
      { repo: "core", n: 7, cursor: "50" },
      { repo: "core", n: 7, cursor: `${prCommitCursor()}=` },
      { repo: "core", n: 7, cursor: `pc1_${wrongVersionFrame.toString("base64url")}` },
      { repo: "core", n: 7, cursor: `pc1_${positionZeroFrame.toString("base64url")}` },
      { repo: "core", n: 7, cursor: `pc1_${tooDeepFrame.toString("base64url")}` },
      { repo: "core", n: 7, cursor: `pc1_${"a".repeat(253)}` },
      { repo: "core", n: 7, surprise: true },
    ]) expect(parseGitPrCommitsInput(value), JSON.stringify(value)).toBeNull();
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
    () => parseGitRepoInput("platform.git/core"),
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
    () => parseGitPrCommitsInput({ repo: "core", n: 1, cursor: "x\nsmuggled" }),
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
