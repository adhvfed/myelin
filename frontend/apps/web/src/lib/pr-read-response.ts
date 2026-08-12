// Runtime projections for PR read endpoints. Every nested value is bounded and rebuilt so an Edge
// regression cannot smuggle internal fields or unbounded data into the browser payload.

import type {
  ChecksSummaryVM,
  DiffHunkVM,
  DiffLineVM,
  PrDiffFileVM,
  PrDiffVM,
  PrListPage,
  PrListRowVM,
  PrVM,
} from "./api";
import { parseArtifactRef } from "./artifact-ref";

type WireRecord = Record<string, unknown>;
const utf8 = new TextEncoder();

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function bounded(value: unknown, maximum: number, allowEmpty = false): value is string {
  return typeof value === "string" && (allowEmpty || value.length > 0) &&
    utf8.encode(value).byteLength <= maximum;
}

function uint(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function positive(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > 0;
}

function oid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{40}$/.test(value);
}

function shortOid(value: unknown, full: string): value is string {
  return typeof value === "string" && /^[0-9a-f]{7,40}$/.test(value) && full.startsWith(value);
}

function cursor(value: unknown): value is string | null {
  return value === null || bounded(value, 4 * 1024);
}

function safePath(value: unknown): value is string {
  return bounded(value, 4 * 1024) && !value.startsWith("/") && !value.includes("\\") &&
    ![...value].some((character) => {
      const point = character.codePointAt(0)!;
      return point <= 0x1f || point === 0x7f;
    }) && value.split("/").every((part) => part !== "" && part !== "." && part !== "..");
}

function branchRef(value: unknown): value is string {
  return bounded(value, 4 * 1024) && value.startsWith("refs/heads/") && !value.includes("\\") &&
    ![...value].some((character) => character.codePointAt(0)! <= 0x20) &&
    value.split("/").every((part) => part !== "" && part !== "." && part !== "..");
}

function repo(value: unknown): value is string {
  return bounded(value, 255) && value.split("/").every((part) =>
    part !== "" && part !== "." && part !== ".." && /^[A-Za-z0-9._-]+$/.test(part)
  );
}

function prRef(value: unknown, number: number): value is string {
  const parsed = parseArtifactRef(value);
  return parsed !== null && parsed.subsystem === "git" && parsed.type === "pr" &&
    parsed.sub === null && parsed.id.endsWith(`:${number}`);
}

function checksSummary(value: unknown): ChecksSummaryVM | null {
  const input = record(value);
  if (!input || !["pass", "fail", "running", "none", "unavailable"].includes(input.verdict as string) ||
      !uint(input.passing) || !uint(input.failing) || !uint(input.total) ||
      input.passing + input.failing > input.total) return null;
  return {
    verdict: input.verdict as ChecksSummaryVM["verdict"],
    passing: input.passing,
    failing: input.failing,
    total: input.total,
  };
}

function prListRow(value: unknown): PrListRowVM | null {
  const input = record(value);
  const summary = checksSummary(input?.checks_summary);
  if (!input || !positive(input.number) ||
      !(input.title === null || bounded(input.title, 512)) ||
      !["draft", "open", "merged", "closed"].includes(input.pr_state as string) ||
      !branchRef(input.base_ref) || !branchRef(input.head_ref) || !bounded(input.author, 4 * 1024) ||
      typeof input.author_is_agent !== "boolean" || !uint(input.reviews) ||
      !["requested", "approved", "changes", "none"].includes(input.review_state as string) ||
      typeof input.you_are_requested !== "boolean" || !summary ||
      !(input.updated_at === null || uint(input.updated_at)) ||
      !(input.repo === null || repo(input.repo))) return null;
  return {
    number: input.number,
    title: input.title,
    pr_state: input.pr_state as PrListRowVM["pr_state"],
    base_ref: input.base_ref,
    head_ref: input.head_ref,
    author: input.author,
    author_is_agent: input.author_is_agent,
    reviews: input.reviews,
    review_state: input.review_state as PrListRowVM["review_state"],
    you_are_requested: input.you_are_requested,
    checks_summary: summary,
    updated_at: input.updated_at,
    repo: input.repo,
  };
}

function countRecord(value: unknown, keys: readonly string[]): Record<string, number> | null {
  const input = record(value);
  if (!input || !keys.every((key) => uint(input[key]))) return null;
  return Object.fromEntries(keys.map((key) => [key, input[key] as number]));
}

export function parsePrListPage(value: unknown, scope: "repo" | "cross"): PrListPage | null {
  const input = record(value);
  const page = record(input?.page);
  const countKeys = scope === "repo"
    ? ["open", "merged", "closed", "all", "yours", "needs_review"]
    : ["bucket"];
  const counts = countRecord(input?.counts, countKeys);
  if (!input || !Array.isArray(input.items) || input.items.length > 100 || !page || !counts ||
      !cursor(page.next_cursor) || !cursor(page.prev_cursor) || !positive(page.limit) || page.limit > 100 ||
      (page.offset !== undefined && !uint(page.offset)) ||
      (page.total !== undefined && !uint(page.total))) return null;
  const items = input.items.map(prListRow);
  if (!items.every((item): item is PrListRowVM => item !== null)) return null;
  return {
    items,
    page: {
      next_cursor: page.next_cursor,
      prev_cursor: page.prev_cursor,
      limit: page.limit,
      ...(page.offset === undefined ? {} : { offset: page.offset }),
      ...(page.total === undefined ? {} : { total: page.total }),
    },
    counts,
  };
}

export function parsePr(value: unknown): PrVM | null {
  const input = record(value);
  if (!input || !positive(input.number) || !prRef(input.ref, input.number) ||
      !["draft", "open", "merged", "closed"].includes(input.pr_state as string) ||
      !(input.title === null || bounded(input.title, 512)) ||
      !(input.body_md === null || bounded(input.body_md, 64 * 1024, true)) ||
      !branchRef(input.base_ref) || !branchRef(input.head_ref) || !oid(input.head_oid) ||
      !bounded(input.author, 4 * 1024) ||
      (input.author_is_agent !== undefined && typeof input.author_is_agent !== "boolean") ||
      !uint(input.reviews) || !(input.created_at === null || uint(input.created_at)) ||
      (input.updated_at !== undefined && input.updated_at !== null && !uint(input.updated_at)) ||
      (input.commits_count !== undefined && input.commits_count !== null && !uint(input.commits_count)) ||
      (input.commits_count_capped !== undefined && typeof input.commits_count_capped !== "boolean") ||
      input.durable !== true) return null;
  return {
    number: input.number,
    ref: input.ref,
    pr_state: input.pr_state as PrVM["pr_state"],
    title: input.title,
    body_md: input.body_md,
    base_ref: input.base_ref,
    head_ref: input.head_ref,
    head_oid: input.head_oid,
    author: input.author,
    ...(input.author_is_agent === undefined ? {} : { author_is_agent: input.author_is_agent }),
    reviews: input.reviews,
    created_at: input.created_at,
    ...(input.updated_at === undefined ? {} : { updated_at: input.updated_at }),
    ...(input.commits_count === undefined ? {} : { commits_count: input.commits_count }),
    ...(input.commits_count_capped === undefined ? {} : { commits_count_capped: input.commits_count_capped }),
    durable: true,
  };
}

function diffLine(value: unknown): DiffLineVM | null {
  const input = record(value);
  if (!input || (input.origin !== "+" && input.origin !== "-" && input.origin !== " ") ||
      !bounded(input.content, 64 * 1024, true) ||
      !(input.old_no === null || positive(input.old_no)) ||
      !(input.new_no === null || positive(input.new_no)) ||
      (input.origin === "+" && input.old_no !== null) ||
      (input.origin === "-" && input.new_no !== null) ||
      (input.origin === " " && (input.old_no === null || input.new_no === null))) return null;
  return { origin: input.origin, content: input.content, old_no: input.old_no, new_no: input.new_no };
}

function diffHunk(value: unknown): DiffHunkVM | null {
  const input = record(value);
  if (!input || !bounded(input.header, 64 * 1024) || !uint(input.old_start) || !uint(input.old_lines) ||
      !uint(input.new_start) || !uint(input.new_lines) || !Array.isArray(input.lines) ||
      input.lines.length > 4_000) return null;
  const lines = input.lines.map(diffLine);
  return lines.every((line): line is DiffLineVM => line !== null) ? {
    header: input.header,
    old_start: input.old_start,
    old_lines: input.old_lines,
    new_start: input.new_start,
    new_lines: input.new_lines,
    lines,
  } : null;
}

function diffFile(value: unknown): PrDiffFileVM | null {
  const input = record(value);
  if (!input || !safePath(input.path) || !(input.old_path === null || safePath(input.old_path)) ||
      !(input.new_blob_oid === null || oid(input.new_blob_oid)) ||
      typeof input.status !== "string" || !/^[AMDRC]$/.test(input.status) ||
      !["text", "binary", "lfs", "submodule"].includes(input.kind as string) ||
      !uint(input.additions) || !uint(input.deletions) ||
      !(input.size_bytes === null || uint(input.size_bytes)) || !Array.isArray(input.hunks) ||
      input.hunks.length > 4_000 || typeof input.deleted_body_available !== "boolean" ||
      typeof input.truncated !== "boolean") return null;
  if ((input.status === "D" || input.kind === "submodule") !== (input.new_blob_oid === null)) {
    return null;
  }
  const hunks = input.hunks.map(diffHunk);
  if (!hunks.every((hunk): hunk is DiffHunkVM => hunk !== null) ||
      hunks.reduce((total, hunk) => total + hunk.lines.length, 0) > 4_000) return null;
  return {
    path: input.path,
    old_path: input.old_path,
    new_blob_oid: input.new_blob_oid,
    status: input.status,
    kind: input.kind as PrDiffFileVM["kind"],
    additions: input.additions,
    deletions: input.deletions,
    size_bytes: input.size_bytes,
    hunks,
    deleted_body_available: input.deleted_body_available,
    truncated: input.truncated,
  };
}

export function parsePrDiff(value: unknown): PrDiffVM | null {
  const input = record(value);
  const page = record(input?.page);
  if (!input || !positive(input.number) || !branchRef(input.base_ref) || !oid(input.base_oid) ||
      !shortOid(input.short_base_oid, input.base_oid) || !oid(input.head_oid) ||
      !shortOid(input.short_head_oid, input.head_oid) || typeof input.three_dot !== "boolean" ||
      !Array.isArray(input.files) || input.files.length > 100 || !uint(input.restricted_files) ||
      !uint(input.total_files) || input.files.length > input.total_files ||
      !uint(input.total_additions) || !uint(input.total_deletions) || !page ||
      !cursor(page.next_cursor) || !positive(page.limit) || page.limit > 100) return null;
  const files = input.files.map(diffFile);
  if (!files.every((file): file is PrDiffFileVM => file !== null) ||
      files.reduce((total, file) => total + file.additions, 0) > input.total_additions ||
      files.reduce((total, file) => total + file.deletions, 0) > input.total_deletions) return null;
  return {
    number: input.number,
    base_ref: input.base_ref,
    base_oid: input.base_oid,
    short_base_oid: input.short_base_oid,
    head_oid: input.head_oid,
    short_head_oid: input.short_head_oid,
    three_dot: input.three_dot,
    files,
    restricted_files: input.restricted_files,
    total_files: input.total_files,
    total_additions: input.total_additions,
    total_deletions: input.total_deletions,
    page: { next_cursor: page.next_cursor, limit: page.limit },
  };
}
