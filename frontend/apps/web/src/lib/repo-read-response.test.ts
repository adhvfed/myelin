import { describe, expect, it } from "vitest";

import { parseBlob, parseRefs, parseRepoHome, parseTree } from "./repo-read-response";

const OID = "0123456789abcdef0123456789abcdef01234567";
const REPO_REF = "myelin://acme/git/repo/core";

describe("repository read response projection", () => {
  it("projects bounded known fields and drops surplus fields recursively", () => {
    expect(parseRepoHome({
      state: "populated",
      slug: "acme/core",
      ref: REPO_REF,
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
      ref: REPO_REF,
      default_branch: "main",
      entries: [{
        name: "x", path: "src/x", is_dir: false, size: 1,
        latest_commit: { short_oid: OID.slice(0, 12), oid: OID, summary: "ship", committed_at: 1 },
      }],
    });
  });

  it("decodes the repo-home tree continuation with its exact qualified snapshot", () => {
    expect(parseRepoHome({
      state: "populated",
      slug: "acme/core",
      ref: REPO_REF,
      default_branch: "main",
      snapshot_oid: OID,
      entries: [{ path: "src", is_dir: true }],
      entries_page: {
        ref: "refs/heads/main", next_cursor: "gt1_a-b_c", limit: 1,
        snapshot_oid: OID, internal: "drop",
      },
    })).toMatchObject({
      snapshot_oid: OID,
      entries_page: {
        ref: "refs/heads/main", next_cursor: "gt1_a-b_c", limit: 1, snapshot_oid: OID,
      },
    });
  });

  it("projects refs, trees, blobs, and kind-mismatch redirects", () => {
    expect(parseRefs({
      branches: [{ name: "main", oid: OID, is_default: true, secret: "drop" }],
      tags: [], default_branch: "main", pinned: [],
      page: { next_cursor: null, limit: 1 }, secret: "drop",
    })).toEqual({
      branches: [{ name: "main", oid: OID, is_default: true }],
      tags: [], default_branch: "main", pinned: [], page: { next_cursor: null, limit: 1 },
    });
    expect(parseTree({
      ref: "refs/heads/main", path: "", snapshot_oid: OID,
      entries: [{ path: "x", is_dir: false }], readme: "# readme",
      page: { next_cursor: "gt1_a-b_c", limit: 1, secret: "drop" }, secret: "drop",
    })).toEqual({
      ref: "refs/heads/main", path: "", snapshot_oid: OID,
      entries: [{ path: "x", is_dir: false }], readme: "# readme",
      page: { next_cursor: "gt1_a-b_c", limit: 1 },
    });
    expect(parseTree({
      ref: "refs/heads/main", path: "README.md", redirect_to_blob: true, secret: "drop",
    })).toEqual({
      ref: "refs/heads/main", path: "README.md", redirect_to_blob: true,
    });
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

  it.each([
    undefined,
    "myelin://other/git/repo/core",
    "myelin://acme/git/repo/other",
    "myelin://acme/git/pr/core",
    "myelin://acme/git/repo/core#check-build",
  ])("rejects a repository home without its exact canonical identity: %s", (ref) => {
    expect(parseRepoHome({
      state: "empty", slug: "acme/core", ref, default_branch: "main",
    })).toBeNull();
  });

  it("keeps a hierarchical repository slug and reference byte-for-byte aligned", () => {
    expect(parseRepoHome({
      state: "empty",
      slug: "acme/platform/api",
      ref: "myelin://acme/git/repo/platform/api",
      default_branch: "main",
    })).toEqual({
      state: "empty",
      slug: "acme/platform/api",
      ref: "myelin://acme/git/repo/platform/api",
      default_branch: "main",
    });
  });

  it("rejects unsafe browse projections", () => {
    expect(parseRefs({ branches: [{ name: "main", oid: "short" }], tags: [], default_branch: "main" })).toBeNull();
    expect(parseTree({ path: "../secret", entries: [] })).toBeNull();
    expect(parseBlob({ path: "x", contents: "x", base_oid: "x", viewer_may_edit: false, raw_url: "https://evil.test/x" })).toBeNull();
    expect(parseBlob({ path: "x", contents: "not empty", base_oid: OID, viewer_may_edit: false, preview_unavailable: true })).toBeNull();
  });

  it.each([
    {},
    { ref: "main", path: "", redirect_to_blob: true },
    {
      ref: "main", path: "README.md", redirect_to_blob: true,
      snapshot_oid: OID, entries: [], page: { next_cursor: null, limit: 1 },
    },
    {
      ref: "main", path: "", snapshot_oid: OID, entries: [],
    },
    {
      ref: "main", path: "", entries: [], page: { next_cursor: null, limit: 1 },
    },
    {
      ref: "main", path: "", snapshot_oid: "A".repeat(40), entries: [],
      page: { next_cursor: null, limit: 1 },
    },
    {
      ref: "main", path: "", snapshot_oid: OID, entries: [],
      page: { next_cursor: "not-a-tree-cursor", limit: 1 },
    },
    {
      ref: "main", path: "", snapshot_oid: OID,
      entries: [{ path: "a", is_dir: false }, { path: "b", is_dir: false }],
      page: { next_cursor: null, limit: 1 },
    },
    {
      state: "populated", slug: "acme/core", ref: REPO_REF, default_branch: "main", entries: [],
      entries_page: { ref: "main", next_cursor: null, limit: 100, snapshot_oid: OID },
    },
    {
      state: "populated", slug: "acme/core", ref: REPO_REF, default_branch: "main", entries: [],
      snapshot_oid: OID,
      entries_page: {
        ref: "refs/heads/main", next_cursor: null, limit: 100,
        snapshot_oid: "1".repeat(40),
      },
    },
    {
      state: "populated", slug: "acme/core", ref: REPO_REF, default_branch: "main", entries: [],
      snapshot_oid: OID,
    },
    {
      state: "populated", slug: "acme/core", ref: REPO_REF, default_branch: "main", entries: [],
      entries_page: {
        ref: "refs/heads/main", next_cursor: null, limit: 100, snapshot_oid: OID,
      },
    },
  ])("rejects malformed modern tree pagination %#", (value) => {
    const parsed = "state" in value ? parseRepoHome(value) : parseTree(value);
    expect(parsed).toBeNull();
  });

  it("projects the paginated refs contract freshly, including pins outside the page", () => {
    expect(parseRefs({
      branches: [{ name: "feature", oid: OID, is_default: false, secret: "drop" }],
      tags: [],
      default_branch: "main",
      pinned: [{
        kind: "branch", full_name: "refs/heads/main", name: "main", oid: OID,
        is_default: true, secret: "drop",
      }],
      page: { next_cursor: "gr1_abc-DEF_123", limit: 1, total: 500 },
      secret: "drop",
    })).toEqual({
      branches: [{ name: "feature", oid: OID, is_default: false }],
      tags: [],
      default_branch: "main",
      pinned: [{
        kind: "branch", full_name: "refs/heads/main", name: "main", oid: OID,
        is_default: true,
      }],
      page: { next_cursor: "gr1_abc-DEF_123", limit: 1 },
    });
  });

  it.each([
    {
      branches: [{ name: "a", oid: OID, is_default: false }],
      tags: [{ name: "v1", oid: OID }], default_branch: "main", pinned: [],
      page: { next_cursor: null, limit: 1 },
    },
    { branches: [], tags: [], default_branch: "main", pinned: [], page: { next_cursor: null, limit: 101 } },
    {
      branches: [], tags: [], default_branch: "main",
      pinned: Array(3).fill({ kind: "branch", full_name: "refs/heads/main", name: "main", oid: OID, is_default: true }),
      page: { next_cursor: null, limit: 1 },
    },
    {
      branches: [{ name: "main", oid: OID }], tags: [], default_branch: "main", pinned: [],
      page: { next_cursor: null, limit: 1 },
    },
    {
      branches: [{ name: "main", oid: OID, is_default: false }], tags: [],
      default_branch: "main", pinned: [], page: { next_cursor: null, limit: 1 },
    },
    {
      branches: [], tags: [], default_branch: "main", pinned: [],
      page: { next_cursor: "not-a-cursor", limit: 1 },
    },
    {
      branches: [], tags: [], default_branch: "main",
      pinned: [{ kind: "tag", full_name: "refs/heads/main", name: "main", oid: OID, is_default: false }],
      page: { next_cursor: null, limit: 1 },
    },
    {
      branches: [], tags: [], default_branch: "main",
      pinned: [{ kind: "branch", full_name: "refs/heads/main", name: "other", oid: OID, is_default: false }],
      page: { next_cursor: null, limit: 1 },
    },
    {
      branches: [], tags: [], default_branch: "main",
      pinned: [{ kind: "branch", full_name: "refs/heads/main", name: "main", oid: OID, is_default: false }],
      page: { next_cursor: null, limit: 1 },
    },
    { branches: [], tags: [], default_branch: "main", page: { next_cursor: null, limit: 1 } },
    { branches: [], tags: [], default_branch: "main", pinned: [] },
    {
      branches: [], tags: [], default_branch: "main",
      pinned: [{ kind: "branch", full_name: "refs/heads/main", name: "main", oid: "A".repeat(40), is_default: true }],
      page: { next_cursor: null, limit: 1 },
    },
    {
      branches: [{ name: "x".repeat(4 * 1024 + 1), oid: OID, is_default: false }],
      tags: [], default_branch: "main", pinned: [], page: { next_cursor: null, limit: 1 },
    },
    {
      branches: [], tags: [], default_branch: "main", pinned: [],
      page: { next_cursor: `gr1_${"x".repeat(8 * 1024)}`, limit: 1 },
    },
  ])("rejects an invalid refs response %#", (value) => {
    expect(parseRefs(value)).toBeNull();
  });
});
