import type {
  BlobVM,
  CommitBriefVM,
  RefRow,
  RefsVM,
  RepoEntry,
  RepoHomeVM,
  ReposPage,
  TreeVM,
} from "./api";

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

function refRow(value: unknown): RefRow | null {
  const row = record(value);
  if (!row || !displayText(row.name, 1_024) || !row.name || !gitOid(row.oid) ||
      (row.is_default !== undefined && typeof row.is_default !== "boolean")) return null;
  return {
    name: row.name,
    oid: row.oid,
    ...(row.is_default === undefined ? {} : { is_default: row.is_default }),
  };
}

export function parseRefs(value: unknown): RefsVM | null {
  const refs = record(value);
  if (!refs || !Array.isArray(refs.branches) || !Array.isArray(refs.tags) ||
      refs.branches.length > 1_000 || refs.tags.length > 1_000 ||
      !displayText(refs.default_branch, 1_024) || !refs.default_branch) return null;
  const branches = refs.branches.map(refRow);
  const tags = refs.tags.map(refRow);
  return branches.every((row): row is RefRow => row !== null) &&
    tags.every((row): row is RefRow => row !== null)
    ? { branches, tags, default_branch: refs.default_branch }
    : null;
}

export function parseTree(value: unknown): TreeVM | null {
  const tree = record(value);
  if (!tree || (tree.ref !== undefined && (!displayText(tree.ref, 1_024) || !tree.ref)) ||
      (tree.path !== undefined && tree.path !== "" && !repoPath(tree.path)) ||
      (tree.redirect_to_blob !== undefined && typeof tree.redirect_to_blob !== "boolean") ||
      (tree.readme !== undefined && tree.readme !== null && !bounded(tree.readme, MAX_TEXT_BYTES))) {
    return null;
  }
  let parsedEntries: RepoEntry[] | undefined;
  if (tree.entries !== undefined) {
    if (!Array.isArray(tree.entries) || tree.entries.length > MAX_REPO_ENTRIES) return null;
    const candidate = tree.entries.map(repoEntry);
    if (!candidate.every((entry): entry is RepoEntry => entry !== null)) return null;
    parsedEntries = candidate;
  }
  return {
    ...(tree.ref === undefined ? {} : { ref: tree.ref }),
    ...(tree.path === undefined ? {} : { path: tree.path }),
    ...(parsedEntries === undefined ? {} : { entries: parsedEntries }),
    ...(typeof tree.readme === "string" ? { readme: tree.readme } : {}),
    ...(tree.redirect_to_blob === undefined ? {} : { redirect_to_blob: tree.redirect_to_blob }),
  };
}

export function parseBlob(value: unknown): BlobVM | null {
  const blob = record(value);
  if (!blob || !repoPath(blob.path) ||
      (blob.redirect_to_tree !== undefined && typeof blob.redirect_to_tree !== "boolean")) return null;
  if (blob.redirect_to_tree === true) {
    return { path: blob.path, contents: "", base_oid: "", viewer_may_edit: false, redirect_to_tree: true };
  }
  if (!bounded(blob.contents, MAX_TEXT_BYTES) || !displayText(blob.base_oid, 256) ||
      typeof blob.viewer_may_edit !== "boolean" ||
      (blob.is_binary !== undefined && typeof blob.is_binary !== "boolean") ||
      (blob.size_bytes !== undefined && !uint(blob.size_bytes)) ||
      (blob.is_truncated !== undefined && typeof blob.is_truncated !== "boolean")) return null;
  const relative = (candidate: unknown) => candidate === undefined ||
    (displayText(candidate, 8 * 1024) && candidate.startsWith("/") && !candidate.startsWith("//"));
  if (!relative(blob.raw_url) || !relative(blob.download_url)) return null;
  return {
    path: blob.path,
    contents: blob.contents,
    base_oid: blob.base_oid,
    viewer_may_edit: blob.viewer_may_edit,
    ...(blob.is_binary === undefined ? {} : { is_binary: blob.is_binary }),
    ...(blob.size_bytes === undefined ? {} : { size_bytes: blob.size_bytes }),
    ...(blob.is_truncated === undefined ? {} : { is_truncated: blob.is_truncated }),
    ...(blob.raw_url === undefined ? {} : { raw_url: blob.raw_url as string }),
    ...(blob.download_url === undefined ? {} : { download_url: blob.download_url as string }),
  };
}
