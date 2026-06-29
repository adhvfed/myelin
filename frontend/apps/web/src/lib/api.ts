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

/** The repos screen's data: GET /v1/git/repos through the gateway → the edge ViewModel JSON. */
export const getRepos = query(async (): Promise<ReposPage> => {
  "use server";
  try {
    return await edgeGet<ReposPage>("/v1/git/repos");
  } catch (e) {
    if (e instanceof Unauthorized) throw redirect("/login");
    throw e;
  }
}, "git-repos");
