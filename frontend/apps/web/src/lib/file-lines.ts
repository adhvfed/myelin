// Runtime codec for the browser-exposed expand-context query. TypeScript types disappear at the
// server-function boundary, so both the request and Edge response are reconstructed from bounded,
// exact wire shapes before they can influence a URL or the diff UI.

export const MAX_FILE_LINES_RANGE = 1_000;
export const MAX_FILE_LINES_BLOB_BYTES = 512 * 1024;
export const MAX_FILE_LINES_PATH_BYTES = 4 * 1024;

export interface FileLinesInput {
  repo: string;
  oid: string;
  path: string;
  start: number;
  end: number;
}

export interface FileLine {
  origin: " ";
  content: string;
  old_no: null;
  new_no: number;
}

type WireRecord = Record<string, unknown>;

const utf8 = new TextEncoder();

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exactKeys(value: WireRecord, allowed: readonly string[]): boolean {
  const allow = new Set(allowed);
  return Object.keys(value).every((key) => allow.has(key));
}

function boundedString(value: unknown, maximum: number): value is string {
  return typeof value === "string" && utf8.encode(value).byteLength <= maximum;
}

function repoSlug(value: unknown): value is string {
  return boundedString(value, 255) && value.length > 0 && value.split("/").every((part) =>
    part !== "" && part !== "." && part !== ".." && /^[A-Za-z0-9._-]+$/.test(part)
  );
}

function gitPath(value: unknown): value is string {
  if (!boundedString(value, MAX_FILE_LINES_PATH_BYTES) || !value || value.startsWith("/") ||
      value.includes("\\") || [...value].some((character) => {
        const point = character.codePointAt(0)!;
        return point <= 0x1f || point === 0x7f;
      })) return false;
  return value.split("/").every((part) => part !== "" && part !== "." && part !== "..");
}

function lineNumber(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > 0 && (value as number) <= 0xffff_ffff;
}

export function parseFileLinesInput(value: unknown): FileLinesInput | null {
  const input = record(value);
  if (!input || !exactKeys(input, ["repo", "oid", "path", "start", "end"]) ||
      !repoSlug(input.repo) || typeof input.oid !== "string" ||
      !/^[0-9a-f]{40}$/.test(input.oid) || !gitPath(input.path) ||
      !lineNumber(input.start) || !lineNumber(input.end) || input.end < input.start ||
      input.end - input.start + 1 > MAX_FILE_LINES_RANGE) return null;
  return {
    repo: input.repo,
    oid: input.oid,
    path: input.path,
    start: input.start,
    end: input.end,
  };
}

export function parseFileLinesResponse(value: unknown): { lines: FileLine[] } | null {
  const response = record(value);
  if (!response || !Array.isArray(response.lines) || response.lines.length > MAX_FILE_LINES_RANGE) {
    return null;
  }
  const lines: FileLine[] = [];
  let contentBytes = 0;
  for (const value of response.lines) {
    const line = record(value);
    if (!line || line.origin !== " " || typeof line.content !== "string" ||
        line.old_no !== null || !lineNumber(line.new_no)) return null;
    contentBytes += utf8.encode(line.content).byteLength;
    if (contentBytes > MAX_FILE_LINES_BLOB_BYTES) return null;
    lines.push({ origin: " ", content: line.content, old_no: null, new_no: line.new_no });
  }
  return { lines };
}
