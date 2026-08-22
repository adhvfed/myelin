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

/** Build a file route without letting nested file paths become repository or ref segments. */
export function gitBlobPath(repo: string, refName: string, path: string): string {
  const nestedPath = path.split("/").map(encodeURIComponent).join("/");
  return `${gitRepositoryPath(repo)}/blob/${encodeURIComponent(refName)}/${nestedPath}`;
}

/** Parse the canonical positive integer coordinate supported by the browser client. */
export function parseGitPullRequestRouteParam(value: unknown): number | null {
  return parseGitPullRequestNumberText(value);
}
