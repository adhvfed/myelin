const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export interface GitRepoInput { repo: string }
export interface GitBrowseInput extends GitRepoInput { ref: string; path: string }
export interface GitCommitsInput extends GitRepoInput { ref: string; cursor?: string }
export interface GitCommitInput extends GitRepoInput { oid: string }

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

export function parseGitRepoInput(value: unknown): GitRepoInput | null {
  if (typeof value === "string") return repo(value) ? { repo: value } : null;
  const input = record(value);
  return input && exact(input, ["repo"]) && repo(input.repo) ? { repo: input.repo } : null;
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
