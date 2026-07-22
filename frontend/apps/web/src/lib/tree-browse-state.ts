import type { RepoHomeVM, TreeVM } from "./api";
import type { GitTreeInput } from "./git-read-input";

export const TREE_PAGE_LIMIT = 100;
export const TREE_SEARCH_DEBOUNCE_MS = 200;
const utf8 = new TextEncoder();

export interface TreeLocation {
  repo: string;
  ref: string;
  path: string;
  limit?: number;
  cursor?: string;
  q?: string;
}

export type TreeFetcher = (input: GitTreeInput) => Promise<TreeVM>;

export class InitialTreeReader {
  private key: string | undefined;
  private request: Promise<TreeVM> | undefined;

  constructor(private readonly fetchTree: TreeFetcher) {}

  read(coordinates: Pick<GitTreeInput, "repo" | "ref" | "path">): Promise<TreeVM> {
    const key = `${coordinates.repo}\0${coordinates.ref}\0${coordinates.path}`;
    if (this.key === key && this.request) return this.request;
    this.key = key;
    const request = this.fetchTree({ ...coordinates, limit: TREE_PAGE_LIMIT });
    this.request = request;
    void request.catch(() => {
      if (this.request === request) this.request = undefined;
    });
    return request;
  }
}

function nestedPath(path: string): string {
  return path.split("/").filter(Boolean).map(encodeURIComponent).join("/");
}

export function treeHref(location: TreeLocation): string {
  const tail = nestedPath(location.path);
  const base = `/git/repos/${encodeURIComponent(location.repo)}/tree/${encodeURIComponent(location.ref)}`;
  const query = new URLSearchParams();
  if (location.limit !== undefined && location.limit !== TREE_PAGE_LIMIT) {
    query.set("limit", String(location.limit));
  }
  if (location.cursor) query.set("cursor", location.cursor);
  if (location.q) query.set("q", location.q);
  const encoded = query.toString();
  return `${base}${tail ? `/${tail}` : ""}${encoded ? `?${encoded}` : ""}`;
}

export function treeReloadHref(location: TreeLocation): string {
  return treeHref({ ...location, cursor: undefined });
}

export function treeSearchValue(value: unknown): string {
  return typeof value === "string" && utf8.encode(value).byteLength <= 256 &&
    !/\p{Cc}/u.test(value) ? value : "";
}

export function treeCursorValue(value: unknown): string | undefined {
  return typeof value === "string" && utf8.encode(value).byteLength <= 8 * 1024 &&
    /^gt1_[A-Za-z0-9_-]+$/.test(value) ? value : undefined;
}

export function treeLimitValue(value: unknown): number {
  if (typeof value !== "string" || !/^(?:[1-9]|[1-9][0-9]|100)$/.test(value)) {
    return TREE_PAGE_LIMIT;
  }
  return Number(value);
}

export function repoHomeContinuationHref(
  repo: string,
  page: NonNullable<RepoHomeVM["entries_page"]>,
): string | null {
  if (!page.next_cursor) return null;
  return treeHref({
    repo,
    ref: page.ref,
    path: "",
    limit: page.limit,
    cursor: page.next_cursor,
  });
}
