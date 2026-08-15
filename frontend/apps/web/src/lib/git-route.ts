import {
  isGitRepositorySlug,
  parseGitPullRequestNumberText,
} from "./git-coordinate";

/** Decode the router's single encoded repository segment exactly once. */
export function parseGitRepositoryRouteParam(value: unknown): string | null {
  if (typeof value !== "string" || value.length === 0) return null;
  try {
    const repo = decodeURIComponent(value);
    return isGitRepositorySlug(repo) ? repo : null;
  } catch {
    return null;
  }
}

/** Build the stable browser path for a validated repository slug. */
export function gitRepositoryPath(repo: string): string {
  return `/git/repos/${encodeURIComponent(repo)}`;
}

/** Parse the canonical positive integer coordinate supported by the browser client. */
export function parseGitPullRequestRouteParam(value: unknown): number | null {
  return parseGitPullRequestNumberText(value);
}
