const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export interface RepoCreateReceipt {
  slug: string;
  created: boolean;
}

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

export function parseRepositorySlug(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const slug = value.trim();
  if (!slug || utf8.encode(slug).byteLength > 255) return null;
  return slug.split("/").every((part) =>
    part !== "" && part !== "." && part !== ".." && /^[A-Za-z0-9._-]+$/.test(part)
  ) ? slug : null;
}

export function repositorySlugError(value: string): string | null {
  if (!value.trim()) return "Enter a repository name.";
  if (utf8.encode(value.trim()).byteLength > 255) return "Use at most 255 UTF-8 bytes.";
  return parseRepositorySlug(value)
    ? null
    : "Use letters, numbers, dots, dashes, underscores, and optional namespace slashes.";
}

export function parseRepoCreateReceipt(
  value: unknown,
  expectedSlug: string,
): RepoCreateReceipt | null {
  const response = record(value);
  const applied = record(response?.applied);
  if (!response || !applied || response.durable !== true ||
      typeof response.created !== "boolean" ||
      applied.action !== "git.repo.create" || applied.slug !== expectedSlug) return null;
  return { slug: expectedSlug, created: response.created };
}
