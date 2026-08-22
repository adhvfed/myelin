import { isFullGitRef } from "./git-ref";
import { isGitPullRequestNumber, isGitRepositorySlug } from "./git-coordinate";

export { isFullGitRef } from "./git-ref";

const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export interface GitRepoInput { repo: string }
export interface GitRepoListInput {
  limit?: number;
  cursor?: string;
}
export interface GitRefsInput extends GitRepoInput {
  limit?: number;
  cursor?: string;
  q?: string;
  current?: string;
}
export interface GitBrowseInput extends GitRepoInput { ref: string; path: string }
export interface GitTreeInput extends GitBrowseInput {
  limit?: number;
  cursor?: string;
  q?: string;
}
export interface GitCommitsInput extends GitRepoInput { ref: string; cursor?: string }
export interface GitCommitInput extends GitRepoInput { oid: string }
export interface GitPrInput extends GitRepoInput { n: number }
export interface GitRepoPrsInput extends GitRepoInput {
  state?: "open" | "merged" | "closed" | "all";
  sort?: "updated" | "created";
  cursor?: string;
}
export interface GitMyPrsInput {
  bucket: "needs-review" | "yours";
  cursor?: string;
}
export interface GitPrCommitsInput extends GitPrInput {
  limit?: number;
  cursor?: string;
}
export interface GitPrDiffInput extends GitPrInput { cursor?: string }

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).every((key) => allowed.has(key));
}

function bounded(value: unknown, maximum: number): value is string {
  return typeof value === "string" && utf8.encode(value).byteLength <= maximum;
}

function hasControl(value: string): boolean {
  return [...value].some((character) => {
    const point = character.codePointAt(0)!;
    return point <= 0x1f || point === 0x7f;
  });
}

export function isGitRefName(value: unknown): value is string {
  return bounded(value, 1_024) && value.length > 0 && !hasControl(value);
}

export function isGitPath(value: unknown, allowEmpty = false): value is string {
  if (!bounded(value, 4 * 1024) || (!allowEmpty && !value) || value.startsWith("/") ||
      value.includes("\\") || hasControl(value)) return false;
  return value === "" || value.split("/").every((part) =>
    part !== "" && part !== "." && part !== ".."
  );
}

function safeCursor(value: unknown): value is string {
  return bounded(value, 4 * 1024) && !hasControl(value);
}

function refsCursor(value: unknown): value is string {
  return bounded(value, 8 * 1024) && /^gr1_[A-Za-z0-9_-]+$/.test(value) && !hasControl(value);
}

function treeCursor(value: unknown): value is string {
  return bounded(value, 8 * 1024) && /^gt1_[A-Za-z0-9_-]+$/.test(value) && !hasControl(value);
}

function cursorFrame(value: unknown, prefix: string, maximum: number): Uint8Array | null {
  if (!bounded(value, maximum) || !value.startsWith(prefix)) return null;
  const encoded = value.slice(prefix.length);
  if (!encoded || !/^[A-Za-z0-9_-]+$/.test(encoded) || encoded.length % 4 === 1) return null;
  try {
    const padded = encoded.replace(/-/g, "+").replace(/_/g, "/") +
      "=".repeat((4 - (encoded.length % 4)) % 4);
    const decoded = atob(padded);
    const canonical = btoa(decoded).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    return canonical === encoded
      ? Uint8Array.from(decoded, (byte) => byte.charCodeAt(0))
      : null;
  } catch {
    return null;
  }
}

/** An opaque canonical repository-list cursor (`rl2_`, unpadded base64url, at most 512 bytes). */
export function isRepoListCursor(value: unknown): value is string {
  return cursorFrame(value, "rl2_", 512) !== null;
}

/** A canonical PR-commit cursor (`pc1_` + the Edge's exact 78-byte v1 frame). */
export function isPrCommitCursor(value: unknown): value is string {
  const frame = cursorFrame(value, "pc1_", 256);
  if (!frame || frame.length !== 78 || frame[0] !== 1) return false;
  try {
    const baseKind = frame[33]!;
    if (baseKind !== 0 && baseKind !== 1) return false;
    if (baseKind === 0 && frame.slice(34, 54).some((byte) => byte !== 0)) {
      return false;
    }
    const position = new DataView(frame.buffer, frame.byteOffset, frame.byteLength)
      .getUint32(74, false);
    return position >= 1 && position <= 100_000;
  } catch {
    return false;
  }
}

export function parseGitRepoInput(value: unknown): GitRepoInput | null {
  if (typeof value === "string") return isGitRepositorySlug(value) ? { repo: value } : null;
  const input = record(value);
  return input && exact(input, ["repo"]) && isGitRepositorySlug(input.repo)
    ? { repo: input.repo }
    : null;
}

export function parseGitRepoListInput(value: unknown): GitRepoListInput | null {
  const input = record(value);
  if (!input || !exact(input, ["limit", "cursor"]) ||
      (input.limit !== undefined && (!Number.isSafeInteger(input.limit) ||
        (input.limit as number) < 1 || (input.limit as number) > 100)) ||
      (input.cursor !== undefined && !isRepoListCursor(input.cursor))) return null;
  return {
    ...(input.limit === undefined ? {} : { limit: input.limit as number }),
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}

export function gitRepoListSearchParams(input: GitRepoListInput): URLSearchParams {
  const params = new URLSearchParams();
  if (input.limit !== undefined) params.set("limit", String(input.limit));
  if (input.cursor !== undefined) params.set("cursor", input.cursor);
  return params;
}

export function parseGitRefsInput(value: unknown): GitRefsInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "limit", "cursor", "q", "current"]) ||
      !isGitRepositorySlug(input.repo) || (input.limit !== undefined &&
        (!Number.isSafeInteger(input.limit) || (input.limit as number) < 1 ||
          (input.limit as number) > 100)) ||
      (input.cursor !== undefined && !refsCursor(input.cursor)) ||
      (input.q !== undefined && (!bounded(input.q, 256) || hasControl(input.q))) ||
      (input.current !== undefined && !isFullGitRef(input.current))) return null;
  return {
    repo: input.repo,
    ...(input.limit === undefined ? {} : { limit: input.limit as number }),
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
    ...(input.q === undefined ? {} : { q: input.q }),
    ...(input.current === undefined ? {} : { current: input.current }),
  };
}

export function gitRefsSearchParams(input: GitRefsInput): URLSearchParams {
  const params = new URLSearchParams();
  if (input.limit !== undefined) params.set("limit", String(input.limit));
  if (input.cursor !== undefined) params.set("cursor", input.cursor);
  if (input.q !== undefined) params.set("q", input.q);
  if (input.current !== undefined) params.set("current", input.current);
  return params;
}

export function parseGitBrowseInput(value: unknown, allowEmptyPath: boolean): GitBrowseInput | null {
  const input = record(value);
  return input && exact(input, ["repo", "ref", "path"]) && isGitRepositorySlug(input.repo) &&
    isGitRefName(input.ref) && isGitPath(input.path, allowEmptyPath)
    ? { repo: input.repo, ref: input.ref, path: input.path }
    : null;
}

export function parseGitTreeInput(value: unknown): GitTreeInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "ref", "path", "limit", "cursor", "q"]) ||
      !isGitRepositorySlug(input.repo) || !isGitRefName(input.ref) || !isGitPath(input.path, true) ||
      (input.limit !== undefined && (!Number.isSafeInteger(input.limit) ||
        (input.limit as number) < 1 || (input.limit as number) > 100)) ||
      (input.cursor !== undefined && !treeCursor(input.cursor)) ||
      (input.q !== undefined && (!bounded(input.q, 256) || hasControl(input.q)))) return null;
  return {
    repo: input.repo,
    ref: input.ref,
    path: input.path,
    ...(input.limit === undefined ? {} : { limit: input.limit as number }),
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
    ...(input.q === undefined ? {} : { q: input.q }),
  };
}

export function gitTreeSearchParams(input: GitTreeInput): URLSearchParams {
  const params = new URLSearchParams();
  if (input.limit !== undefined) params.set("limit", String(input.limit));
  if (input.cursor !== undefined) params.set("cursor", input.cursor);
  if (input.q !== undefined) params.set("q", input.q);
  return params;
}

export function parseGitCommitsInput(value: unknown): GitCommitsInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "ref", "cursor"]) || !isGitRepositorySlug(input.repo) ||
      !isGitRefName(input.ref) || (input.cursor !== undefined &&
        (!bounded(input.cursor, 4 * 1024) || hasControl(input.cursor)))) return null;
  return {
    repo: input.repo,
    ref: input.ref,
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}

export function parseGitCommitInput(value: unknown): GitCommitInput | null {
  const input = record(value);
  return input && exact(input, ["repo", "oid"]) && isGitRepositorySlug(input.repo) &&
    typeof input.oid === "string" && /^[0-9a-f]{40}$/.test(input.oid)
    ? { repo: input.repo, oid: input.oid }
    : null;
}

export function parseGitPrInput(value: unknown): GitPrInput | null {
  const input = record(value);
  return input && exact(input, ["repo", "n"]) && isGitRepositorySlug(input.repo) &&
      isGitPullRequestNumber(input.n)
    ? { repo: input.repo, n: input.n }
    : null;
}

export function parseGitRepoPrsInput(value: unknown): GitRepoPrsInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "state", "sort", "cursor"]) ||
      !isGitRepositorySlug(input.repo) ||
      (input.state !== undefined && !["open", "merged", "closed", "all"].includes(input.state as string)) ||
      (input.sort !== undefined && input.sort !== "updated" && input.sort !== "created") ||
      (input.cursor !== undefined && !safeCursor(input.cursor))) return null;
  return {
    repo: input.repo,
    ...(input.state === undefined ? {} : { state: input.state as GitRepoPrsInput["state"] }),
    ...(input.sort === undefined ? {} : { sort: input.sort as GitRepoPrsInput["sort"] }),
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}

export function parseGitMyPrsInput(value: unknown): GitMyPrsInput | null {
  const input = record(value);
  if (!input || !exact(input, ["bucket", "cursor"]) ||
      (input.bucket !== "needs-review" && input.bucket !== "yours") ||
      (input.cursor !== undefined && !safeCursor(input.cursor))) return null;
  return {
    bucket: input.bucket,
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}

export function parseGitPrCommitsInput(value: unknown): GitPrCommitsInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "n", "limit", "cursor"]) ||
      !isGitRepositorySlug(input.repo) ||
      !isGitPullRequestNumber(input.n) || (input.limit !== undefined &&
        (!Number.isSafeInteger(input.limit) ||
        (input.limit as number) < 1 || (input.limit as number) > 100)) ||
      (input.cursor !== undefined && !isPrCommitCursor(input.cursor))) return null;
  return {
    repo: input.repo,
    n: input.n,
    ...(input.limit === undefined ? {} : { limit: input.limit as number }),
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}

export function gitPrCommitsSearchParams(input: GitPrCommitsInput): URLSearchParams {
  const params = new URLSearchParams();
  if (input.limit !== undefined) params.set("limit", String(input.limit));
  if (input.cursor !== undefined) params.set("cursor", input.cursor);
  return params;
}

export function gitPrCommitsPath(input: GitPrCommitsInput): string {
  const query = gitPrCommitsSearchParams(input).toString();
  const path = `/v1/git/repos/${encodeURIComponent(input.repo)}/prs/${input.n}/commits`;
  return query ? `${path}?${query}` : path;
}

export function parseGitPrDiffInput(value: unknown): GitPrDiffInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "n", "cursor"]) ||
      !isGitRepositorySlug(input.repo) ||
      !isGitPullRequestNumber(input.n) ||
      (input.cursor !== undefined && !safeCursor(input.cursor))) return null;
  return {
    repo: input.repo,
    n: input.n,
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}
