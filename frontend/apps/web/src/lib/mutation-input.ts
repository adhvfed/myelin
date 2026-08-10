// Runtime parsers for browser-supplied server-action arguments. TypeScript unions disappear at the
// RPC boundary, so every mutation is reconstructed from an exact, bounded wire shape before a URL
// is built or an Edge call can run.

export const MAX_ISSUE_TITLE_BYTES = 512;
export const MAX_PR_MARKDOWN_BYTES = 64 * 1024;
export const MAX_REPO_SLUG_BYTES = 255;
export const MAX_GIT_PATH_BYTES = 4 * 1024;

export type IssueMutation =
  | { op: "create"; projectId: string; title: string }
  | { op: "close"; issueId: string }
  | { op: "activation"; requestEventId: string };

export type PrMutation =
  | { op: "thread"; repo: string; n: number; body_md: string; anchor?: PrAnchor }
  | { op: "comment"; repo: string; n: number; threadId: string; body_md: string }
  | { op: "review-start"; repo: string; n: number }
  | { op: "review-comment"; repo: string; n: number; reviewId: string; body_md: string }
  | { op: "review-submit"; repo: string; n: number; reviewId: string; verdict: ReviewVerdict; summary_md?: string }
  | { op: "review-discard"; repo: string; n: number; reviewId: string }
  | { op: "merge"; repo: string; n: number };

export interface PrAnchor {
  path: string;
  line: number;
  side: "old" | "new";
}

type ReviewVerdict = "approved" | "changes_requested" | "commented";
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

function bounded(value: unknown, maxBytes: number): value is string {
  return typeof value === "string" && utf8.encode(value).byteLength <= maxBytes;
}

function hasControl(value: string): boolean {
  return [...value].some((character) => {
    const point = character.codePointAt(0)!;
    return point <= 0x1f || point === 0x7f;
  });
}

function canonicalUuid(value: unknown): value is string {
  return typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);
}

function canonicalUlid(value: unknown): value is string {
  return typeof value === "string" &&
    /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/.test(value);
}

function repoSlug(value: unknown): value is string {
  if (!bounded(value, MAX_REPO_SLUG_BYTES) || !value) return false;
  return value.split("/").every((part) =>
    part !== "" && part !== "." && part !== ".." && /^[A-Za-z0-9._-]+$/.test(part)
  );
}

function prNumber(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > 0;
}

function markdown(value: unknown, required: boolean): string | null {
  if (!bounded(value, MAX_PR_MARKDOWN_BYTES)) return null;
  const normalized = value.trim();
  return required && !normalized ? null : normalized;
}

function threadId(value: unknown): value is string {
  return typeof value === "string" && /^t-[1-9][0-9]{0,19}$/.test(value);
}

function reviewId(value: unknown): value is string {
  return typeof value === "string" && /^r-[1-9][0-9]{0,19}$/.test(value);
}

function gitPath(value: unknown): value is string {
  if (!bounded(value, MAX_GIT_PATH_BYTES) || !value || value.startsWith("/") ||
      value.includes("\\") || hasControl(value)) return false;
  return value.split("/").every((part) => part !== "" && part !== "." && part !== "..");
}

function parseAnchor(value: unknown): PrAnchor | null {
  const input = record(value);
  if (!input || !exactKeys(input, ["path", "line", "side"]) ||
      !gitPath(input.path) || !prNumber(input.line) ||
      (input.side !== "old" && input.side !== "new")) return null;
  return {
    path: input.path,
    line: input.line,
    side: input.side,
  };
}

export function parseIssueMutation(value: unknown): IssueMutation | null {
  const input = record(value);
  if (!input || typeof input.op !== "string") return null;
  switch (input.op) {
    case "create": {
      if (!exactKeys(input, ["op", "projectId", "title"]) ||
          !canonicalUuid(input.projectId) || !bounded(input.title, MAX_ISSUE_TITLE_BYTES)) {
        return null;
      }
      const title = input.title.trim();
      return !title || hasControl(title)
        ? null
        : { op: "create", projectId: input.projectId, title };
    }
    case "close":
      return exactKeys(input, ["op", "issueId"]) && canonicalUuid(input.issueId)
        ? { op: "close", issueId: input.issueId }
        : null;
    case "activation":
      return exactKeys(input, ["op", "requestEventId"]) && canonicalUlid(input.requestEventId)
        ? { op: "activation", requestEventId: input.requestEventId }
        : null;
    default:
      return null;
  }
}

export function parsePrMutation(value: unknown): PrMutation | null {
  const input = record(value);
  if (!input || typeof input.op !== "string" || !repoSlug(input.repo) || !prNumber(input.n)) {
    return null;
  }
  const base = { repo: input.repo, n: input.n };
  switch (input.op) {
    case "thread": {
      if (!exactKeys(input, ["op", "repo", "n", "body_md", "anchor"])) return null;
      const body_md = markdown(input.body_md, true);
      if (body_md === null) return null;
      if (input.anchor === undefined) return { op: "thread", ...base, body_md };
      const anchor = parseAnchor(input.anchor);
      return anchor ? { op: "thread", ...base, body_md, anchor } : null;
    }
    case "comment": {
      if (!exactKeys(input, ["op", "repo", "n", "threadId", "body_md"]) ||
          !threadId(input.threadId)) return null;
      const body_md = markdown(input.body_md, true);
      return body_md === null ? null : { op: "comment", ...base, threadId: input.threadId, body_md };
    }
    case "review-start":
      return exactKeys(input, ["op", "repo", "n"]) ? { op: "review-start", ...base } : null;
    case "review-comment": {
      if (!exactKeys(input, ["op", "repo", "n", "reviewId", "body_md"]) ||
          !reviewId(input.reviewId)) return null;
      const body_md = markdown(input.body_md, true);
      return body_md === null
        ? null
        : { op: "review-comment", ...base, reviewId: input.reviewId, body_md };
    }
    case "review-submit": {
      if (!exactKeys(input, ["op", "repo", "n", "reviewId", "verdict", "summary_md"]) ||
          !reviewId(input.reviewId) ||
          !["approved", "changes_requested", "commented"].includes(input.verdict as string)) {
        return null;
      }
      const summary = input.summary_md === undefined ? undefined : markdown(input.summary_md, false);
      if (summary === null) return null;
      return {
        op: "review-submit",
        ...base,
        reviewId: input.reviewId,
        verdict: input.verdict as ReviewVerdict,
        ...(summary === undefined ? {} : { summary_md: summary }),
      };
    }
    case "review-discard":
      return exactKeys(input, ["op", "repo", "n", "reviewId"]) && reviewId(input.reviewId)
        ? { op: "review-discard", ...base, reviewId: input.reviewId }
        : null;
    case "merge":
      return exactKeys(input, ["op", "repo", "n"]) ? { op: "merge", ...base } : null;
    default:
      return null;
  }
}
