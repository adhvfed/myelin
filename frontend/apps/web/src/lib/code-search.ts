const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export interface CodeSearchInput {
  q: string;
  repo?: string;
}

export interface CodeSearchHit {
  repo: string;
  ref: string;
  snapshot_oid: string;
  path: string;
  line: number;
  excerpt: string;
}

export interface CodeSearchPage {
  items: CodeSearchHit[];
  page: { next_cursor: null; limit: number };
  complete: boolean;
}

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

function clean(value: unknown, maximum: number): value is string {
  return bounded(value, maximum) && !/\p{Cc}/u.test(value);
}

function repoSlug(value: unknown): value is string {
  return clean(value, 255) && value.length > 0 && value.split("/").every((part) =>
    part !== "" && part !== "." && part !== ".." && /^[A-Za-z0-9._-]+$/.test(part)
  );
}

export function parseCodeSearchInput(value: unknown): CodeSearchInput | null {
  const input = record(value);
  if (!input || !exact(input, ["q", "repo"]) || !clean(input.q, 4 * 1024) ||
      input.q.trim().length === 0 ||
      (input.repo !== undefined && !repoSlug(input.repo))) return null;
  return {
    q: input.q,
    ...(input.repo === undefined ? {} : { repo: input.repo }),
  };
}

export function codeSearchParams(input: CodeSearchInput): URLSearchParams {
  const params = new URLSearchParams({ q: input.q });
  if (input.repo) params.set("repo", input.repo);
  return params;
}

function parseHit(value: unknown): CodeSearchHit | null {
  const hit = record(value);
  if (!hit || !exact(hit, ["repo", "ref", "snapshot_oid", "path", "line", "excerpt"]) ||
      !repoSlug(hit.repo) || !clean(hit.ref, 4 * 1024) || hit.ref.length === 0 ||
      typeof hit.snapshot_oid !== "string" || !/^[0-9a-f]{40,64}$/.test(hit.snapshot_oid) ||
      !clean(hit.path, 4 * 1024) || hit.path.length === 0 || hit.path.startsWith("/") ||
      hit.path.includes("\\") || hit.path.split("/").some((part) => !part || part === "." || part === "..") ||
      !Number.isSafeInteger(hit.line) || (hit.line as number) < 1 ||
      !clean(hit.excerpt, 4 * 1024)) return null;
  return hit as unknown as CodeSearchHit;
}

export function parseCodeSearchPage(value: unknown): CodeSearchPage | null {
  const envelope = record(value);
  const page = record(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page", "complete"]) ||
      !Array.isArray(envelope.items) || typeof envelope.complete !== "boolean" ||
      !page || !exact(page, ["next_cursor", "limit"]) || page.next_cursor !== null ||
      !Number.isSafeInteger(page.limit) || (page.limit as number) < 1 ||
      (page.limit as number) > 100 || envelope.items.length > (page.limit as number)) return null;
  const items = envelope.items.map(parseHit);
  return items.every((item): item is CodeSearchHit => item !== null)
    ? {
        items,
        page: { next_cursor: null, limit: page.limit as number },
        complete: envelope.complete,
      }
    : null;
}

export function codeSearchHref(input: CodeSearchInput): string {
  return `/git/search?${codeSearchParams(input).toString()}`;
}

export function codeSearchHitHref(hit: CodeSearchHit): string {
  const path = hit.path.split("/").map(encodeURIComponent).join("/");
  return `/git/repos/${encodeURIComponent(hit.repo)}/blob/${encodeURIComponent(hit.ref)}/${path}#L${hit.line}`;
}
