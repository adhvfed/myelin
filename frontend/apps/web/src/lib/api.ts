// The data layer: SolidStart `query`s that call the edge THROUGH the server-side gateway client. The
// `"use server"` directive keeps the gateway + token strictly server-side. On `Unauthorized` (a 401
// that survived the single refresh + retry) the query throws a `/login` redirect — the canon's
// 401→/login behaviour, applied centrally.
import { query, redirect } from "@solidjs/router";
import { edgeGet, Unauthorized } from "../server/gateway";

/** A tree entry in a populated repo (RepoHome::to_json, crates/myelin-git/src/web.rs). */
export interface RepoEntry {
  path: string;
  is_dir: boolean;
}

/** The Git RepoHome ViewModel as the edge projects it (populated / empty / restricted). */
export interface RepoHomeVM {
  state: "populated" | "empty" | "restricted";
  slug?: string;
  readme_excerpt?: string;
  clone_url?: string;
  entries?: RepoEntry[];
}

/** The MR-014 uniform list envelope `{ items, page }`. */
export interface ReposPage {
  items: RepoHomeVM[];
  page: { next_cursor: string | null; limit: number };
}

/** The single-file view ViewModel (WebEditForm::to_json). The commit/edit composer is GT-004b/GF-6. */
export interface BlobVM {
  path: string;
  contents: string;
  base_oid: string;
  viewer_may_edit: boolean;
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
  page: { next_cursor: string | null; limit: number };
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

/** Run an edge GET through the gateway, mapping a surviving 401 to the `/login` redirect. */
async function authed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (e) {
    if (e instanceof Unauthorized) throw redirect("/login");
    throw e;
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

/** A single file at a ref (GET /v1/git/repos/{repo}/blob/{ref}/{path}). Single path segment (v1). */
export const getBlob = query(
  async (input: { repo: string; ref: string; path: string }): Promise<BlobVM> => {
    "use server";
    return authed(() =>
      edgeGet<BlobVM>(`/v1/git/repos/${seg(input.repo)}/blob/${seg(input.ref)}/${seg(input.path)}`),
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
