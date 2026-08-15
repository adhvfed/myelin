import { describe, expect, it } from "vitest";

import { parsePr, parsePrDiff, parsePrListPage } from "./pr-read-response";

const BASE = "0123456789abcdef0123456789abcdef01234567";
const HEAD = "89abcdef0123456789abcdef0123456789abcdef";
const row = {
  number: 42,
  title: "Bound the PR read path",
  pr_state: "open",
  base_ref: "refs/heads/main",
  head_ref: "refs/heads/read-boundary",
  author: "reviewer@example.invalid",
  author_is_agent: false,
  reviews: 1,
  review_state: "approved",
  you_are_requested: false,
  checks_summary: { verdict: "pass", passing: 2, failing: 0, total: 2 },
  updated_at: 1_719_450_001,
  repo: "core",
};

describe("PR read response projection", () => {
  it("projects list pages and drops nested surplus fields", () => {
    expect(parsePrListPage({
      items: [{ ...row, secret: "drop", checks_summary: { ...row.checks_summary, secret: "drop" } }],
      page: { next_cursor: null, prev_cursor: "0", limit: 50, offset: 50, total: 51, secret: "drop" },
      counts: { open: 1, merged: 0, closed: 0, all: 1, yours: 1, needs_review: 0, secret: 9 },
      secret: "drop",
    }, "repo")).toEqual({
      items: [row],
      page: { next_cursor: null, prev_cursor: "0", limit: 50, offset: 50, total: 51 },
      counts: { open: 1, merged: 0, closed: 0, all: 1, yours: 1, needs_review: 0 },
    });

    expect(parsePrListPage({
      items: [{ ...row, repo: "core" }],
      page: { next_cursor: null, prev_cursor: null, limit: 50 },
      counts: { bucket: 1, internal: 1 },
    }, "cross")?.counts).toEqual({ bucket: 1 });
  });

  it("projects the durable PR record", () => {
    expect(parsePr({
      number: 42,
      ref: "myelin://acme/git/pr/platform/api:42",
      pr_state: "open",
      title: "Bound the PR read path",
      body_md: "No unchecked JSON.",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/read-boundary",
      head_oid: HEAD,
      author: "reviewer@example.invalid",
      author_is_agent: false,
      reviews: 1,
      created_at: 1_719_450_000,
      updated_at: 1_719_450_001,
      commits_count: 2,
      commits_count_capped: false,
      durable: true,
      storage_key: "drop",
    })).toEqual({
      number: 42,
      ref: "myelin://acme/git/pr/platform/api:42",
      pr_state: "open",
      title: "Bound the PR read path",
      body_md: "No unchecked JSON.",
      base_ref: "refs/heads/main",
      head_ref: "refs/heads/read-boundary",
      head_oid: HEAD,
      author: "reviewer@example.invalid",
      author_is_agent: false,
      reviews: 1,
      created_at: 1_719_450_000,
      updated_at: 1_719_450_001,
      commits_count: 2,
      commits_count_capped: false,
      durable: true,
    });
  });

  it("projects PR diffs recursively", () => {
    const projected = parsePrDiff({
      number: 42,
      base_ref: "refs/heads/main",
      base_oid: BASE,
      short_base_oid: BASE.slice(0, 7),
      head_oid: HEAD,
      short_head_oid: HEAD.slice(0, 12),
      three_dot: true,
      files: [{
        path: "src/read.ts",
        old_path: null,
        new_blob_oid: HEAD,
        status: "M",
        kind: "text",
        additions: 1,
        deletions: 0,
        size_bytes: 12,
        hunks: [{
          header: "@@ -1 +1,2 @@",
          old_start: 1,
          old_lines: 1,
          new_start: 1,
          new_lines: 2,
          lines: [
            { origin: " ", content: "before", old_no: 1, new_no: 1, secret: "drop" },
            { origin: "+", content: "after", old_no: null, new_no: 2 },
          ],
          secret: "drop",
        }],
        deleted_body_available: false,
        truncated: false,
        secret: "drop",
      }],
      restricted_files: 0,
      total_files: 1,
      total_additions: 1,
      total_deletions: 0,
      page: { next_cursor: null, limit: 50, secret: "drop" },
      secret: "drop",
    });
    expect(projected?.files[0]?.hunks[0]?.lines).toEqual([
      { origin: " ", content: "before", old_no: 1, new_no: 1 },
      { origin: "+", content: "after", old_no: null, new_no: 2 },
    ]);
    expect(projected?.files[0]?.new_blob_oid).toBe(HEAD);
    expect(projected?.page).toEqual({ next_cursor: null, limit: 50 });
  });

  it.each([
    () => parsePrListPage({ items: [row], page: { next_cursor: null, prev_cursor: null, limit: 50 }, counts: { bucket: 1 } }, "repo"),
    () => parsePrListPage({ items: [{ ...row, checks_summary: { verdict: "pass", passing: 2, failing: 1, total: 2 } }], page: { next_cursor: null, prev_cursor: null, limit: 50 }, counts: { bucket: 1 } }, "cross"),
    () => parsePr({ number: 1, durable: false }),
    () => parsePr({
      number: 1, ref: "myelin://acme/git/pr/core:1", pr_state: "open", title: "x", body_md: null, base_ref: "main",
      head_ref: "refs/heads/x", head_oid: HEAD, author: "u", reviews: 0, created_at: null, durable: true,
    }),
    () => parsePrDiff({
      number: 1, base_ref: "refs/heads/main", base_oid: BASE, short_base_oid: "deadbee",
      head_oid: HEAD, short_head_oid: HEAD.slice(0, 7), three_dot: true, files: [],
      restricted_files: 0, total_files: 0, total_additions: 0, total_deletions: 0,
      page: { next_cursor: null, limit: 50 },
    }),
    () => parsePrDiff({
      number: 1, base_ref: "refs/heads/main", base_oid: BASE, short_base_oid: BASE.slice(0, 7),
      head_oid: HEAD, short_head_oid: HEAD.slice(0, 7), three_dot: true,
      files: [{
        path: "../secret", old_path: null, new_blob_oid: HEAD, status: "M", kind: "text", additions: 0,
        deletions: 0, size_bytes: null, hunks: [], deleted_body_available: false, truncated: false,
      }],
      restricted_files: 0, total_files: 1, total_additions: 0, total_deletions: 0,
      page: { next_cursor: null, limit: 50 },
    }),
    () => parsePrDiff({
      number: 1, base_ref: "refs/heads/main", base_oid: BASE, short_base_oid: BASE.slice(0, 7),
      head_oid: HEAD, short_head_oid: HEAD.slice(0, 7), three_dot: true,
      files: [{
        path: "deleted.rs", old_path: null, new_blob_oid: HEAD, status: "D", kind: "text",
        additions: 0, deletions: 1, size_bytes: null, hunks: [],
        deleted_body_available: true, truncated: false,
      }],
      restricted_files: 0, total_files: 1, total_additions: 0, total_deletions: 1,
      page: { next_cursor: null, limit: 50 },
    }),
  ])("rejects malformed or inconsistent PR payloads", (parse) => {
    expect(parse()).toBeNull();
  });
});
