const MAX_TEXT_BYTES = 512 * 1024;
const MAX_PATH_BYTES = 4 * 1024;
const MAX_REF_BYTES = 4 * 1024;
const MAX_HUNKS = 20_000;
const MAX_LINES = 10_000;
const utf8 = new TextEncoder();

type WireRecord = Record<string, unknown>;

export interface BlameCommitVM {
  oid: string;
  summary: string;
  author: string;
  committed_at: number;
}

export interface BlameHunkVM {
  start_line: number;
  line_count: number;
  commit: BlameCommitVM;
}

export interface BlameVM {
  path: string;
  ref: string;
  snapshot_oid: string;
  contents: string;
  hunks: BlameHunkVM[];
}

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function bounded(value: unknown, maximum: number): value is string {
  return typeof value === "string" && utf8.encode(value).byteLength <= maximum;
}

function displayText(value: unknown, maximum: number): value is string {
  return bounded(value, maximum) && ![...value].some((character) => {
    const point = character.codePointAt(0)!;
    return point === 0 || point === 0x7f;
  });
}

function uint(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function gitOid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{40}$/.test(value);
}

function repoPath(value: unknown): value is string {
  return displayText(value, MAX_PATH_BYTES) && value.length > 0 && !value.startsWith("/") &&
    !value.includes("\\") && value.split("/").every((part) =>
      part !== "" && part !== "." && part !== ".."
    );
}

function commit(value: unknown): BlameCommitVM | null {
  const candidate = record(value);
  if (!candidate || !gitOid(candidate.oid) || !displayText(candidate.summary, 8 * 1024) ||
      !displayText(candidate.author, 1_024) || !uint(candidate.committed_at)) return null;
  return {
    oid: candidate.oid,
    summary: candidate.summary,
    author: candidate.author,
    committed_at: candidate.committed_at,
  };
}

/** Split content using Git's line model: a terminal newline ends a line; it does not add an empty one. */
export function splitRepositoryLines(contents: string): string[] {
  if (contents === "") return [];
  const lines = contents.split("\n");
  if (contents.endsWith("\n")) lines.pop();
  return lines;
}

export function parseBlame(value: unknown): BlameVM | null {
  const candidate = record(value);
  if (!candidate || !repoPath(candidate.path) || !displayText(candidate.ref, MAX_REF_BYTES) ||
      candidate.ref.length === 0 || !gitOid(candidate.snapshot_oid) ||
      !bounded(candidate.contents, MAX_TEXT_BYTES) || !Array.isArray(candidate.hunks) ||
      candidate.hunks.length > MAX_HUNKS) return null;

  const lines = splitRepositoryLines(candidate.contents);
  if (lines.length > MAX_LINES) return null;
  let expectedStart = 1;
  const hunks: BlameHunkVM[] = [];
  for (const rawHunk of candidate.hunks) {
    const hunk = record(rawHunk);
    const attribution = commit(hunk?.commit);
    if (!hunk || !attribution || !uint(hunk.start_line) || hunk.start_line !== expectedStart ||
        !uint(hunk.line_count) || hunk.line_count < 1) return null;
    expectedStart += hunk.line_count;
    if (!Number.isSafeInteger(expectedStart)) return null;
    hunks.push({
      start_line: hunk.start_line,
      line_count: hunk.line_count,
      commit: attribution,
    });
  }
  if (expectedStart - 1 !== lines.length) return null;

  return {
    path: candidate.path,
    ref: candidate.ref,
    snapshot_oid: candidate.snapshot_oid,
    contents: candidate.contents,
    hunks,
  };
}
