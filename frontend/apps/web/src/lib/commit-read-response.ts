import type {
  CommitDiffVM,
  CommitRowVM,
  CommitsPage,
  DiffFileVM,
  DiffLineVM,
  PrCommitsPage,
} from "./api";
import { isPrCommitCursor } from "./git-read-input";

const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const actual = Object.keys(value);
  return actual.length === keys.length && actual.every((key) => keys.includes(key));
}

function bounded(value: unknown, maximum: number): value is string {
  return typeof value === "string" && utf8.encode(value).byteLength <= maximum;
}

function uint(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function oid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{40}$/.test(value);
}

function cursor(value: unknown): value is string | null {
  return value === null || bounded(value, 4 * 1024);
}

function path(value: unknown): value is string {
  return bounded(value, 4 * 1024) && value.length > 0 && !value.startsWith("/") &&
    !value.includes("\\") && value.split("/").every((part) =>
      part !== "" && part !== "." && part !== ".."
    );
}

function commitRow(value: unknown): CommitRowVM | null {
  const row = record(value);
  if (!row || !oid(row.oid) || typeof row.short_oid !== "string" ||
      !/^[0-9a-f]{7,40}$/.test(row.short_oid) || !bounded(row.summary, 8 * 1024) ||
      !bounded(row.author, 1_024) || !uint(row.committed_at) || !Array.isArray(row.parents) ||
      row.parents.length > 64 || !row.parents.every(oid)) return null;
  return {
    oid: row.oid,
    short_oid: row.short_oid,
    summary: row.summary,
    author: row.author,
    committed_at: row.committed_at,
    parents: [...row.parents],
  };
}

function exactCommitRow(value: unknown): CommitRowVM | null {
  const row = record(value);
  if (!row || !exact(row, ["oid", "short_oid", "summary", "author", "committed_at", "parents"])) {
    return null;
  }
  const projected = commitRow(row);
  return projected && projected.short_oid === projected.oid.slice(0, 12) ? projected : null;
}

export function parseCommitsPage(value: unknown): CommitsPage | null {
  const envelope = record(value);
  const page = record(envelope?.page);
  if (!envelope || !Array.isArray(envelope.items) || envelope.items.length > 100 || !page ||
      !cursor(page.next_cursor) || (page.prev_cursor !== undefined && !cursor(page.prev_cursor)) ||
      !Number.isSafeInteger(page.limit) || (page.limit as number) < 1 || (page.limit as number) > 100 ||
      (page.offset !== undefined && !uint(page.offset))) return null;
  const items = envelope.items.map(commitRow);
  if (!items.every((item): item is CommitRowVM => item !== null)) return null;
  let range: { from: number; to: number } | undefined;
  if (page.range !== undefined) {
    const candidate = record(page.range);
    if (!candidate || !uint(candidate.from) || !uint(candidate.to) || candidate.to < candidate.from) {
      return null;
    }
    range = { from: candidate.from, to: candidate.to };
  }
  return {
    items,
    page: {
      next_cursor: page.next_cursor,
      limit: page.limit as number,
      ...(page.prev_cursor === undefined ? {} : { prev_cursor: page.prev_cursor }),
      ...(page.offset === undefined ? {} : { offset: page.offset }),
      ...(range ? { range } : {}),
    },
  };
}

/** Strict projection for the snapshot-paged PR commit route; branch history keeps its legacy shape. */
export function parsePrCommitsPage(value: unknown): PrCommitsPage | null {
  const envelope = record(value);
  const page = record(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) ||
      !page || !exact(page, ["next_cursor", "limit"]) ||
      !Number.isSafeInteger(page.limit) || (page.limit as number) < 1 ||
      (page.limit as number) > 100 || envelope.items.length > (page.limit as number) ||
      (page.next_cursor !== null && !isPrCommitCursor(page.next_cursor))) return null;

  const items = envelope.items.map(exactCommitRow);
  if (!items.every((item): item is CommitRowVM => item !== null)) return null;
  const seen = new Set<string>();
  for (const item of items) {
    if (seen.has(item.oid)) return null;
    seen.add(item.oid);
  }
  return {
    items,
    page: { next_cursor: page.next_cursor, limit: page.limit as number },
  };
}

function diffLine(value: unknown): DiffLineVM | null {
  const line = record(value);
  if (!line || (line.origin !== "+" && line.origin !== "-" && line.origin !== " ") ||
      !bounded(line.content, 64 * 1024)) return null;
  for (const key of ["old_no", "new_no"] as const) {
    const number = line[key];
    if (number !== undefined && number !== null && (!uint(number) || number === 0)) return null;
  }
  return {
    origin: line.origin,
    content: line.content,
    ...(line.old_no === undefined ? {} : { old_no: line.old_no as number | null }),
    ...(line.new_no === undefined ? {} : { new_no: line.new_no as number | null }),
  };
}

function diffFile(value: unknown): DiffFileVM | null {
  const file = record(value);
  if (!file || !path(file.path) || (file.old_path !== null && !path(file.old_path)) ||
      typeof file.status !== "string" || !/^[AMDRC]$/.test(file.status) ||
      !Array.isArray(file.lines) || file.lines.length > 4_000) return null;
  const lines = file.lines.map(diffLine);
  return lines.every((line): line is DiffLineVM => line !== null)
    ? { path: file.path, old_path: file.old_path, status: file.status, lines }
    : null;
}

export function parseCommitDiff(value: unknown): CommitDiffVM | null {
  const commit = record(value);
  if (!commit || !oid(commit.oid) || typeof commit.short_oid !== "string" ||
      !/^[0-9a-f]{7,40}$/.test(commit.short_oid) || !bounded(commit.summary, 8 * 1024) ||
      !bounded(commit.message, 512 * 1024) || !bounded(commit.author, 1_024) ||
      !uint(commit.committed_at) || !Array.isArray(commit.parents) || commit.parents.length > 64 ||
      !commit.parents.every(oid) || !Array.isArray(commit.files) || commit.files.length > 1_000) {
    return null;
  }
  const files = commit.files.map(diffFile);
  return files.every((file): file is DiffFileVM => file !== null) ? {
    oid: commit.oid,
    short_oid: commit.short_oid,
    summary: commit.summary,
    message: commit.message,
    author: commit.author,
    committed_at: commit.committed_at,
    parents: [...commit.parents],
    files,
  } : null;
}
