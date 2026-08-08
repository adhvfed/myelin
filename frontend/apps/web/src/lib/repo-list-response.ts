import type { RepoListPage, RepoListRowVM } from "./api";
import { isRepoListCursor } from "./git-read-input";

const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).every((key) => allowed.has(key));
}

function bounded(value: unknown, maximum: number): value is string {
  return typeof value === "string" && utf8.encode(value).byteLength <= maximum;
}

function safeCloneUrl(value: string): boolean {
  return !/[\p{Cc}\s]/u.test(value);
}

function repoSlug(value: unknown): value is string {
  return bounded(value, 255) && value.length > 0 && value.split("/").every((part) =>
    part !== "" && part !== "." && part !== ".." && /^[A-Za-z0-9._-]+$/.test(part)
  );
}

function row(value: unknown): RepoListRowVM | null {
  const item = record(value);
  if (!item) return null;
  if (item.state === "restricted") return { state: "restricted" };
  if (item.state === "empty" && exact(item, ["state", "slug"]) && repoSlug(item.slug)) {
    return { state: "empty", slug: item.slug };
  }
  if (item.state === "populated" && exact(item, ["state", "slug", "clone_url"]) &&
      repoSlug(item.slug) && bounded(item.clone_url, 4 * 1024) &&
      item.clone_url.length > 0 && safeCloneUrl(item.clone_url)) {
    return { state: "populated", slug: item.slug, clone_url: item.clone_url };
  }
  return null;
}

/** Decode only the summary catalogue contract; RepoHome fields are deliberately not accepted here. */
export function parseRepoListPage(value: unknown): RepoListPage | null {
  const envelope = record(value);
  const page = record(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) ||
      !page || !exact(page, ["next_cursor", "limit"]) ||
      !Number.isSafeInteger(page.limit) || (page.limit as number) < 1 ||
      (page.limit as number) > 100 || envelope.items.length > (page.limit as number) ||
      (page.next_cursor !== null && !isRepoListCursor(page.next_cursor))) return null;
  const items = envelope.items.map(row);
  return items.every((item): item is RepoListRowVM => item !== null)
    ? { items, page: { next_cursor: page.next_cursor as string | null, limit: page.limit as number } }
    : null;
}
