export const RELATED_REFS_PAGE_LIMIT = 100;

export interface RelatedRefItem {
  ref: string;
  root_ref: string;
  source_ref: string;
  source_root_ref: string;
  target_ref: string;
  target_root_ref: string;
  relation: "mentions" | "embeds" | "links" | "closes" | "blocks" | "blocked_by" |
    "depends_on" | "parent" | "child" | "assigns" | "relates";
  relation_class: "reference" | "lifecycle";
  origin_actor: string;
}

export interface RelatedRefsPage {
  ref: string;
  root_ref: string;
  items: RelatedRefItem[];
  page: { next_cursor: string | null; limit: number };
}

type WireRecord = Record<string, unknown>;
const utf8 = new TextEncoder();
const ITEM_KEYS = [
  "ref", "root_ref", "source_ref", "source_root_ref", "target_ref", "target_root_ref",
  "relation", "relation_class", "origin_actor",
] as const;
const RELATIONS = [
  "mentions", "embeds", "links", "closes", "blocks", "blocked_by", "depends_on", "parent",
  "child", "assigns", "relates",
] as const;

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const expected = new Set(keys);
  return Object.keys(value).length === keys.length && Object.keys(value).every((key) => expected.has(key));
}

export function isArtifactRef(value: unknown): value is string {
  return typeof value === "string" && utf8.encode(value).byteLength <= 4 * 1024 &&
    /^myelin:\/\/[^/]+\/[^/]+\/[^/]+\/[^#]+(?:#.+)?$/.test(value) &&
    ![...value].some((character) => character.charCodeAt(0) <= 0x20 || character.charCodeAt(0) === 0x7f);
}

function rootRef(value: unknown): value is string {
  return isArtifactRef(value) && !value.includes("#");
}

function cursor(value: unknown): value is string | null {
  return value === null || typeof value === "string" && (
    /^[0-9a-f]{32}$/.test(value) || /^blake3:[0-9a-f]{64}$/.test(value)
  );
}

function relatedItem(value: unknown): RelatedRefItem | null {
  const item = record(value);
  if (!item || !exact(item, ITEM_KEYS) || !isArtifactRef(item.ref) || !rootRef(item.root_ref) ||
      !isArtifactRef(item.source_ref) || !rootRef(item.source_root_ref) ||
      !isArtifactRef(item.target_ref) || !rootRef(item.target_root_ref) ||
      !RELATIONS.includes(item.relation as RelatedRefItem["relation"]) ||
      !["reference", "lifecycle"].includes(item.relation_class as string) ||
      typeof item.origin_actor !== "string" || item.origin_actor.length === 0 ||
      utf8.encode(item.origin_actor).byteLength > 4 * 1024) return null;
  return item as unknown as RelatedRefItem;
}

export function parseRelatedRefsPage(value: unknown): RelatedRefsPage | null {
  const envelope = record(value);
  const page = record(envelope?.page);
  if (!envelope || !exact(envelope, ["ref", "root_ref", "items", "page"]) ||
      !isArtifactRef(envelope.ref) || !rootRef(envelope.root_ref) || !Array.isArray(envelope.items) ||
      envelope.items.length > RELATED_REFS_PAGE_LIMIT || !page ||
      !exact(page, ["next_cursor", "limit"]) || !cursor(page.next_cursor) ||
      page.limit !== RELATED_REFS_PAGE_LIMIT) return null;
  const items = envelope.items.map(relatedItem);
  return items.every((item): item is RelatedRefItem => item !== null)
    ? {
      ref: envelope.ref,
      root_ref: envelope.root_ref,
      items,
      page: { next_cursor: page.next_cursor, limit: RELATED_REFS_PAGE_LIMIT },
    }
    : null;
}
