import type { GitRepoListInput } from "./git-read-input";
import { parseGitRepoListInput } from "./git-read-input";

/** Convert router search values without silently repairing malformed or duplicate coordinates. */
export function repoListInputFromSearch(
  limit: unknown,
  cursor: unknown,
): GitRepoListInput | null {
  const raw: Record<string, unknown> = {};
  if (limit !== undefined) {
    raw.limit = typeof limit === "string" && /^(?:[1-9]|[1-9][0-9]|100)$/.test(limit)
      ? Number(limit)
      : limit;
  }
  if (cursor !== undefined) raw.cursor = cursor;
  return parseGitRepoListInput(raw);
}

export function repoListHref(input: GitRepoListInput): string {
  const query = new URLSearchParams();
  if (input.limit !== undefined) query.set("limit", String(input.limit));
  if (input.cursor !== undefined) query.set("cursor", input.cursor);
  const encoded = query.toString();
  return `/git/repos${encoded ? `?${encoded}` : ""}`;
}
