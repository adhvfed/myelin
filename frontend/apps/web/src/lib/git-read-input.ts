const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export interface GitRepoInput { repo: string }
export interface GitRefsInput extends GitRepoInput {
  limit?: number;
  cursor?: string;
  q?: string;
  current?: string;
}
export interface GitBrowseInput extends GitRepoInput { ref: string; path: string }
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
export interface GitPrCursorInput extends GitPrInput { cursor?: string }
export interface GitPrDiffInput extends GitPrCursorInput { view?: "split" | "unified" }

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

function repo(value: unknown): value is string {
  return bounded(value, 255) && value.length > 0 && value.split("/").every((part) =>
    part !== "" && part !== "." && part !== ".." && /^[A-Za-z0-9._-]+$/.test(part)
  );
}

function refName(value: unknown): value is string {
  return bounded(value, 1_024) && value.length > 0 && !hasControl(value);
}

function path(value: unknown, allowEmpty: boolean): value is string {
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

export function isFullGitRef(value: unknown): value is string {
  if (!bounded(value, 4 * 1024) || hasControl(value)) return false;
  const name = value.startsWith("refs/heads/")
    ? value.slice("refs/heads/".length)
    : value.startsWith("refs/tags/")
      ? value.slice("refs/tags/".length)
      : "";
  const components = name.split("/");
  return name.length > 0 && name !== "@" && !name.endsWith(".") &&
    components.every((component) => component.length > 0 && !component.startsWith(".") &&
      !component.endsWith(".lock")) && !name.includes("..") && !name.includes("@{") &&
    ![" ", "~", "^", ":", "?", "*", "[", "\\"].some((character) => name.includes(character));
}

function prNumber(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > 0;
}

export function parseGitRepoInput(value: unknown): GitRepoInput | null {
  if (typeof value === "string") return repo(value) ? { repo: value } : null;
  const input = record(value);
  return input && exact(input, ["repo"]) && repo(input.repo) ? { repo: input.repo } : null;
}

export function parseGitRefsInput(value: unknown): GitRefsInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "limit", "cursor", "q", "current"]) ||
      !repo(input.repo) || (input.limit !== undefined &&
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
  return input && exact(input, ["repo", "ref", "path"]) && repo(input.repo) &&
    refName(input.ref) && path(input.path, allowEmptyPath)
    ? { repo: input.repo, ref: input.ref, path: input.path }
    : null;
}

export function parseGitCommitsInput(value: unknown): GitCommitsInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "ref", "cursor"]) || !repo(input.repo) ||
      !refName(input.ref) || (input.cursor !== undefined &&
        (!bounded(input.cursor, 4 * 1024) || hasControl(input.cursor)))) return null;
  return {
    repo: input.repo,
    ref: input.ref,
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}

export function parseGitCommitInput(value: unknown): GitCommitInput | null {
  const input = record(value);
  return input && exact(input, ["repo", "oid"]) && repo(input.repo) &&
    typeof input.oid === "string" && /^[0-9a-f]{40}$/.test(input.oid)
    ? { repo: input.repo, oid: input.oid }
    : null;
}

export function parseGitPrInput(value: unknown): GitPrInput | null {
  const input = record(value);
  return input && exact(input, ["repo", "n"]) && repo(input.repo) && prNumber(input.n)
    ? { repo: input.repo, n: input.n }
    : null;
}

export function parseGitRepoPrsInput(value: unknown): GitRepoPrsInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "state", "sort", "cursor"]) || !repo(input.repo) ||
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

export function parseGitPrCursorInput(value: unknown): GitPrCursorInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "n", "cursor"]) || !repo(input.repo) ||
      !prNumber(input.n) || (input.cursor !== undefined && !safeCursor(input.cursor))) return null;
  return {
    repo: input.repo,
    n: input.n,
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
  };
}

export function parseGitPrDiffInput(value: unknown): GitPrDiffInput | null {
  const input = record(value);
  if (!input || !exact(input, ["repo", "n", "cursor", "view"]) || !repo(input.repo) ||
      !prNumber(input.n) || (input.cursor !== undefined && !safeCursor(input.cursor)) ||
      (input.view !== undefined && input.view !== "split" && input.view !== "unified")) return null;
  return {
    repo: input.repo,
    n: input.n,
    ...(input.cursor === undefined ? {} : { cursor: input.cursor }),
    ...(input.view === undefined ? {} : { view: input.view }),
  };
}
