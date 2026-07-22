import type { CommitBriefVM, RepoEntry, RepoHomeVM, ReposPage } from "./api";

const MAX_TEXT_BYTES = 512 * 1024;
const MAX_PATH_BYTES = 4 * 1024;
const MAX_REPO_ENTRIES = 1_000;
const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

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

function repoSlug(value: unknown): value is string {
  return bounded(value, 255) && value.length > 0 && value.split("/").every((part) =>
    part !== "" && part !== "." && part !== ".." && /^[A-Za-z0-9._-]+$/.test(part)
  );
}

function repoPath(value: unknown): value is string {
  return displayText(value, MAX_PATH_BYTES) && value.length > 0 && !value.startsWith("/") &&
    !value.includes("\\") && value.split("/").every((part) =>
      part !== "" && part !== "." && part !== ".."
    );
}

function commitBrief(value: unknown): CommitBriefVM | null {
  const commit = record(value);
  if (!commit || typeof commit.short_oid !== "string" ||
      !/^[0-9a-f]{7,40}$/.test(commit.short_oid) || !displayText(commit.summary, 8 * 1024) ||
      !uint(commit.committed_at) || (commit.oid !== undefined && !gitOid(commit.oid)) ||
      (commit.author !== undefined && !displayText(commit.author, 1_024))) return null;
  return {
    short_oid: commit.short_oid,
    summary: commit.summary,
    committed_at: commit.committed_at,
    ...(commit.oid === undefined ? {} : { oid: commit.oid }),
    ...(commit.author === undefined ? {} : { author: commit.author }),
  };
}

function repoEntry(value: unknown): RepoEntry | null {
  const entry = record(value);
  if (!entry || !repoPath(entry.path) || typeof entry.is_dir !== "boolean" ||
      (entry.name !== undefined && !displayText(entry.name, 1_024)) ||
      (entry.size !== undefined && !uint(entry.size))) return null;
  const latest = entry.latest_commit === undefined ? undefined : commitBrief(entry.latest_commit);
  if (entry.latest_commit !== undefined && !latest) return null;
  return {
    path: entry.path,
    is_dir: entry.is_dir,
    ...(entry.name === undefined ? {} : { name: entry.name }),
    ...(entry.size === undefined ? {} : { size: entry.size }),
    ...(latest ? { latest_commit: latest } : {}),
  };
}

export function parseRepoHome(value: unknown): RepoHomeVM | null {
  const home = record(value);
  if (!home) return null;
  const state = home.state;
  if (state !== "populated" && state !== "empty" && state !== "restricted") return null;
  if (state === "restricted") return { state: "restricted" };
  if (!repoSlug(home.slug) || !displayText(home.default_branch, 1_024) ||
      (home.clone_url !== undefined && !displayText(home.clone_url, 4 * 1024))) return null;
  const result: RepoHomeVM = {
    state,
    slug: home.slug,
    default_branch: home.default_branch,
    ...(home.clone_url === undefined ? {} : { clone_url: home.clone_url }),
  };
  if (home.counts !== undefined) {
    const counts = record(home.counts);
    if (!counts || !uint(counts.branches) || !uint(counts.tags)) return null;
    result.counts = { branches: counts.branches, tags: counts.tags };
  }
  for (const key of ["readme", "readme_excerpt"] as const) {
    if (home[key] !== undefined) {
      if (!bounded(home[key], MAX_TEXT_BYTES)) return null;
      result[key] = home[key];
    }
  }
  if (home.entries !== undefined) {
    if (!Array.isArray(home.entries) || home.entries.length > MAX_REPO_ENTRIES) return null;
    const entries = home.entries.map(repoEntry);
    if (!entries.every((entry): entry is RepoEntry => entry !== null)) return null;
    result.entries = entries;
  }
  if (home.latest_commit !== undefined) {
    const latest = commitBrief(home.latest_commit);
    if (!latest) return null;
    result.latest_commit = latest;
  }
  return result;
}

export function parseReposPage(value: unknown): ReposPage | null {
  const envelope = record(value);
  const page = record(envelope?.page);
  if (!envelope || !Array.isArray(envelope.items) || envelope.items.length > 100 || !page ||
      (page.next_cursor !== null && !displayText(page.next_cursor, 4 * 1024)) ||
      !Number.isSafeInteger(page.limit) || (page.limit as number) < 1 || (page.limit as number) > 100) {
    return null;
  }
  const items = envelope.items.map(parseRepoHome);
  return items.every((item): item is RepoHomeVM => item !== null)
    ? { items, page: { next_cursor: page.next_cursor, limit: page.limit as number } }
    : null;
}
