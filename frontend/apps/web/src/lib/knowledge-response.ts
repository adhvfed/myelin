import { BLOCK_TYPES, type BlockType, type EditorBlock } from "@myelin/design-system";

type WireRecord = Record<string, unknown>;
const utf8 = new TextEncoder();

export type KnowledgeVisibility = "private" | "team";
export type KnowledgeTextState = "active" | "tombstoned";

export interface KnowledgePageSummary {
  id: string;
  ref: string;
  space: string;
  parent_page_id: string | null;
  title: string;
  title_state: KnowledgeTextState;
  visibility: KnowledgeVisibility;
  version: number;
  can_edit: boolean;
  created_at: number;
  updated_at: number;
}

export interface KnowledgeBlock extends EditorBlock {
  id: string;
  references: string[];
  state: KnowledgeTextState;
  is_you: boolean;
}

export interface KnowledgePage extends KnowledgePageSummary {
  blocks: KnowledgeBlock[];
}

export interface KnowledgePageList {
  items: KnowledgePageSummary[];
  page: { next_cursor: string | null; limit: number };
}

export interface KnowledgeCreateReceipt { page: KnowledgePage; created: boolean; durable: true }
export interface KnowledgeSaveReceipt { page: KnowledgePage; version: number; durable: true }

export interface KnowledgeCreateDraft {
  title: string;
  template: "blank" | "product-spec" | "runbook";
  visibility: KnowledgeVisibility;
  clientNonce: string;
}

export interface KnowledgeSaveDraft {
  pageId: string;
  expectedVersion: number;
  title: string;
  visibility: KnowledgeVisibility;
  blocks: Array<EditorBlock & { references?: string[] }>;
}

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as WireRecord : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).length === keys.length && Object.keys(value).every((key) => allowed.has(key));
}

export function isKnowledgeUlid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9A-HJKMNP-TV-Z]{26}$/.test(value);
}

function cleanText(value: unknown, maximum: number, empty = false): value is string {
  return typeof value === "string" && (empty || value.length > 0) && utf8.encode(value).byteLength <= maximum &&
    ![...value].some((character) => character === "\0");
}

function artifactRef(value: unknown): value is string {
  return cleanText(value, 4 * 1024) && /^myelin:\/\/[^/]+\/[^/]+\/[^/]+\/[^#]+(?:#.+)?$/.test(value);
}

function pageRef(value: unknown, id: string): value is string {
  return artifactRef(value) && new RegExp(`^myelin://[^/]+/knowledge/page/${id}$`).test(value);
}

function references(value: unknown): string[] | null {
  if (!Array.isArray(value) || value.length > 32 || !value.every(artifactRef)) return null;
  return value as string[];
}

function summary(value: unknown): KnowledgePageSummary | null {
  const row = record(value);
  const keys = ["id", "ref", "space", "parent_page_id", "title", "title_state", "visibility", "version", "can_edit", "created_at", "updated_at"];
  if (!row || !exact(row, keys) || !isKnowledgeUlid(row.id) || !pageRef(row.ref, row.id) || !cleanText(row.space, 64) ||
      (row.parent_page_id !== null && !isKnowledgeUlid(row.parent_page_id)) || !cleanText(row.title, 512) ||
      !["active", "tombstoned"].includes(row.title_state as string) ||
      !["private", "team"].includes(row.visibility as string) || !Number.isSafeInteger(row.version) ||
      (row.version as number) < 1 || typeof row.can_edit !== "boolean" || !Number.isSafeInteger(row.created_at) ||
      !Number.isSafeInteger(row.updated_at)) return null;
  return row as unknown as KnowledgePageSummary;
}

function block(value: unknown): KnowledgeBlock | null {
  const row = record(value);
  const refs = references(row?.references);
  if (!row || !exact(row, ["id", "type", "markdown", "references", "state", "is_you"]) || !refs || !isKnowledgeUlid(row.id) ||
      !BLOCK_TYPES.includes(row.type as BlockType) || !cleanText(row.markdown, 64 * 1024, true) ||
      !["active", "tombstoned"].includes(row.state as string) || typeof row.is_you !== "boolean" ||
      (row.state === "tombstoned" && row.markdown !== "") ||
      [...(row.markdown as string)].filter((character) => character === "\uFFFC").length !== refs.length) return null;
  return row as unknown as KnowledgeBlock;
}

function paging(value: unknown): KnowledgePageList["page"] | null {
  const row = record(value);
  if (!row || !exact(row, ["next_cursor", "limit"]) ||
      (row.next_cursor !== null && !isKnowledgeUlid(row.next_cursor)) || !Number.isSafeInteger(row.limit) ||
      (row.limit as number) < 1 || (row.limit as number) > 100) return null;
  return row as unknown as KnowledgePageList["page"];
}

export function parseKnowledgePages(value: unknown): KnowledgePageList | null {
  const envelope = record(value);
  const page = paging(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) || !page ||
      envelope.items.length > page.limit) return null;
  const items = envelope.items.map(summary);
  return items.every((item): item is KnowledgePageSummary => item !== null) ? { items, page } : null;
}

function document(value: unknown): KnowledgePage | null {
  const row = record(value);
  if (!row || !Array.isArray(row.blocks)) return null;
  const base = summary(Object.fromEntries(Object.entries(row).filter(([key]) => key !== "blocks")));
  const blocks = row.blocks.map(block);
  return base && blocks.length >= 1 && blocks.length <= 500 && blocks.every((item): item is KnowledgeBlock => item !== null)
    ? { ...base, blocks }
    : null;
}

export function parseKnowledgePage(value: unknown): KnowledgePage | null {
  const envelope = record(value);
  const page = document(envelope?.page);
  return envelope && exact(envelope, ["page"]) && page ? page : null;
}

export function parseKnowledgeCreateReceipt(value: unknown): KnowledgeCreateReceipt | null {
  const receipt = record(value);
  const page = document(receipt?.page);
  return receipt && exact(receipt, ["page", "created", "durable"]) && typeof receipt.created === "boolean" &&
    receipt.durable === true && page ? { page, created: receipt.created, durable: true } : null;
}

export function parseKnowledgeSaveReceipt(value: unknown): KnowledgeSaveReceipt | null {
  const receipt = record(value);
  const page = document(receipt?.page);
  return receipt && exact(receipt, ["page", "version", "durable"]) && Number.isSafeInteger(receipt.version) &&
    receipt.version === page?.version && receipt.durable === true && page
    ? { page, version: receipt.version as number, durable: true }
    : null;
}

export function parseKnowledgeCreateDraft(value: unknown): KnowledgeCreateDraft | null {
  const row = record(value);
  if (!row || !exact(row, ["title", "template", "visibility", "clientNonce"]) || !cleanText(row.title, 512) ||
      (row.title as string).trim() !== row.title || !["blank", "product-spec", "runbook"].includes(row.template as string) ||
      !["private", "team"].includes(row.visibility as string) || typeof row.clientNonce !== "string" ||
      !/^[A-Za-z0-9_-]{1,128}$/.test(row.clientNonce)) return null;
  return row as unknown as KnowledgeCreateDraft;
}

export function parseKnowledgeSaveDraft(value: unknown): KnowledgeSaveDraft | null {
  const row = record(value);
  if (!row || !exact(row, ["pageId", "expectedVersion", "title", "visibility", "blocks"]) ||
      !isKnowledgeUlid(row.pageId) || !Number.isSafeInteger(row.expectedVersion) || (row.expectedVersion as number) < 1 ||
      !cleanText(row.title, 512) || (row.title as string).trim() !== row.title ||
      !["private", "team"].includes(row.visibility as string) || !Array.isArray(row.blocks) ||
      row.blocks.length < 1 || row.blocks.length > 500) return null;
  let total = 0;
  let totalReferences = 0;
  const blocks = row.blocks.map((value) => {
    const item = record(value);
    const refs = item?.references === undefined ? [] : references(item.references);
    if (!item || !refs || Object.keys(item).some((key) => !["id", "type", "markdown", "references", "state"].includes(key)) ||
        (item.id !== undefined && !isKnowledgeUlid(item.id)) || !BLOCK_TYPES.includes(item.type as BlockType) ||
        !cleanText(item.markdown, 64 * 1024, true) ||
        (item.state !== undefined && !["active", "tombstoned"].includes(item.state as string)) ||
        (item.state === "tombstoned" && (item.id === undefined || item.markdown !== "")) ||
        [...(item.markdown as string)].filter((character) => character === "\uFFFC").length !== refs.length) return null;
    total += utf8.encode(item.markdown as string).byteLength;
    totalReferences += refs.length;
    return { ...item, ...(item.references === undefined ? {} : { references: refs }) } as unknown as EditorBlock & { references?: string[] };
  });
  return blocks.every((item): item is EditorBlock & { references?: string[] } => item !== null) &&
      total <= 256 * 1024 && totalReferences <= 100
    ? { pageId: row.pageId, expectedVersion: row.expectedVersion as number, title: row.title, visibility: row.visibility, blocks } as KnowledgeSaveDraft
    : null;
}
