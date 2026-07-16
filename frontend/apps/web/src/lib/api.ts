// The data layer: SolidStart `query`s that call the edge THROUGH the server-side gateway client. The
// `"use server"` directive keeps the gateway + token strictly server-side. On `Unauthorized` (a 401
// that survived the single refresh + retry) the query throws a `/login` redirect — the canon's
// 401→/login behaviour, applied centrally.
import { query, redirect } from "@solidjs/router";
import { edgeGet, GatewayError, Unauthorized } from "../server/gateway";

/** A brief commit projection for the latest-commit bar / per-entry activity (R3.4). */
export interface CommitBriefVM {
  short_oid: string;
  oid?: string;
  summary: string;
  author?: string;
  committed_at: number;
}

/** A tree entry in a populated repo / the tree-at-path view (R3.4). `name` is the basename, `path`
 *  the full repo-relative path (the link target). `latest_commit` is present when the bounded walk
 *  resolved it (absent rows render name-only — the graceful degrade). */
export interface RepoEntry {
  name?: string;
  /** Retained for back-compat; equals `name` at the root. The link uses `path`. */
  path: string;
  is_dir: boolean;
  size?: number;
  latest_commit?: CommitBriefVM;
}

/** The Git RepoHome ViewModel as the edge projects it (populated / empty / restricted). Extended by
 *  R3.4 with default_branch, full README, latest_commit, branch/tag counts, name-carrying entries. */
export interface RepoHomeVM {
  state: "populated" | "empty" | "restricted";
  slug?: string;
  /** Full README markdown (rendered via the read-path / sanitized markdown renderer). */
  readme?: string;
  readme_excerpt?: string;
  clone_url?: string;
  entries?: RepoEntry[];
  default_branch?: string;
  latest_commit?: CommitBriefVM;
  counts?: { branches: number; tags: number };
}

/** One ref row for the switcher. */
export interface RefRow {
  name: string;
  oid: string;
  is_default?: boolean;
}

/** The ref switcher source (GET /v1/git/repos/{repo}/refs). */
export interface RefsVM {
  branches: RefRow[];
  tags: RefRow[];
  default_branch: string;
}

/** The tree-at-path ViewModel (GET /v1/git/repos/{repo}/tree/{ref}/{...path}). `redirect_to_blob`
 *  signals a file requested under tree/ (the client redirects to the blob route — kind mismatch). */
export interface TreeVM {
  ref?: string;
  path?: string;
  entries?: RepoEntry[];
  readme?: string;
  redirect_to_blob?: boolean;
}

/** The MR-014 uniform list envelope `{ items, page }`. */
export interface ReposPage {
  items: RepoHomeVM[];
  page: { next_cursor: string | null; limit: number };
}

/** The single-file view ViewModel. R3.4 adds binary detection, byte size, a truncated head, and the
 *  gateway-proxied raw/download URLs. `redirect_to_tree` signals a directory requested under blob/. */
export interface BlobVM {
  path: string;
  contents: string;
  base_oid: string;
  viewer_may_edit: boolean;
  is_binary?: boolean;
  size_bytes?: number;
  is_truncated?: boolean;
  raw_url?: string;
  download_url?: string;
  redirect_to_tree?: boolean;
}

/** One commit-log row (CommitRow::to_json). `committed_at` is unix seconds (formatted client-side). */
export interface CommitRowVM {
  oid: string;
  short_oid: string;
  summary: string;
  author: string;
  committed_at: number;
  parents: string[];
}

export interface CommitsPage {
  items: CommitRowVM[];
  page: {
    next_cursor: string | null;
    /** R3.4: the "Newer" link (null on the first page — back-button-independent). */
    prev_cursor?: string | null;
    limit: number;
    offset?: number;
    /** R3.4: honest position — range + page, NO fabricated total. */
    range?: { from: number; to: number };
  };
}

/** One unified-diff line: `+` add / `-` remove / ` ` context (the three-channel diff signal). */
export interface DiffLineVM {
  origin: string;
  content: string;
}

/** One changed file in a commit diff (DiffFile::to_json). */
export interface DiffFileVM {
  path: string;
  old_path: string | null;
  status: string;
  lines: DiffLineVM[];
}

/** The commit diff page (CommitDiff::to_json). */
export interface CommitDiffVM {
  oid: string;
  short_oid: string;
  summary: string;
  message: string;
  author: string;
  committed_at: number;
  parents: string[];
  files: DiffFileVM[];
}

/** The durable PR record (DurableGitBackend::pr_json). */
export interface PrVM {
  number: number;
  pr_state: "draft" | "open" | "merged" | "closed";
  base_ref: string;
  head_ref: string;
  head_oid: string;
  author: string;
  reviews: number;
  durable: boolean;
}

/** The PR checks/merge-gate projection (DPrChecks). `gate_admitted` is the AUTHORITATIVE server gate —
 *  the UI reflects it, never recomputes policy. The "why blocked" reasons are display-only. */
export interface PrChecksVM {
  required_contexts: string[];
  required_approvals: number;
  green_contexts: string[];
  endorsed_contexts: string[];
  fork_unendorsed_contexts: string[];
  gate_admitted: boolean;
  durable: boolean;
}

/** The dignified error trio (R-21). `no-access` and `not-found` are calm notes; `error` is a
 *  retryable failure. The route maps this to `<RepoErrorState kind>`. */
export type RepoErrorKind = "no-access" | "not-found" | "error";

/** The message-prefix carrying the kind across the server→client boundary (the class fields don't
 *  survive serialization, but the message string does). `<RepoErrorState>` parses this. */
export const REPO_ERR_PREFIX = "REPO_ERR:";

/** A git-surface route error carrying the mapped `kind` (never the raw edge message — findings 7:
 *  the UI never renders `err.message` as content). */
export class RepoRouteError extends Error {
  readonly kind: RepoErrorKind;
  constructor(kind: RepoErrorKind) {
    super(`${REPO_ERR_PREFIX}${kind}`);
    this.name = "RepoRouteError";
    this.kind = kind;
  }
}

/** Map an edge HTTP status → the dignified error kind. Anti-oracle: policy may make no-access
 *  indistinguishable from not-found (the edge serves the 0-leak 404 on a Pull deny), so a 404 is
 *  `not-found` and a 403 is `no-access`; everything else is the retryable `error`. */
export function mapStatusToKind(status: number): RepoErrorKind {
  if (status === 403) return "no-access";
  if (status === 404) return "not-found";
  return "error";
}

/** Run an edge GET through the gateway: a surviving 401 → the `/login` redirect (unchanged); any
 *  other edge failure → a `RepoRouteError` carrying the mapped kind, so every git route renders the
 *  shared `<RepoErrorState>` instead of leaking `err.message`. */
async function authed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (e) {
    if (e instanceof Unauthorized) throw redirect("/login");
    if (e instanceof GatewayError) throw new RepoRouteError(mapStatusToKind(e.status));
    // A transport/parse failure with no HTTP status is the retryable error kind.
    throw new RepoRouteError("error");
  }
}

/** Encode a path segment for the edge URL (the gateway matches one segment per `{param}`). */
function seg(s: string): string {
  return encodeURIComponent(s);
}

/** The repos screen's data: GET /v1/git/repos through the gateway → the edge ViewModel JSON. */
export const getRepos = query(async (): Promise<ReposPage> => {
  "use server";
  return authed(() => edgeGet<ReposPage>("/v1/git/repos"));
}, "git-repos");

/** A single repo's home (GET /v1/git/repos/{repo}) → the RepoHome ViewModel. */
export const getRepo = query(async (repo: string): Promise<RepoHomeVM> => {
  "use server";
  return authed(() => edgeGet<RepoHomeVM>(`/v1/git/repos/${seg(repo)}`));
}, "git-repo");

/** Encode a NESTED path for a `{...path}` catch-all: encode each segment, keep the `/` separators
 *  (the edge router splits on `/`). An empty path stays empty (the tree/blob root). */
function nestedPath(path: string): string {
  return path
    .split("/")
    .filter((s) => s.length > 0)
    .map(seg)
    .join("/");
}

/** The ref switcher source (GET /v1/git/repos/{repo}/refs). */
export const getRefs = query(async (repo: string): Promise<RefsVM> => {
  "use server";
  return authed(() => edgeGet<RefsVM>(`/v1/git/repos/${seg(repo)}/refs`));
}, "git-refs");

/** A tree at a ref + nested path (GET /v1/git/repos/{repo}/tree/{ref}/{...path}). Root = empty path. */
export const getTree = query(
  async (input: { repo: string; ref: string; path: string }): Promise<TreeVM> => {
    "use server";
    const tail = input.path ? `/${nestedPath(input.path)}` : "";
    return authed(() =>
      edgeGet<TreeVM>(`/v1/git/repos/${seg(input.repo)}/tree/${seg(input.ref)}${tail}`),
    );
  },
  "git-tree",
);

/** A single file at a ref + NESTED path (GET /v1/git/repos/{repo}/blob/{ref}/{...path}). */
export const getBlob = query(
  async (input: { repo: string; ref: string; path: string }): Promise<BlobVM> => {
    "use server";
    return authed(() =>
      edgeGet<BlobVM>(
        `/v1/git/repos/${seg(input.repo)}/blob/${seg(input.ref)}/${nestedPath(input.path)}`,
      ),
    );
  },
  "git-blob",
);

/** The commit log for a ref (GET /v1/git/repos/{repo}/commits/{ref}). */
export const getCommits = query(
  async (input: { repo: string; ref: string; cursor?: string }): Promise<CommitsPage> => {
    "use server";
    const q = input.cursor ? `?cursor=${seg(input.cursor)}` : "";
    return authed(() =>
      edgeGet<CommitsPage>(`/v1/git/repos/${seg(input.repo)}/commits/${seg(input.ref)}${q}`),
    );
  },
  "git-commits",
);

/** A single commit's diff (GET /v1/git/repos/{repo}/commit/{oid}). */
export const getCommit = query(
  async (input: { repo: string; oid: string }): Promise<CommitDiffVM> => {
    "use server";
    return authed(() =>
      edgeGet<CommitDiffVM>(`/v1/git/repos/${seg(input.repo)}/commit/${seg(input.oid)}`),
    );
  },
  "git-commit",
);

/** A PR record (GET /v1/git/repos/{repo}/prs/{n}). */
export const getPr = query(
  async (input: { repo: string; n: number }): Promise<PrVM> => {
    "use server";
    return authed(() => edgeGet<PrVM>(`/v1/git/repos/${seg(input.repo)}/prs/${input.n}`));
  },
  "git-pr",
);

/** A PR's checks + merge-gate projection (GET /v1/git/repos/{repo}/prs/{n}/checks). */
export const getPrChecks = query(
  async (input: { repo: string; n: number }): Promise<PrChecksVM> => {
    "use server";
    return authed(() =>
      edgeGet<PrChecksVM>(`/v1/git/repos/${seg(input.repo)}/prs/${input.n}/checks`),
    );
  },
  "git-pr-checks",
);
