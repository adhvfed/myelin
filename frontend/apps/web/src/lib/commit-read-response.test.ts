import { describe, expect, it } from "vitest";

import { parseCommitDiff, parseCommitsPage } from "./commit-read-response";

const OID = "0123456789abcdef0123456789abcdef01234567";

describe("commit read response projection", () => {
  it("projects commit pages and diffs recursively", () => {
    expect(parseCommitsPage({
      items: [{ oid: OID, short_oid: OID.slice(0, 12), summary: "ship", author: "u", committed_at: 1, parents: [], secret: "drop" }],
      page: { next_cursor: null, prev_cursor: "0", limit: 50, offset: 1, range: { from: 2, to: 2 }, total: 9 },
    })?.items[0]).toEqual({
      oid: OID, short_oid: OID.slice(0, 12), summary: "ship", author: "u", committed_at: 1, parents: [],
    });
    expect(parseCommitDiff({
      oid: OID, short_oid: OID.slice(0, 12), summary: "ship", message: "ship", author: "u",
      committed_at: 1, parents: [],
      files: [{ path: "x", old_path: null, status: "A", lines: [{ origin: "+", content: "x", secret: "drop" }], secret: "drop" }],
      secret: "drop",
    })?.files[0]).toEqual({
      path: "x", old_path: null, status: "A", lines: [{ origin: "+", content: "x" }],
    });
  });

  it.each([
    () => parseCommitsPage({ items: [], page: { next_cursor: null, limit: 0 } }),
    () => parseCommitsPage({ items: Array(101).fill({}), page: { next_cursor: null, limit: 50 } }),
    () => parseCommitDiff({ oid: "short" }),
    () => parseCommitDiff({
      oid: OID, short_oid: OID.slice(0, 12), summary: "x", message: "x", author: "u",
      committed_at: 1, parents: [], files: [{ path: "../x", old_path: null, status: "A", lines: [] }],
    }),
    () => parseCommitDiff({
      oid: OID, short_oid: OID.slice(0, 12), summary: "x", message: "x", author: "u",
      committed_at: 1, parents: [], files: [{ path: "x", old_path: null, status: "A", lines: [{ origin: "!", content: "x" }] }],
    }),
  ])("rejects malformed or unbounded commit payload", (parse) => {
    expect(parse()).toBeNull();
  });
});
