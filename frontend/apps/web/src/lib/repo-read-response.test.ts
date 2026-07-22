import { describe, expect, it } from "vitest";

import { parseBlob, parseRefs, parseRepoHome, parseReposPage, parseTree } from "./repo-read-response";

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

  it("projects refs, trees, blobs, and kind-mismatch redirects", () => {
    expect(parseRefs({
      branches: [{ name: "main", oid: OID, is_default: true, secret: "drop" }],
      tags: [], default_branch: "main", secret: "drop",
    })).toEqual({ branches: [{ name: "main", oid: OID, is_default: true }], tags: [], default_branch: "main" });
    expect(parseTree({ ref: "main", path: "", entries: [{ path: "x", is_dir: false }], readme: null, secret: "drop" }))
      .toEqual({ ref: "main", path: "", entries: [{ path: "x", is_dir: false }] });
    expect(parseBlob({
      path: "x", contents: "hello", base_oid: "blake3:value", viewer_may_edit: false,
      preview_unavailable: false, download_available: true, raw_url: "/raw/x", secret: "drop",
    })).toEqual({
      path: "x", contents: "hello", base_oid: "blake3:value", viewer_may_edit: false,
      preview_unavailable: false, download_available: true, raw_url: "/raw/x",
    });
    expect(parseBlob({
      path: "large.bin", contents: "", base_oid: OID, viewer_may_edit: false,
      size_bytes: 65 * 1024 * 1024, preview_unavailable: true, download_available: false,
    })).toEqual({
      path: "large.bin", contents: "", base_oid: OID, viewer_may_edit: false,
      size_bytes: 65 * 1024 * 1024, preview_unavailable: true, download_available: false,
    });
    expect(parseBlob({ path: "dir", ref: "main", redirect_to_tree: true, secret: "drop" }))
      .toEqual({ path: "dir", contents: "", base_oid: "", viewer_may_edit: false, redirect_to_tree: true });
  });

  it.each([
    { state: "populated", slug: "../core", default_branch: "main" },
    { state: "populated", slug: "core", default_branch: "main", entries: [{ path: "../x", is_dir: false }] },
    { state: "populated", slug: "core", default_branch: "main", entries: Array(1001).fill({ path: "x", is_dir: false }) },
    { state: "populated", slug: "core", default_branch: "main", latest_commit: { short_oid: "short", summary: "x", committed_at: 1 } },
  ])("rejects malformed or unbounded home payload %#", (value) => {
    expect(parseRepoHome(value)).toBeNull();
  });

  it("rejects unsafe browse projections", () => {
    expect(parseRefs({ branches: [{ name: "main", oid: "short" }], tags: [], default_branch: "main" })).toBeNull();
    expect(parseTree({ path: "../secret", entries: [] })).toBeNull();
    expect(parseBlob({ path: "x", contents: "x", base_oid: "x", viewer_may_edit: false, raw_url: "https://evil.test/x" })).toBeNull();
    expect(parseBlob({ path: "x", contents: "not empty", base_oid: OID, viewer_may_edit: false, preview_unavailable: true })).toBeNull();
  });
});
