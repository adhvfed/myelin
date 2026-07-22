// The data layer: SolidStart `query`s that call the edge THROUGH the server-side gateway client. The
// `"use server"` directive keeps the gateway + token strictly server-side. On `Unauthorized` (a 401
// that survived the single refresh + retry) the query throws a `/login` redirect — the canon's
// 401→/login behaviour, applied centrally.
import { action, json, query, redirect } from "@solidjs/router";
import { edgeGet, edgePost, GatewayError, Unauthorized } from "../server/gateway";
import {
  parseIssueMutation,
  parsePrMutation,
  type IssueMutation,
  type PrMutation,
} from "./mutation-input";
import {
  hasAppliedAction,
  parseAppliedComment,
  parseAppliedMerge,
  parseAppliedReview,
  parseAppliedThread,
  parseIssue,
  parseIssueAuthorizationStatus,
  parseIssueCreateReceipt,
  parsePrChecks,
} from "./mutation-response";

export type { IssueMutation, PrMutation } from "./mutation-input";

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

/** One unified-diff line: `+` add / `-` remove / ` ` context (the three-channel diff signal).
 *  `old_no`/`new_no` are the additive R3.2 line-number fields (null on `+`/`-` respectively; absent
 *  on the legacy commit-diff shape — the DiffViewer tolerates their absence). */
export interface DiffLineVM {
  origin: string;
  content: string;
  old_no?: number | null;
  new_no?: number | null;
}

/** One hunk of a PR diff — the `@@` header + boundaries + lines (collapsed-run + expand-context need
 *  the boundaries a flat `lines[]` can't carry). */
export interface DiffHunkVM {
  header: string;
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  lines: DiffLineVM[];
}

/** One changed file in a PR diff (PrDiffFile::to_json). A RESTRICTED file is NEVER in this list — the
 *  count-only disclosure lives on `PrDiffVM.restricted_files` (non-leak by construction). */
export interface PrDiffFileVM {
  path: string;
  old_path: string | null;
  status: string;
  kind: "text" | "binary" | "lfs" | "submodule";
  additions: number;
  deletions: number;
  size_bytes: number | null;
  hunks: DiffHunkVM[];
  deleted_body_available: boolean;
  truncated: boolean;
}

/** The PR three-dot diff page (PrDiffVM::to_json) — `merge-base(base, head) … head`. `three_dot`
 *  false labels the two-dot floor; `restricted_files` is COUNT-ONLY (no paths cross the wire). */
export interface PrDiffVM {
  number: number;
  base_ref: string;
  base_oid: string;
  short_base_oid: string;
  head_oid: string;
  short_head_oid: string;
  three_dot: boolean;
  files: PrDiffFileVM[];
  restricted_files: number;
  total_files: number;
  total_additions: number;
  total_deletions: number;
  page: { next_cursor: string | null; limit: number };
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

/** The durable PR record (DurableGitBackend::pr_json). R3.3 adds the overview header fields:
 *  `title`/`body_md` (the description via the ONE BlockEditor read path), `created_at` (the "opened …"
 *  date, Intl-formatted client-side; null on a legacy record), `commits_count` (the tab badge; null
 *  when the walk couldn't read; `commits_count_capped` = the count is `500+`). */
export interface PrVM {
  number: number;
  pr_state: "draft" | "open" | "merged" | "closed";
  title: string | null;
  body_md: string | null;
  base_ref: string;
  head_ref: string;
  head_oid: string;
  author: string;
  author_is_agent?: boolean;
  reviews: number;
  created_at: number | null;
  updated_at?: number | null;
  commits_count?: number | null;
  commits_count_capped?: boolean;
  durable: boolean;
}

/** The identity/agent badge atom (PrincipalVM). `display` arrives pre-collapsed ("[erased user]" /
 *  "Restricted"); `on_behalf_of`/`trigger` are the agent attribution channels. */
export interface PrincipalVM {
  kind: "human" | "agent" | "service";
  display: string;
  on_behalf_of: string | null;
  trigger: string | null;
}

/** One diff-line content anchor on a thread/comment (null = a PR-level discussion thread). */
export interface PrAnchorVM {
  path: string;
  line: number | null;
  side: "old" | "new" | null;
  base_oid: string | null;
  head_oid: string | null;
  anchor_state: "live" | "moved" | "outdated";
}

/** One comment (PrCommentVM). `body_md` is null for a removed comment ("Comment removed", tree kept).
 *  `pending` is true ONLY in the author's own view of an un-submitted review batch. */
export interface PrCommentVM {
  id: string;
  author: PrincipalVM;
  body_md: string | null;
  created_at: number;
  edited_at: number | null;
  state: "visible" | "removed";
  review_id: string | null;
  pending: boolean;
}

/** One thread (PrThreadVM). `anchor` null = a PR-level discussion thread (the Overview renders those). */
export interface PrThreadVM {
  id: string;
  anchor: PrAnchorVM | null;
  resolved: boolean;
  comments: PrCommentVM[];
}

/** One review batch (PrReviewVM). `advisory` = an agent review — it NEVER counts toward the gate. */
export interface PrReviewVM {
  id: string;
  reviewer: PrincipalVM;
  verdict: "in_progress" | "approved" | "changes_requested" | "commented";
  advisory: boolean;
  submitted_at: number | null;
  summary_md: string | null;
}

/** The GET …/threads envelope (viewer-scoped): discussion (anchor null) vs anchored threads + the
 *  visible review batches. The overview consumes `discussion` + `reviews`. */
export interface PrThreadsVM {
  discussion: PrThreadVM[];
  anchored: PrThreadVM[];
  threads: PrThreadVM[];
  reviews: PrReviewVM[];
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
  /** The VERIFIED R2 gate inputs (never fabricated): a live request-changes blocks unconditionally;
   *  `current_approvals` counts non-author approvals toward `required_approvals`. */
  changes_requested?: boolean;
  current_approvals?: number;
  durable: boolean;
}

/** The rolled-up checks posture for a PR list row (edge `checks_summary`; the ring stays reserved for
 *  the CI verdict trio). `verdict` is the load-bearing leak-free signal; the counts refine the label.
 *  `unavailable` = the projection could not be read (the row fails static — it still lists). */
export interface ChecksSummaryVM {
  verdict: "pass" | "fail" | "running" | "none" | "unavailable";
  passing: number;
  failing: number;
  total: number;
}

/** One PR list row (DurableGitBackend::pr_list_row_json). `title` is `null` for a legacy record with
 *  no stored title — the UI renders `#number` (honest, never a fabricated title). `repo` is present
 *  only on the cross-repo front door (the repo chip). `updated_at` is unix seconds (formatted via
 *  Intl client-side, never hand-formatted). */
export interface PrListRowVM {
  number: number;
  title: string | null;
  pr_state: "draft" | "open" | "merged" | "closed";
  base_ref: string;
  head_ref: string;
  author: string;
  author_is_agent: boolean;
  reviews: number;
  review_state: "requested" | "approved" | "changes" | "none";
  you_are_requested: boolean;
  checks_summary: ChecksSummaryVM;
  updated_at: number | null;
  repo: string | null;
}

/** The R3.1 PR-list envelope: the rows + the bidirectional cursor (`prev_cursor` added for the
 *  Newer/Older pager, fixes ux-git #12) + `counts` computed over the LEAK-FREE set (a forbidden PR
 *  never contributes to a tab/sidebar badge — the anti-oracle rule). */
export interface PrListPage {
  items: PrListRowVM[];
  page: {
    next_cursor: string | null;
    prev_cursor: string | null;
    limit: number;
    offset?: number;
    total?: number;
  };
  counts: Record<string, number>;
}

// ── Founder Issues floor (R4.4). The v1 surface intentionally exposes only one canonical
// dogfood target for creation; project/type/prefix are injected by the server action and never
// accepted from browser code. Lists are authoritative key-prefix searches, not title search. ──

export type IssueListState = "open" | "closed" | "all";
export type IssueStateCategory = "unstarted" | "started" | "completed" | "cancelled";

export interface IssueVM {
  id: string;
  key: string;
  project_id: string;
  state: string;
  state_category: IssueStateCategory;
  title: string;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface IssuesPage {
  items: IssueVM[];
  page: { next_cursor: string | null; limit: number };
}

export interface IssueCreateReceipt {
  issue: { id: string; key: string; project_id: string };
  authorization: { status: "pending"; request_event_id: string };
}

export type IssueAuthorizationStatus =
  | {
      status: "pending";
      issue: IssueCreateReceipt["issue"];
      retry_after_ms: number;
    }
  | { status: "active"; issue: IssueVM };

export type IssueErrorKind =
  | "bad-input"
  | "not-found"
  | "unavailable"
  | "configuration"
  | "error";

export const ISSUE_ERR_PREFIX = "ISSUE_ERR:";

export class IssueRouteError extends Error {
  readonly kind: IssueErrorKind;
  constructor(kind: IssueErrorKind) {
    super(`${ISSUE_ERR_PREFIX}${kind}`);
    this.name = "IssueRouteError";
    this.kind = kind;
  }
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

/** Issues keeps 404 leak-free while distinguishing the retryable projection-unavailable state. */
async function issueAuthed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (e) {
    if (e instanceof Unauthorized) throw redirect("/login");
    if (e instanceof GatewayError) {
      if (e.status === 400) throw new IssueRouteError("bad-input");
      if (e.status === 404) throw new IssueRouteError("not-found");
      if (e.status === 503) throw new IssueRouteError("unavailable");
    }
    if (e instanceof IssueRouteError) throw e;
    throw new IssueRouteError("error");
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

/** The "no access" sentinel — a `Pull` denial is the 0-leak 404 (indistinguishable from an absent
 *  repo, by design), surfaced here as the dignified "not available to you" state (never leaks whether
 *  PRs exist or their count). */
export interface PrListRestricted {
  restricted: true;
}
export type PrListResult = PrListPage | PrListRestricted;

/** The per-repo PR list (GET /v1/git/repos/{repo}/prs?state=&sort=&cursor=). Leak-free by the `Pull`
 *  object guard (`pull_request.view = parent_repo->pull`): a viewer who cannot pull gets the 0-leak
 *  404 the query surfaces as the "no access" state (a real transport failure still throws → the
 *  scoped error state). */
export const getRepoPrs = query(
  async (input: {
    repo: string;
    state?: string;
    sort?: string;
    cursor?: string;
  }): Promise<PrListResult> => {
    "use server";
    const p = new URLSearchParams();
    if (input.state) p.set("state", input.state);
    if (input.sort) p.set("sort", input.sort);
    if (input.cursor) p.set("cursor", input.cursor);
    const q = p.toString();
    return authed(async () => {
      try {
        return await edgeGet<PrListPage>(
          `/v1/git/repos/${seg(input.repo)}/prs${q ? `?${q}` : ""}`,
        );
      } catch (e) {
        // A 0-leak 404 (no pull grant, or an absent repo) → the dignified no-access state; any other
        // status still throws so the route renders the SCOPED error (never a raw err.message).
        if (e instanceof GatewayError && e.status === 404) return { restricted: true };
        throw e;
      }
    });
  },
  "git-prs",
);

/** The cross-repo PR front door (GET /v1/git/prs?bucket=needs-review|yours&cursor=). Prefiltered by
 *  the `visible_repos` list_objects seam — a repo the viewer cannot pull never contributes a PR. */
export const getMyPrs = query(
  async (input: { bucket: "needs-review" | "yours"; cursor?: string }): Promise<PrListPage> => {
    "use server";
    const p = new URLSearchParams({ bucket: input.bucket });
    if (input.cursor) p.set("cursor", input.cursor);
    return authed(() => edgeGet<PrListPage>(`/v1/git/prs?${p.toString()}`));
  },
  "git-prs-cross",
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

/** The PR discussion + review batches (GET /v1/git/repos/{repo}/prs/{n}/threads). Viewer-scoped: a
 *  pending review comment authored by another reviewer never crosses the wire (non-leak). */
export const getPrThreads = query(
  async (input: { repo: string; n: number }): Promise<PrThreadsVM> => {
    "use server";
    return authed(() =>
      edgeGet<PrThreadsVM>(`/v1/git/repos/${seg(input.repo)}/prs/${input.n}/threads`),
    );
  },
  "git-pr-threads",
);

/** The commits IN a PR (GET /v1/git/repos/{repo}/prs/{n}/commits) — reachable from head but not base. */
export const getPrCommits = query(
  async (input: { repo: string; n: number; cursor?: string }): Promise<CommitsPage> => {
    "use server";
    const q = input.cursor ? `?cursor=${seg(input.cursor)}` : "";
    return authed(() =>
      edgeGet<CommitsPage>(`/v1/git/repos/${seg(input.repo)}/prs/${input.n}/commits${q}`),
    );
  },
  "git-pr-commits",
);

/** The PR three-dot diff (GET /v1/git/repos/{repo}/prs/{n}/diff?cursor=&view=). `Pull`-guarded, 0-leak
 *  (a denial is the same 404 as an absent PR — surfaced as the no-access state, never a leaked path). */
export const getPrDiff = query(
  async (input: { repo: string; n: number; cursor?: string; view?: string }): Promise<PrDiffVM> => {
    "use server";
    const p = new URLSearchParams();
    if (input.cursor) p.set("cursor", input.cursor);
    if (input.view) p.set("view", input.view);
    const q = p.toString();
    return authed(() =>
      edgeGet<PrDiffVM>(`/v1/git/repos/${seg(input.repo)}/prs/${input.n}/diff${q ? `?${q}` : ""}`),
    );
  },
  "git-pr-diff",
);

/** Expand-context lines (GET /v1/git/repos/{repo}/file-lines/{oid}?path=&start=&end=). Same object
 *  check as the blob route (`Pull`). Returns context lines (origin " ") carrying their blob line
 *  number in `new_no`; the client maps the old-side column from the surrounding hunk offset. */
export const getFileLines = query(
  async (input: {
    repo: string;
    oid: string;
    path: string;
    start: number;
    end: number;
  }): Promise<{ lines: DiffLineVM[] }> => {
    "use server";
    const p = new URLSearchParams({
      path: input.path,
      start: String(input.start),
      end: String(input.end),
    });
    return authed(() =>
      edgeGet<{ lines: DiffLineVM[] }>(
        `/v1/git/repos/${seg(input.repo)}/file-lines/${seg(input.oid)}?${p.toString()}`,
      ),
    );
  },
  "git-file-lines",
);

/** Authoritative Issues list. `key` is an ASCII issue-key prefix, never title/free-text search. */
export const getIssues = query(
  async (input: {
    state: IssueListState;
    key?: string;
    cursor?: string;
    limit?: number;
  }): Promise<IssuesPage> => {
    "use server";
    const p = new URLSearchParams({ state: input.state });
    if (input.key) p.set("key", input.key);
    if (input.cursor) p.set("cursor", input.cursor);
    if (input.limit) p.set("limit", String(input.limit));
    return issueAuthed(() => edgeGet<IssuesPage>(`/v1/issues?${p.toString()}`));
  },
  "issues-list",
);

export const getIssue = query(async (id: string): Promise<IssueVM> => {
  "use server";
  return issueAuthed(() => edgeGet<IssueVM>(`/v1/issues/${seg(id)}`));
}, "issue-detail");

export type IssueMutationResult =
  | { ok: true; op: "create"; receipt: IssueCreateReceipt }
  | { ok: true; op: "close"; issue: IssueVM }
  | { ok: true; op: "activation"; status: IssueAuthorizationStatus }
  | { ok: false; error: IssueErrorKind };

export const ISSUE_ACTIVATION_STATUS_TIMEOUT_MS = 10_000;

function isCanonicalUuid(value: string | undefined): value is string {
  return Boolean(
    value &&
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value),
  );
}

/** Read and validate the one founder target. Invalid deployment config stays opaque to the client. */
function dogfoodIssueTarget(): { project_id: string; type_id: string; prefix: string } {
  const project = process.env.MYELIN_DOGFOOD_ISSUES_PROJECT;
  const type = process.env.MYELIN_DOGFOOD_ISSUES_TYPE;
  const prefix = process.env.MYELIN_DOGFOOD_ISSUES_PREFIX;
  if (
    !isCanonicalUuid(project) ||
    !isCanonicalUuid(type) ||
    !prefix ||
    !/^[A-Z0-9]{2,10}$/.test(prefix)
  ) {
    throw new IssueRouteError("configuration");
  }
  return { project_id: project, type_id: type, prefix };
}

/** One Issues mutation action: browser inputs can carry a title or an issue UUID, never scope IDs. */
export const issuesMutate = action(async (mutation: IssueMutation) => {
  "use server";
  const result = (value: IssueMutationResult) => json(value, { revalidate: [] });
  try {
    const parsed = parseIssueMutation(mutation);
    if (!parsed) return result({ ok: false, error: "bad-input" });
    if (parsed.op === "create") {
      const target = dogfoodIssueTarget();
      const receipt = await issueAuthed(async () => {
        const response = await edgePost("/v1/issues", { ...target, title: parsed.title });
        const decoded = parseIssueCreateReceipt(response);
        if (!decoded) throw new IssueRouteError("error");
        return decoded;
      });
      return result({ ok: true, op: "create", receipt });
    }
    if (parsed.op === "activation") {
      const status = await issueAuthed(async () => {
        const response = await edgeGet(
          `/v1/issues/authorization-requests/${seg(parsed.requestEventId)}`,
          { timeoutMs: ISSUE_ACTIVATION_STATUS_TIMEOUT_MS },
        );
        const decoded = parseIssueAuthorizationStatus(response);
        if (!decoded) throw new IssueRouteError("error");
        return decoded;
      });
      return result({ ok: true, op: "activation", status });
    }
    const issue = await issueAuthed(async () => {
      const response = await edgePost(`/v1/issues/${seg(parsed.issueId)}/close`, {});
      const decoded = parseIssue(response);
      if (!decoded) throw new IssueRouteError("error");
      return decoded;
    });
    return result({ ok: true, op: "close", issue });
  } catch (e) {
    if (e instanceof IssueRouteError) return result({ ok: false, error: e.kind });
    throw e;
  }
}, "issues-mutate");

// ── PR write paths (R3.3 G-8): threads, comments, review batches, merge. Server-only functions;
//    the overview route calls them then revalidates the thread/checks queries. A 409 merge surfaces
//    the fresh re-rendered checks so the UI re-renders the blocked card (N6), never merges on stale. ──

/** The typed 409-blocked result of a merge attempt — carries the FRESH checks projection (N6). */
export interface MergeBlocked {
  blocked: true;
  checks: PrChecksVM | null;
}
export interface MergeOk {
  blocked: false;
  base_ref: string;
  new_oid: string;
}
export type MergeResult = MergeOk | MergeBlocked;

/** The PR write ops (R3.3 G-8). ALL mutations route through ONE dispatching `action` — multiple
 *  sibling `action(...)`s in this module collided onto one server-fn under the vinxi bundler (each
 *  resolved to the first and returned null), so a single server function keyed by `op` is the robust
 *  shape. The server-RPC (`action`) is the proven path with request/session context (a bare
 *  `"use server"` function did not bind reliably here). */
/** The union of every mutation's result (the caller narrows by `op`). */
export type PrMutationResult =
  | { thread: PrThreadVM }
  | { comment: PrCommentVM }
  | { review: PrReviewVM }
  | { ok: true }
  | MergeResult;

export const prMutate = action(async (m: PrMutation): Promise<PrMutationResult> => {
  "use server";
  const parsed = parsePrMutation(m);
  if (!parsed) throw new RepoRouteError("error");
  const base = `/v1/git/repos/${seg(parsed.repo)}/prs/${parsed.n}`;
  return authed(async () => {
    switch (parsed.op) {
      case "thread": {
        const response = await edgePost(`${base}/threads`, {
          body_md: parsed.body_md,
          ...(parsed.anchor ? { anchor: parsed.anchor } : {}),
        });
        const thread = parseAppliedThread(response);
        if (!thread) throw new RepoRouteError("error");
        return { thread };
      }
      case "comment": {
        const response = await edgePost(`${base}/threads/${seg(parsed.threadId)}/comments`, { body_md: parsed.body_md });
        const comment = parseAppliedComment(response, "git.pr.comment.create");
        if (!comment) throw new RepoRouteError("error");
        return { comment };
      }
      case "review-start": {
        const response = await edgePost(`${base}/reviews/start`, {});
        const review = parseAppliedReview(response);
        if (!review) throw new RepoRouteError("error");
        return { review };
      }
      case "review-comment": {
        const response = await edgePost(`${base}/reviews/${seg(parsed.reviewId)}/comments`, { body_md: parsed.body_md });
        const comment = parseAppliedComment(response, "git.pr.review.comment");
        if (!comment) throw new RepoRouteError("error");
        return { comment };
      }
      case "review-submit": {
        const response = await edgePost(`${base}/reviews/${seg(parsed.reviewId)}/submit`, { verdict: parsed.verdict, summary_md: parsed.summary_md });
        if (!hasAppliedAction(response, "git.pr.review.submit")) throw new RepoRouteError("error");
        return { ok: true };
      }
      case "review-discard": {
        const response = await edgePost(`${base}/reviews/${seg(parsed.reviewId)}/discard`, {});
        if (!hasAppliedAction(response, "git.pr.review.discard")) throw new RepoRouteError("error");
        return { ok: true };
      }
      case "merge": {
        try {
          const response = await edgePost(
            `${base}/merge`,
            {},
            { idempotencyKey: crypto.randomUUID() },
          );
          const merged = parseAppliedMerge(response);
          if (!merged) throw new RepoRouteError("error");
          return { blocked: false, ...merged };
        } catch (e) {
          if (e instanceof GatewayError && e.status === 409) {
            const body = e.body !== null && typeof e.body === "object" && !Array.isArray(e.body)
              ? e.body as Record<string, unknown>
              : null;
            const checks = parsePrChecks(body?.checks);
            return { blocked: true, checks };
          }
          throw e;
        }
      }
    }
  });
}, "git-pr-mutate");
