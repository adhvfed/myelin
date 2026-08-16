import type {
  BlobVM,
  BranchRefRow,
  CommitBriefVM,
  PinnedRefRow,
  RefRow,
  RefsVM,
  RepoEntry,
  RepoHomeVM,
  PopulatedRepoHomeVM,
  TreeVM,
} from "./api";
import { isFullGitRef } from "./git-read-input";
import { parseArtifactRef } from "./artifact-ref";
import { isGitRepositorySlug } from "./git-coordinate";

const MAX_TEXT_BYTES = 512 * 1024;
const MAX_PATH_BYTES = 4 * 1024;
const MAX_REPO_ENTRIES = 1_000;
const MAX_REF_BYTES = 4 * 1024;
const MAX_REF_CURSOR_BYTES = 8 * 1024;
const MAX_TREE_CURSOR_BYTES = 8 * 1024;
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

function treeCursor(value: unknown): value is string {
  return displayText(value, MAX_TREE_CURSOR_BYTES) && /^gt1_[A-Za-z0-9_-]+$/.test(value);
}

function treePage(value: unknown): { next_cursor: string | null; limit: number } | null {
  const page = record(value);
  if (!page || (page.next_cursor !== null && !treeCursor(page.next_cursor)) ||
      !Number.isSafeInteger(page.limit) || (page.limit as number) < 1 ||
      (page.limit as number) > 100) return null;
  return { next_cursor: page.next_cursor as string | null, limit: page.limit as number };
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
  const reference = parseArtifactRef(home.ref);
  if (!isGitRepositorySlug(home.slug) || !displayText(home.default_branch, 1_024) ||
      (home.clone_url !== undefined && !displayText(home.clone_url, 4 * 1024)) ||
      !reference || reference.sub !== null || reference.subsystem !== "git" ||
      reference.type !== "repo") return null;
  const separator = home.slug.indexOf("/");
  if (separator < 1 || reference.tenant !== home.slug.slice(0, separator) ||
      reference.id !== home.slug.slice(separator + 1)) return null;
  let counts: { branches: number; tags: number } | undefined;
  if (home.counts !== undefined) {
    const wireCounts = record(home.counts);
    if (!wireCounts || !uint(wireCounts.branches) || !uint(wireCounts.tags)) return null;
    counts = { branches: wireCounts.branches, tags: wireCounts.tags };
  }
  const visible = {
    slug: home.slug,
    ref: reference.root,
    default_branch: home.default_branch,
    ...(home.clone_url === undefined ? {} : { clone_url: home.clone_url }),
    ...(counts === undefined ? {} : { counts }),
  };
  if (state === "empty") return { state, ...visible };

  const result: PopulatedRepoHomeVM = { state, ...visible };
  const modernPagination = home.snapshot_oid !== undefined || home.entries_page !== undefined;
  if (modernPagination && (home.snapshot_oid === undefined || home.entries_page === undefined)) {
    return null;
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
  if (home.snapshot_oid !== undefined) {
    if (!gitOid(home.snapshot_oid)) return null;
    result.snapshot_oid = home.snapshot_oid;
  }
  if (home.entries_page !== undefined) {
    const entriesPage = record(home.entries_page);
    const page = treePage(entriesPage);
    if (!entriesPage || !page || !isFullGitRef(entriesPage.ref) ||
        !gitOid(entriesPage.snapshot_oid) ||
        (result.entries !== undefined && result.entries.length > page.limit) ||
        (result.snapshot_oid !== undefined && result.snapshot_oid !== entriesPage.snapshot_oid)) {
      return null;
    }
    result.entries_page = {
      ref: entriesPage.ref,
      snapshot_oid: entriesPage.snapshot_oid,
      ...page,
    };
  }
  if (home.latest_commit !== undefined) {
    const latest = commitBrief(home.latest_commit);
    if (!latest) return null;
    result.latest_commit = latest;
  }
  return result;
}

function refText(value: unknown, maximum = MAX_REF_BYTES): value is string {
  return bounded(value, maximum) && value.length > 0 && ![...value].some((character) => {
    const point = character.codePointAt(0)!;
    return point <= 0x1f || point === 0x7f;
  });
}

function branchRefRow(value: unknown): BranchRefRow | null {
  const row = record(value);
  if (!row || !refText(row.name) || !gitOid(row.oid) ||
      typeof row.is_default !== "boolean") return null;
  return {
    name: row.name,
    oid: row.oid,
    is_default: row.is_default,
  };
}

function tagRefRow(value: unknown): RefRow | null {
  const row = record(value);
  return row && refText(row.name) && gitOid(row.oid)
    ? { name: row.name, oid: row.oid }
    : null;
}

function pinnedRef(value: unknown, defaultBranch: string): PinnedRefRow | null {
  const row = record(value);
  if (!row || (row.kind !== "branch" && row.kind !== "tag") || !refText(row.full_name) ||
      !refText(row.name) || !gitOid(row.oid) || typeof row.is_default !== "boolean") return null;
  const prefix = row.kind === "branch" ? "refs/heads/" : "refs/tags/";
  const isDefault = row.kind === "branch" && row.name === defaultBranch;
  if (row.full_name !== `${prefix}${row.name}` || row.is_default !== isDefault) return null;
  return {
    kind: row.kind,
    full_name: row.full_name,
    name: row.name,
    oid: row.oid,
    is_default: row.is_default,
  };
}

function refCursor(value: unknown): value is string {
  return refText(value, MAX_REF_CURSOR_BYTES) && /^gr1_[A-Za-z0-9_-]+$/.test(value);
}

export function parseRefs(value: unknown): RefsVM | null {
  const refs = record(value);
  if (!refs || !Array.isArray(refs.branches) || !Array.isArray(refs.tags) ||
      !refText(refs.default_branch) || !Array.isArray(refs.pinned)) return null;
  const page = record(refs.page);
  const rowCount = refs.branches.length + refs.tags.length;
  const limit = page?.limit;
  if (!page || !Number.isSafeInteger(limit) || (limit as number) < 1 ||
      (limit as number) > 100 || rowCount > (limit as number) ||
      (page.next_cursor !== null && !refCursor(page.next_cursor))) {
    return null;
  }
  const branches = refs.branches.map(branchRefRow);
  const tags = refs.tags.map(tagRefRow);
  if (!branches.every((row): row is BranchRefRow => row !== null) ||
      !tags.every((row): row is RefRow => row !== null) ||
      branches.some((row) => row.is_default && row.name !== refs.default_branch) ||
      branches.some((row) => row.is_default !== (row.name === refs.default_branch)) ||
      branches.filter((row) => row.is_default).length > 1) return null;
  if (refs.pinned.length > 2) return null;
  const pins = refs.pinned.map((row) => pinnedRef(row, refs.default_branch as string));
  if (!pins.every((row): row is PinnedRefRow => row !== null) ||
      new Set(pins.map((row) => row.full_name)).size !== pins.length) return null;
  return {
    branches,
    tags,
    default_branch: refs.default_branch,
    pinned: pins,
    page: {
      next_cursor: page.next_cursor as string | null,
      limit: limit as number,
    },
  };
}

export function parseTree(value: unknown): TreeVM | null {
  const tree = record(value);
  if (!tree || (tree.ref !== undefined && (!displayText(tree.ref, 1_024) || !tree.ref)) ||
      (tree.path !== undefined && tree.path !== "" && !repoPath(tree.path)) ||
      (tree.redirect_to_blob !== undefined && typeof tree.redirect_to_blob !== "boolean") ||
      (tree.readme !== undefined && tree.readme !== null && !bounded(tree.readme, MAX_TEXT_BYTES))) {
    return null;
  }
  if (tree.redirect_to_blob === true) {
    return {
      ...(tree.ref === undefined ? {} : { ref: tree.ref as string }),
      ...(tree.path === undefined ? {} : { path: tree.path as string }),
      redirect_to_blob: true,
    };
  }
  const modern = tree.page !== undefined || tree.snapshot_oid !== undefined;
  const page = modern ? treePage(tree.page) : null;
  if (modern && (!page || !gitOid(tree.snapshot_oid) || !displayText(tree.ref, 1_024) ||
      !tree.ref || (tree.path !== "" && !repoPath(tree.path)) || !Array.isArray(tree.entries))) {
    return null;
  }
  let parsedEntries: RepoEntry[] | undefined;
  if (tree.entries !== undefined) {
    const maximum = modern ? page!.limit : MAX_REPO_ENTRIES;
    if (!Array.isArray(tree.entries) || tree.entries.length > maximum) return null;
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
    ...(modern ? { snapshot_oid: tree.snapshot_oid as string, page: page! } : {}),
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
      (blob.is_truncated !== undefined && typeof blob.is_truncated !== "boolean") ||
      (blob.preview_unavailable !== undefined && typeof blob.preview_unavailable !== "boolean") ||
      (blob.download_available !== undefined && typeof blob.download_available !== "boolean") ||
      (blob.preview_unavailable === true && blob.contents !== "")) return null;
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
    ...(blob.preview_unavailable === undefined ? {} : { preview_unavailable: blob.preview_unavailable }),
    ...(blob.download_available === undefined ? {} : { download_available: blob.download_available }),
    ...(blob.raw_url === undefined ? {} : { raw_url: blob.raw_url as string }),
    ...(blob.download_url === undefined ? {} : { download_url: blob.download_url as string }),
  };
}
