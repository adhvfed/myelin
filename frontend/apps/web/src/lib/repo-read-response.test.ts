import { describe, expect, it } from "vitest";

import { parseRepoHome, parseReposPage } from "./repo-read-response";

const OID = "0123456789abcdef0123456789abcdef01234567";

describe("repository read response projection", () => {
  it("projects bounded known fields and drops surplus fields recursively", () => {
    expect(parseRepoHome({
      state: "populated",
      slug: "acme/core",
      default_branch: "main",
      entries: [{
        name: "x", path: "src/x", is_dir: false, size: 1, secret: "drop",
        latest_commit: {
          short_oid: OID.slice(0, 12), oid: OID, summary: "ship", committed_at: 1, secret: "drop",
        },
      }],
      secret: "drop",
    })).toEqual({
      state: "populated",
      slug: "acme/core",
      default_branch: "main",
      entries: [{
        name: "x", path: "src/x", is_dir: false, size: 1,
        latest_commit: { short_oid: OID.slice(0, 12), oid: OID, summary: "ship", committed_at: 1 },
      }],
    });
  });

  it("projects the page envelope without totals or internal metadata", () => {
    expect(parseReposPage({
      items: [{ state: "empty", slug: "acme/core", default_branch: "main" }],
      page: { next_cursor: null, limit: 50, total: 99 },
      internal: "drop",
    })).toEqual({
      items: [{ state: "empty", slug: "acme/core", default_branch: "main" }],
      page: { next_cursor: null, limit: 50 },
    });
  });

  it.each([
    { state: "populated", slug: "../core", default_branch: "main" },
    { state: "populated", slug: "core", default_branch: "main", entries: [{ path: "../x", is_dir: false }] },
    { state: "populated", slug: "core", default_branch: "main", entries: Array(1001).fill({ path: "x", is_dir: false }) },
    { state: "populated", slug: "core", default_branch: "main", latest_commit: { short_oid: "short", summary: "x", committed_at: 1 } },
  ])("rejects malformed or unbounded home payload %#", (value) => {
    expect(parseRepoHome(value)).toBeNull();
  });
});
