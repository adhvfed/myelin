// The data layer: SolidStart `query`s that call the edge THROUGH the server-side gateway client. The
// `"use server"` directive keeps the gateway + token strictly server-side. On `Unauthorized` (a 401
// that survived the single refresh + retry) the query throws a `/login` redirect — the canon's
// 401→/login behaviour, applied centrally.
import { action, json, query, redirect } from "@solidjs/router";
import { edgeGet, edgePost, GatewayError, isUnauthorized } from "../server/gateway";
import {
  parsePrMutation,
  type PrMutation,
} from "./mutation-input";
import {
  hasAppliedAction,
  parseAppliedComment,
  parseAppliedMerge,
  parseAppliedReview,
  parseAppliedThreadResolution,
  parseAppliedThread,
  parsePrChecks,
  parsePrThreads,
} from "./mutation-response";
import { parseFileLinesInput, parseFileLinesResponse, type FileLine } from "./file-lines";
import { parseBlob, parseRefs, parseRepoHome, parseTree } from "./repo-read-response";
import { parseBlame, type BlameVM } from "./blame-response";
import { parseRepoListPage } from "./repo-list-response";
import { parseCommitDiff, parseCommitsPage, parsePrCommitsPage } from "./commit-read-response";
import { parsePr, parsePrDiff, parsePrListPage } from "./pr-read-response";
import {
  parseInboxPage,
  parseInboxReadReceipt,
  type InboxPage,
  type InboxReadReceipt,
} from "./inbox-response";
import {
  parseGitBrowseInput,
  parseGitCommitInput,
  parseGitCommitsInput,
  parseGitMyPrsInput,
  parseGitPrCommitsInput,
  parseGitPrDiffInput,
  parseGitPrInput,
  parseGitRepoListInput,
  parseGitRepoInput,
  parseGitRepoPrsInput,
  parseGitRefsInput,
  parseGitTreeInput,
  gitRefsSearchParams,
  gitRepoListSearchParams,
  gitTreeSearchParams,
  gitPrCommitsPath,
  type GitRepoListInput,
  type GitRefsInput,
  type GitTreeInput,
} from "./git-read-input";
import {
  ciLogSearchParams,
  ciRunsSearchParams,
  parseCiLogInput,
  parseCiRunId,
  parseCiRunsInput,
  type CiLogInput,
  type CiRunsInput,
} from "./ci-read-input";
import {
  parseCiLogRange,
  parseCiRunDetail,
  parseCiRunsPage,
  type CiLogRangeVM,
  type CiRunDetailVM,
  type CiRunsPage,
} from "./ci-read-response";
import {
  codeSearchParams,
  parseCodeSearchInput,
  parseCodeSearchPage,
  type CodeSearchInput,
  type CodeSearchPage,
} from "./code-search";
import { parseRepoCreateReceipt, parseRepositorySlug, type RepoCreateReceipt } from "./repo-create";
import {
  mapPrDiffStatusToKind,
  mapStatusToKind,
  RepoRouteError,
  type RepoErrorKind,
} from "./repo-error";

export type { PrMutation } from "./mutation-input";
export type { CodeSearchHit, CodeSearchInput, CodeSearchPage } from "./code-search";
export type { RepoCreateReceipt } from "./repo-create";
export {
  REPO_ERR_PREFIX,
  RepoRouteError,
  mapPrDiffStatusToKind,
  mapStatusToKind,
  type RepoErrorKind,
} from "./repo-error";
export * from "./chat-api";
export * from "./knowledge-api";

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
  name: string;
  path: string;
  is_dir: boolean;
  size?: number;
  latest_commit?: CommitBriefVM;
}

interface VisibleRepoHomeVM {
  slug: string;
  ref: string;
  clone_url: string;
  default_branch: string;
  counts: { branches: number; tags: number };
}

export interface EmptyRepoHomeVM extends VisibleRepoHomeVM {
  state: "empty";
}

/** The populated Git repository home projected by the Edge. */
export interface PopulatedRepoHomeVM extends VisibleRepoHomeVM {
  state: "populated";
  /** Full README markdown (rendered via the read-path / sanitized markdown renderer). */
  readme?: string;
  readme_excerpt?: string;
  entries: RepoEntry[];
  latest_commit?: CommitBriefVM;
  snapshot_oid: string;
  entries_page: TreePageVM & { ref: string; snapshot_oid: string };
}

/** The Git RepoHome ViewModel contains only visible repositories; denied reads are zero-leak 404s. */
export type RepoHomeVM = PopulatedRepoHomeVM | EmptyRepoHomeVM;

/** One ref row for the switcher. */
export interface RefRow {
  name: string;
  oid: string;
  is_default?: boolean;
}

export interface BranchRefRow extends RefRow {
  is_default: boolean;
}

export interface PinnedRefRow extends RefRow {
  kind: "branch" | "tag";
  full_name: string;
  is_default: boolean;
}

/** The ref switcher source (GET /v1/git/repos/{repo}/refs). */
export interface RefsVM {
  branches: BranchRefRow[];
  tags: RefRow[];
  default_branch: string;
  pinned: PinnedRefRow[];
  page: { next_cursor: string | null; limit: number };
}

/** A snapshot-pinned directory page from GET /v1/git/repos/{repo}/tree/{ref}/{...path}. */
export interface TreeDirectoryVM {
  ref: string;
  path: string;
  entries: RepoEntry[];
  readme?: string;
  redirect_to_blob?: false;
  snapshot_oid: string;
  page: TreePageVM;
}

export interface TreePageVM {
  next_cursor: string | null;
  limit: number;
}

/** A file requested through the tree route. The browser follows it to the blob surface. */
export interface TreeBlobRedirectVM {
  ref: string;
  path: string;
  redirect_to_blob: true;
  entries?: never;
  readme?: never;
  snapshot_oid?: never;
  page?: never;
}

export type TreeVM = TreeDirectoryVM | TreeBlobRedirectVM;

/** One summary-only repository catalogue row. Heavy RepoHome data belongs to `GET /repos/{repo}`. */
export type RepoListRowVM =
  | { state: "populated"; slug: string; clone_url: string }
  | { state: "empty"; slug: string };

/** The bounded repository catalogue envelope. */
export interface RepoListPage {
  items: RepoListRowVM[];
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
  preview_unavailable?: boolean;
  download_available?: boolean;
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
    /** Visible range and page; the API does not provide a total. */
    range?: { from: number; to: number };
  };
}

export interface PrCommitsPage {
  items: CommitRowVM[];
  page: {
    next_cursor: string | null;
    limit: number;
  };
}

/** A compact overview page that still makes pagination meaningful in ordinary pull requests. */
export const PR_COMMITS_PAGE_LIMIT = 20;

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

/** One visible file in a PR diff. Restricted files contribute only to
 * `PrDiffVM.restricted_files`. */
export interface PrDiffFileVM {
  path: string;
  old_path: string | null;
  /** Immutable new-side blob used by the bounded expand-context endpoint. */
  new_blob_oid: string | null;
  status: string;
  kind: "text" | "binary" | "lfs" | "submodule";
  additions: number;
  deletions: number;
  size_bytes: number | null;
  hunks: DiffHunkVM[];
  deleted_body_available: boolean;
  truncated: boolean;
}

/** PR diff response. `three_dot` distinguishes merge-base and two-dot results;
 * `restricted_files` contains a count, not paths. */
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

/** Expected bounded-capacity result for a PR diff. It is data rather than an exception so a direct
 * SSR navigation can render the calm state with HTTP 200 instead of escaping as a route 500. */
export interface PrDiffCapacityVM {
  state: "diff-too-large";
}

export type PrDiffReadVM = PrDiffVM | PrDiffCapacityVM;

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

/** Durable PR record returned by `DurableGitBackend::pr_json`. */
export interface PrVM {
  number: number;
  ref: string;
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

/** One review batch. Advisory agent reviews do not count toward the gate. */
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

/** Rolled-up checks state for a PR list row. Counts refine the verdict label. */
export interface ChecksSummaryVM {
  verdict: "pass" | "fail" | "running" | "none" | "unavailable";
  passing: number;
  failing: number;
  total: number;
}

/** One PR list row (DurableGitBackend::pr_list_row_json). `title` is `null` for a legacy record with
 *  no stored title — the UI renders `#number`. `repo` is present
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

export type CiErrorKind = "bad-input" | "not-found" | "stale" | "unavailable" | "error";
export const CI_ERR_PREFIX = "CI_ERR:";

/** CI errors expose only a category to the browser. */
export class CiRouteError extends Error {
  readonly kind: CiErrorKind;
  constructor(kind: CiErrorKind) {
    super(`${CI_ERR_PREFIX}${kind}`);
    this.name = "CiRouteError";
    this.kind = kind;
  }
}

/** Run an edge GET through the gateway: a surviving 401 → the `/login` redirect (unchanged); any
 *  other edge failure → a `RepoRouteError` carrying the mapped kind, so every git route renders the
 *  shared `<RepoErrorState>` instead of leaking `err.message`. */
async function authed<T>(
  fetcher: () => Promise<T>,
  statusKind: (status: number) => RepoErrorKind = mapStatusToKind,
): Promise<T> {
  try {
    return await fetcher();
  } catch (e) {
    if (isUnauthorized(e)) throw redirect("/login");
    if (e instanceof GatewayError) throw new RepoRouteError(statusKind(e.status));
    // A transport/parse failure with no HTTP status is the retryable error kind.
    throw new RepoRouteError("error");
  }
}

async function treeAuthed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (e) {
    if (isUnauthorized(e)) throw redirect("/login");
    if (e instanceof GatewayError) {
      if (e.status === 409) throw new RepoRouteError("stale-tree");
      throw new RepoRouteError(mapStatusToKind(e.status));
    }
    throw new RepoRouteError("error");
  }
}

async function prDiffAuthed<T>(fetcher: () => Promise<T>): Promise<T | PrDiffCapacityVM> {
  try {
    return await fetcher();
  } catch (e) {
    if (isUnauthorized(e)) throw redirect("/login");
    if (e instanceof GatewayError) {
      if (e.status === 413) return { state: "diff-too-large" };
      throw new RepoRouteError(mapPrDiffStatusToKind(e.status));
    }
    if (e instanceof RepoRouteError) throw e;
    throw new RepoRouteError("error");
  }
}

async function ciAuthed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (e) {
    if (isUnauthorized(e)) throw redirect("/login");
    if (e instanceof GatewayError) {
      if (e.status === 400) throw new CiRouteError("bad-input");
      if (e.status === 404) throw new CiRouteError("not-found");
      if (e.status === 409) throw new CiRouteError("stale");
      if (e.status === 503) throw new CiRouteError("unavailable");
    }
    if (e instanceof CiRouteError) throw e;
    throw new CiRouteError("error");
  }
}

async function inboxAuthed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (e) {
    if (isUnauthorized(e)) throw redirect("/login");
    // Mutation callers deliberately distinguish stale/missing approvals (404/409) from transport
    // outages. Preserve the status here; read queries map it to an opaque availability error below.
    if (e instanceof GatewayError) throw e;
    throw new Error("INBOX_UNAVAILABLE");
  }
}

/** Encode a path segment for the edge URL (the gateway matches one segment per `{param}`). */
function seg(s: string): string {
  return encodeURIComponent(s);
}

/** The repos screen's summary data. Repo-home tree/README/history fields never enter this request. */
export const getRepos = query(async (request: GitRepoListInput = {}): Promise<RepoListPage> => {
  "use server";
  const input = parseGitRepoListInput(request);
  if (!input) throw new RepoRouteError("error");
  return authed(async () => {
    const search = gitRepoListSearchParams(input).toString();
    const page = parseRepoListPage(
      await edgeGet(`/v1/git/repos${search.length === 0 ? "" : `?${search}`}`),
    );
    if (!page) throw new RepoRouteError("error");
    return page;
  });
}, "git-repos");

export const getCodeSearch = query(async (request: CodeSearchInput): Promise<CodeSearchPage> => {
  "use server";
  const input = parseCodeSearchInput(request);
  if (!input) throw new RepoRouteError("error");
  return authed(async () => {
    const page = parseCodeSearchPage(
      await edgeGet(`/v1/git/search/code?${codeSearchParams(input).toString()}`),
    );
    if (!page) throw new RepoRouteError("error");
    return page;
  });
}, "git-code-search");

export type RepoCreateError = "bad-input" | "exists" | "forbidden" | "error";
export type RepoCreateResult =
  | { ok: true; receipt: RepoCreateReceipt }
  | { ok: false; error: RepoCreateError };

export const createRepo = action(async (value: string) => {
  "use server";
  const result = (response: RepoCreateResult) => json(response, { revalidate: [] });
  const slug = parseRepositorySlug(value);
  if (!slug) return result({ ok: false, error: "bad-input" });
  try {
    const response = await edgePost("/v1/git/repos", { slug });
    const receipt = parseRepoCreateReceipt(response, slug);
    return result(receipt ? { ok: true, receipt } : { ok: false, error: "error" });
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) return result({ ok: false, error: "bad-input" });
      if (error.status === 403) return result({ ok: false, error: "forbidden" });
      if (error.status === 409) return result({ ok: false, error: "exists" });
    }
    return result({ ok: false, error: "error" });
  }
}, "git-repo-create");

/** Repository-authorized durable CI run summaries, newest first under an opaque keyset cursor. */
export const getCiRuns = query(async (request: CiRunsInput = {}): Promise<CiRunsPage> => {
  "use server";
  const input = parseCiRunsInput(request);
  if (!input) throw new CiRouteError("bad-input");
  return ciAuthed(async () => {
    const search = ciRunsSearchParams(input).toString();
    const page = parseCiRunsPage(await edgeGet(`/v1/ci/runs${search ? `?${search}` : ""}`));
    if (!page) throw new CiRouteError("error");
    return page;
  });
}, "ci-runs");

/** One authorized durable run with its exact job DAG and archived-log step anchors. */
export const getCiRun = query(async (run: string): Promise<CiRunDetailVM> => {
  "use server";
  const input = parseCiRunId(run);
  if (!input) throw new CiRouteError("bad-input");
  return ciAuthed(async () => {
    const detail = parseCiRunDetail(await edgeGet(`/v1/ci/runs/${seg(input)}`), input);
    if (!detail) throw new CiRouteError("error");
    return detail;
  });
}, "ci-run");

/** A byte-exact bounded archived log range. This is deliberately not named or rendered as live. */
export const getCiLog = query(async (request: CiLogInput): Promise<CiLogRangeVM> => {
  "use server";
  const input = parseCiLogInput(request);
  if (!input) throw new CiRouteError("bad-input");
  return ciAuthed(async () => {
    const search = ciLogSearchParams(input).toString();
    const range = parseCiLogRange(await edgeGet(
      `/v1/ci/runs/${seg(input.run)}/jobs/${seg(input.job)}/log${search ? `?${search}` : ""}`,
    ), input);
    if (!range) throw new CiRouteError("error");
    return range;
  });
}, "ci-log");

/** One bounded page of the authenticated viewer's unified inbox. */
export const getInbox = query(async (cursor: string | null = null): Promise<InboxPage> => {
  "use server";
  if (cursor !== null && (typeof cursor !== "string" || cursor.length > 1_024 ||
      !cursor.startsWith("ni1_") || /[\p{Cc}]/u.test(cursor))) {
    throw new Error("INBOX_INVALID_CURSOR");
  }
  try {
    return await inboxAuthed(async () => {
      const search = new URLSearchParams({ view: "all", limit: "50" });
      if (cursor !== null) search.set("cursor", cursor);
      const page = parseInboxPage(await edgeGet(`/v1/notif/inbox?${search.toString()}`));
      if (!page) throw new Error("INBOX_INVALID_RESPONSE");
      return page;
    });
  } catch (error) {
    if (error instanceof GatewayError) throw new Error("INBOX_UNAVAILABLE");
    throw error;
  }
}, "notif-inbox");

export type InboxMutationResult =
  | { ok: true; receipt: InboxReadReceipt }
  | { ok: false };

export type AutomationApprovalDecision = "approve" | "reject";

export interface AutomationApprovalInput {
  automationId: string;
  eventId: string;
  decision: AutomationApprovalDecision;
}

/** Mark one authenticated recipient-scoped inbox item as read. */
export const markInboxRead = action(async (itemId: string) => {
  "use server";
  const result = (value: InboxMutationResult) => json(value, { revalidate: [] });
  if (typeof itemId !== "string" || itemId.length === 0 || itemId.length > 512 ||
      /[\p{Cc}]/u.test(itemId)) return result({ ok: false });
  try {
    return await inboxAuthed(async () => {
      const response = await edgePost(`/v1/notif/inbox/${seg(itemId)}/read`, {});
      const receipt = parseInboxReadReceipt(response);
      return result(receipt ? { ok: true, receipt } : { ok: false });
    });
  } catch (error) {
    if (error instanceof GatewayError && error.status === 404) return result({ ok: false });
    throw error;
  }
}, "notif-inbox-mark-read");

/** Decide one exact automation firing surfaced by the inbox. */
export const decideAutomationApproval = action(async (input: AutomationApprovalInput) => {
  "use server";
  const result = (value: { ok: boolean }) => json(value, { revalidate: [] });
  if (!input ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
        input.automationId,
      ) ||
      typeof input.eventId !== "string" || input.eventId.length === 0 ||
      new TextEncoder().encode(input.eventId).byteLength > 255 ||
      /[\p{Cc}]/u.test(input.eventId) ||
      (input.decision !== "approve" && input.decision !== "reject")) return result({ ok: false });
  try {
    return await inboxAuthed(async () => {
      const response = await edgePost(
        `/v1/triggers/${seg(input.automationId)}/firings/${input.decision}`,
        { event_id: input.eventId },
      );
      const receipt = response !== null && typeof response === "object" &&
        !Array.isArray(response) ? response as Record<string, unknown> : null;
      return result({ ok: receipt?.action === input.decision });
    });
  } catch (error) {
    if (error instanceof GatewayError && (error.status === 404 || error.status === 409)) {
      return result({ ok: false });
    }
    throw error;
  }
}, "notif-automation-approval");

export interface AgentEffectApprovalInput {
  gateId: string;
  decision: AutomationApprovalDecision;
}

/** Decide one exact agent-proposed effect surfaced by the shared inbox. */
export const decideAgentEffectApproval = action(async (input: AgentEffectApprovalInput) => {
  "use server";
  const result = (value: { ok: boolean }) => json(value, { revalidate: [] });
  if (!input || !/^gate:[0-9a-f]{32}$/.test(input.gateId) ||
      (input.decision !== "approve" && input.decision !== "reject")) {
    return result({ ok: false });
  }
  try {
    return await inboxAuthed(async () => {
      const response = await edgePost(
        `/v1/agent-approvals/${seg(input.gateId)}/decision`,
        { decision: input.decision },
      );
      const receipt = response !== null && typeof response === "object" &&
        !Array.isArray(response) ? response as Record<string, unknown> : null;
      return result({ ok: receipt?.gate_id === input.gateId });
    });
  } catch (error) {
    if (error instanceof GatewayError && (error.status === 404 || error.status === 409)) {
      return result({ ok: false });
    }
    throw error;
  }
}, "notif-agent-effect-approval");

/** A single repo's home (GET /v1/git/repos/{repo}) → the RepoHome ViewModel. */
export const getRepo = query(async (repo: string): Promise<RepoHomeVM> => {
  "use server";
  const input = parseGitRepoInput(repo);
  if (!input) throw new RepoRouteError("error");
  return authed(async () => {
    const home = parseRepoHome(await edgeGet(`/v1/git/repos/${seg(input.repo)}`));
    if (!home) throw new RepoRouteError("error");
    return home;
  });
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
export const getRefs = query(async (request: GitRefsInput): Promise<RefsVM> => {
  "use server";
  const input = parseGitRefsInput(request);
  if (!input) throw new RepoRouteError("error");
  return authed(async () => {
    const search = gitRefsSearchParams(input).toString();
    const refs = parseRefs(await edgeGet(
      `/v1/git/repos/${seg(input.repo)}/refs${search ? `?${search}` : ""}`,
    ));
    if (!refs) throw new RepoRouteError("error");
    return refs;
  });
}, "git-refs");

/** A tree at a ref + nested path (GET /v1/git/repos/{repo}/tree/{ref}/{...path}). Root = empty path. */
export const getTree = query(
  async (input: GitTreeInput): Promise<TreeVM> => {
    "use server";
    const parsed = parseGitTreeInput(input);
    if (!parsed) throw new RepoRouteError("error");
    const tail = parsed.path ? `/${nestedPath(parsed.path)}` : "";
    return treeAuthed(async () => {
      const search = gitTreeSearchParams(parsed).toString();
      const tree = parseTree(await edgeGet(
        `/v1/git/repos/${seg(parsed.repo)}/tree/${seg(parsed.ref)}${tail}${search ? `?${search}` : ""}`,
      ));
      if (!tree) throw new RepoRouteError("error");
      return tree;
    });
  },
  "git-tree",
);

/** A single file at a ref + NESTED path (GET /v1/git/repos/{repo}/blob/{ref}/{...path}). */
export const getBlob = query(
  async (input: { repo: string; ref: string; path: string }): Promise<BlobVM> => {
    "use server";
    const parsed = parseGitBrowseInput(input, false);
    if (!parsed) throw new RepoRouteError("error");
    return authed(async () => {
      const blob = parseBlob(await edgeGet(
        `/v1/git/repos/${seg(parsed.repo)}/blob/${seg(parsed.ref)}/${nestedPath(parsed.path)}`,
      ));
      if (!blob) throw new RepoRouteError("error");
      return blob;
    });
  },
  "git-blob",
);

/** Line attribution for one text file, pinned to the ref's resolved commit snapshot. */
export const getBlame = query(
  async (input: { repo: string; ref: string; path: string }): Promise<BlameVM> => {
    "use server";
    const parsed = parseGitBrowseInput(input, false);
    if (!parsed) throw new RepoRouteError("error");
    return authed(async () => {
      const blame = parseBlame(await edgeGet(
        `/v1/git/repos/${seg(parsed.repo)}/blame/${seg(parsed.ref)}/${nestedPath(parsed.path)}`,
      ));
      if (!blame) throw new RepoRouteError("error");
      return blame;
    });
  },
  "git-blame",
);

/** The commit log for a ref (GET /v1/git/repos/{repo}/commits/{ref}). */
export const getCommits = query(
  async (input: { repo: string; ref: string; cursor?: string }): Promise<CommitsPage> => {
    "use server";
    const parsed = parseGitCommitsInput(input);
    if (!parsed) throw new RepoRouteError("error");
    const q = parsed.cursor ? `?cursor=${seg(parsed.cursor)}` : "";
    return authed(async () => {
      const page = parseCommitsPage(await edgeGet(
        `/v1/git/repos/${seg(parsed.repo)}/commits/${seg(parsed.ref)}${q}`,
      ));
      if (!page) throw new RepoRouteError("error");
      return page;
    });
  },
  "git-commits",
);

/** A single commit's diff (GET /v1/git/repos/{repo}/commit/{oid}). */
export const getCommit = query(
  async (input: { repo: string; oid: string }): Promise<CommitDiffVM> => {
    "use server";
    const parsed = parseGitCommitInput(input);
    if (!parsed) throw new RepoRouteError("error");
    return authed(async () => {
      const commit = parseCommitDiff(await edgeGet(
        `/v1/git/repos/${seg(parsed.repo)}/commit/${seg(parsed.oid)}`,
      ));
      if (!commit) throw new RepoRouteError("error");
      return commit;
    });
  },
  "git-commit",
);

/** A 404 sentinel that covers both absent repositories and withheld access. */
export interface PrListRestricted {
  restricted: true;
}
export type PrListResult = PrListPage | PrListRestricted;

/** Per-repository PR list. A 404 maps to the restricted sentinel; other failures propagate. */
export const getRepoPrs = query(
  async (input: {
    repo: string;
    state?: string;
    sort?: string;
    cursor?: string;
  }): Promise<PrListResult> => {
    "use server";
    const parsed = parseGitRepoPrsInput(input);
    if (!parsed) throw new RepoRouteError("error");
    const p = new URLSearchParams();
    if (parsed.state) p.set("state", parsed.state);
    if (parsed.sort) p.set("sort", parsed.sort);
    if (parsed.cursor) p.set("cursor", parsed.cursor);
    const q = p.toString();
    return authed(async () => {
      try {
        const page = parsePrListPage(await edgeGet(
          `/v1/git/repos/${seg(parsed.repo)}/prs${q ? `?${q}` : ""}`,
        ), "repo");
        if (!page) throw new RepoRouteError("error");
        return page;
      } catch (e) {
        // A 404 covers both absent repositories and withheld access.
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
    const parsed = parseGitMyPrsInput(input);
    if (!parsed) throw new RepoRouteError("error");
    const p = new URLSearchParams({ bucket: parsed.bucket });
    if (parsed.cursor) p.set("cursor", parsed.cursor);
    return authed(async () => {
      const page = parsePrListPage(await edgeGet(`/v1/git/prs?${p.toString()}`), "cross");
      if (!page) throw new RepoRouteError("error");
      return page;
    });
  },
  "git-prs-cross",
);

/** A PR record (GET /v1/git/repos/{repo}/prs/{n}). */
export const getPr = query(
  async (input: { repo: string; n: number }): Promise<PrVM> => {
    "use server";
    const parsed = parseGitPrInput(input);
    if (!parsed) throw new RepoRouteError("error");
    return authed(async () => {
      const pr = parsePr(await edgeGet(`/v1/git/repos/${seg(parsed.repo)}/prs/${parsed.n}`));
      if (!pr) throw new RepoRouteError("error");
      return pr;
    });
  },
  "git-pr",
);

/** A PR's checks + merge-gate projection (GET /v1/git/repos/{repo}/prs/{n}/checks). */
export const getPrChecks = query(
  async (input: { repo: string; n: number }): Promise<PrChecksVM> => {
    "use server";
    const parsed = parseGitPrInput(input);
    if (!parsed) throw new RepoRouteError("error");
    return authed(async () => {
      const response = await edgeGet(`/v1/git/repos/${seg(parsed.repo)}/prs/${parsed.n}/checks`);
      const checks = parsePrChecks(response);
      if (!checks) throw new RepoRouteError("error");
      return checks;
    });
  },
  "git-pr-checks",
);

/** The PR discussion + review batches (GET /v1/git/repos/{repo}/prs/{n}/threads). Viewer-scoped: a
 *  pending review comment authored by another reviewer never crosses the wire (non-leak). */
export const getPrThreads = query(
  async (input: { repo: string; n: number }): Promise<PrThreadsVM> => {
    "use server";
    const parsed = parseGitPrInput(input);
    if (!parsed) throw new RepoRouteError("error");
    return authed(async () => {
      const response = await edgeGet(`/v1/git/repos/${seg(parsed.repo)}/prs/${parsed.n}/threads`);
      const threads = parsePrThreads(response);
      if (!threads) throw new RepoRouteError("error");
      return threads;
    });
  },
  "git-pr-threads",
);

/** The commits IN a PR (GET /v1/git/repos/{repo}/prs/{n}/commits) — reachable from head but not base. */
export const getPrCommits = query(
  async (input: { repo: string; n: number; limit?: number; cursor?: string }): Promise<PrCommitsPage> => {
    "use server";
    const parsed = parseGitPrCommitsInput(input);
    if (!parsed) throw new RepoRouteError("error");
    return authed(async () => {
      const page = parsePrCommitsPage(await edgeGet(
        gitPrCommitsPath(parsed),
      ));
      if (!page) throw new RepoRouteError("error");
      return page;
    });
  },
  "git-pr-commits",
);

/** The PR three-dot diff (GET /v1/git/repos/{repo}/prs/{n}/diff?cursor=). `Pull`-guarded, 0-leak
 *  (a denial is the same 404 as an absent PR — surfaced as the no-access state, never a leaked path). */
export const getPrDiff = query(
  async (input: {
    repo: string;
    n: number;
    cursor?: string;
  }): Promise<PrDiffReadVM> => {
    "use server";
    const parsed = parseGitPrDiffInput(input);
    if (!parsed) throw new RepoRouteError("error");
    const p = new URLSearchParams();
    if (parsed.cursor) p.set("cursor", parsed.cursor);
    const q = p.toString();
    return prDiffAuthed(async () => {
      const diff = parsePrDiff(await edgeGet(
        `/v1/git/repos/${seg(parsed.repo)}/prs/${parsed.n}/diff${q ? `?${q}` : ""}`,
      ));
      if (!diff) throw new RepoRouteError("error");
      return diff;
    });
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
  }): Promise<{ lines: FileLine[] }> => {
    "use server";
    const parsed = parseFileLinesInput(input);
    if (!parsed) throw new RepoRouteError("error");
    const p = new URLSearchParams({
      path: parsed.path,
      start: String(parsed.start),
      end: String(parsed.end),
    });
    return authed(async () => {
      const response = await edgeGet(
        `/v1/git/repos/${seg(parsed.repo)}/file-lines/${seg(parsed.oid)}?${p.toString()}`,
      );
      const lines = parseFileLinesResponse(response);
      if (!lines) throw new RepoRouteError("error");
      return lines;
    });
  },
  "git-file-lines",
);

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

/** PR mutations share one action because Vinxi collides sibling actions in this module. */
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
    const mutationOptions = {
      idempotencyKey: "clientNonce" in parsed ? parsed.clientNonce : crypto.randomUUID(),
    };
    switch (parsed.op) {
      case "thread": {
        const response = await edgePost(`${base}/threads`, {
          body_md: parsed.body_md,
          ...(parsed.anchor ? { anchor: parsed.anchor } : {}),
        }, mutationOptions);
        const thread = parseAppliedThread(response);
        if (!thread) throw new RepoRouteError("error");
        return { thread };
      }
      case "comment": {
        const response = await edgePost(
          `${base}/threads/${seg(parsed.threadId)}/comments`,
          { body_md: parsed.body_md },
          mutationOptions,
        );
        const comment = parseAppliedComment(response, "git.pr.comment.create");
        if (!comment) throw new RepoRouteError("error");
        return { comment };
      }
      case "resolve": {
        const response = await edgePost(
          `${base}/threads/${seg(parsed.threadId)}/resolve`,
          { resolved: parsed.resolved },
          mutationOptions,
        );
        const resolution = parseAppliedThreadResolution(response);
        if (!resolution || resolution.thread_id !== parsed.threadId ||
            resolution.resolved !== parsed.resolved) throw new RepoRouteError("error");
        return { ok: true };
      }
      case "review-start": {
        const response = await edgePost(`${base}/reviews/start`, {}, mutationOptions);
        const review = parseAppliedReview(response);
        if (!review) throw new RepoRouteError("error");
        return { review };
      }
      case "review-comment": {
        const response = await edgePost(
          `${base}/reviews/${seg(parsed.reviewId)}/comments`,
          { body_md: parsed.body_md },
          mutationOptions,
        );
        const comment = parseAppliedComment(response, "git.pr.review.comment");
        if (!comment) throw new RepoRouteError("error");
        return { comment };
      }
      case "review-submit": {
        const response = await edgePost(
          `${base}/reviews/${seg(parsed.reviewId)}/submit`,
          { verdict: parsed.verdict, summary_md: parsed.summary_md },
          mutationOptions,
        );
        if (!hasAppliedAction(response, "git.pr.review.submit")) throw new RepoRouteError("error");
        return { ok: true };
      }
      case "review-discard": {
        const response = await edgePost(`${base}/reviews/${seg(parsed.reviewId)}/discard`, {}, mutationOptions);
        if (!hasAppliedAction(response, "git.pr.review.discard")) throw new RepoRouteError("error");
        return { ok: true };
      }
      case "merge": {
        try {
          const response = await edgePost(
            `${base}/merge`,
            {},
            mutationOptions,
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
