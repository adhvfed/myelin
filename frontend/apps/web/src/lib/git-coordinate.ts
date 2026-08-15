const utf8 = new TextEncoder();

export const MAX_GIT_REPOSITORY_SLUG_BYTES = 255;

/** The repository grammar shared by browser routes, server actions, and Edge response codecs. */
export function isGitRepositorySlug(value: unknown): value is string {
  if (typeof value !== "string" || value.length === 0 ||
      utf8.encode(value).byteLength > MAX_GIT_REPOSITORY_SLUG_BYTES) return false;
  const parts = value.split("/");
  return parts.every((part) => part !== "" && part !== "." && part !== ".." &&
    /^[A-Za-z0-9._-]+$/.test(part)) &&
    !parts.slice(0, -1).some((part) => part.toLowerCase().endsWith(".git"));
}

/** Normalize a repository name only at the human text-entry boundary. */
export function normalizeGitRepositorySlug(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const slug = value.trim();
  return isGitRepositorySlug(slug) ? slug : null;
}

/** A PR number that survives a JSON number and a browser route without losing identity. */
export function isGitPullRequestNumber(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > 0;
}

/** Parse the canonical decimal representation used in browser paths. */
export function parseGitPullRequestNumberText(value: unknown): number | null {
  if (typeof value !== "string" || !/^[1-9][0-9]*$/.test(value)) return null;
  const number = Number(value);
  return isGitPullRequestNumber(number) ? number : null;
}
