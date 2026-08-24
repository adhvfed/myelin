import { isStorableArtifactRef } from "./artifact-ref";
import { isClientNonce } from "./client-nonce";
import { isProjectId } from "./project-contract";

const utf8 = new TextEncoder();
type WireRecord = Record<string, unknown>;

export interface ChatConversation {
  id: string;
  ref: string;
  kind: "channel_public" | "channel_private";
  project_id: string | null;
  channel: string;
  topic: string;
  linked_ref: string | null;
  pinned_canvas: string | null;
  retention_days: number | null;
}

export interface ChatConversationPage {
  items: ChatConversation[];
  page: { next_cursor: string | null; limit: number };
}

export type ChatAuthorKind = "human" | "agent" | "service";
export type ChatMessageState = "active" | "edited" | "deleted" | "tombstoned";
export type ChatReferenceCard =
  | {
    kind: "projection";
    title: string;
    state: string;
    icon: string;
    render_hint: string;
    sub_anchor: string | null;
    flag: "moved" | "outdated" | null;
  }
  | { kind: "reference" }
  | { kind: "tombstone" };
export type ChatMessageNode =
  | { kind: "mention"; principal_id: string }
  | { kind: "artifact_ref" | "embed"; ref: string; card: ChatReferenceCard };

export interface ChatMessage {
  id: string;
  author: string;
  author_kind: ChatAuthorKind;
  is_you: boolean;
  content: string;
  nodes: ChatMessageNode[];
  edited: boolean;
  state: ChatMessageState;
  created_at: number | null;
}

export interface ChatMessagePage {
  conversation: ChatConversation;
  items: ChatMessage[];
  page: { next_cursor: string | null; limit: number };
}

export interface ChatConversationReceipt {
  conversation: ChatConversation;
  durable: true;
}

export interface ChatMessageReceipt {
  message_id: string;
  durable: true;
}

export interface ChatConversationDraft {
  projectId: string;
  channel: string;
  topic: string;
  clientNonce: string;
}

export interface ChatMessageDraft {
  conversationId: string;
  content: string;
  references: string[];
  clientNonce: string;
}

function record(value: unknown): WireRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as WireRecord
    : null;
}

function exact(value: WireRecord, keys: readonly string[]): boolean {
  const allowed = new Set(keys);
  return Object.keys(value).length === keys.length &&
    Object.keys(value).every((key) => allowed.has(key));
}

function cleanText(value: unknown, maximum: number, allowEmpty = false): value is string {
  return typeof value === "string" && (allowEmpty || value.length > 0) &&
    utf8.encode(value).byteLength <= maximum &&
    ![...value].some((character) => {
      const point = character.codePointAt(0)!;
      return point === 0 || point === 0x7f || (point < 0x20 && character !== "\n" && character !== "\t");
    });
}

export function isChatUlid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9A-HJKMNP-TV-Z]{26}$/.test(value);
}

function nullableText(value: unknown, maximum: number): value is string | null {
  return value === null || cleanText(value, maximum);
}

function identity(value: unknown): value is string {
  return cleanText(value, 512) && ![...value].some((character) => /\s/u.test(character));
}

function conversation(value: unknown): ChatConversation | null {
  const row = record(value);
  if (!row || !exact(row, [
    "id", "ref", "kind", "project_id", "channel", "topic", "linked_ref", "pinned_canvas",
    "retention_days",
  ]) || !isChatUlid(row.id) || !cleanText(row.ref, 4 * 1024) ||
      !["channel_public", "channel_private"].includes(row.kind as string) ||
      (row.project_id !== null && !isProjectId(row.project_id)) ||
      (row.kind === "channel_public" && row.project_id === null) ||
      !cleanText(row.channel, 255) || !cleanText(row.topic, 255) ||
      !nullableText(row.linked_ref, 4 * 1024) || !nullableText(row.pinned_canvas, 4 * 1024) ||
      (row.retention_days !== null &&
        (!Number.isSafeInteger(row.retention_days) || (row.retention_days as number) < 1))) {
    return null;
  }
  return row as unknown as ChatConversation;
}

function page(value: unknown): { next_cursor: string | null; limit: number } | null {
  const row = record(value);
  if (!row || !exact(row, ["next_cursor", "limit"]) ||
      (row.next_cursor !== null && !isChatUlid(row.next_cursor)) ||
      !Number.isSafeInteger(row.limit) || (row.limit as number) < 1 || (row.limit as number) > 100) {
    return null;
  }
  return { next_cursor: row.next_cursor as string | null, limit: row.limit as number };
}

export function parseChatConversations(value: unknown): ChatConversationPage | null {
  const envelope = record(value);
  const paging = page(envelope?.page);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) ||
      !paging || envelope.items.length > paging.limit) return null;
  const items = envelope.items.map(conversation);
  return items.every((item): item is ChatConversation => item !== null)
    ? { items, page: paging }
    : null;
}

function messageNode(value: unknown): ChatMessageNode | null {
  const node = record(value);
  if (!node || typeof node.kind !== "string") return null;
  if (node.kind === "mention") {
    return exact(node, ["kind", "principal_id"]) && identity(node.principal_id)
      ? { kind: "mention", principal_id: node.principal_id }
      : null;
  }
  if (node.kind === "artifact_ref" || node.kind === "embed") {
    const card = referenceCard(node.card);
    return exact(node, ["kind", "ref", "card"]) && isStorableArtifactRef(node.ref) && card
      ? { kind: node.kind, ref: node.ref, card }
      : null;
  }
  return null;
}

function referenceCard(value: unknown): ChatReferenceCard | null {
  const card = record(value);
  if (!card || typeof card.kind !== "string") return null;
  if (card.kind === "reference" || card.kind === "tombstone") {
    return exact(card, ["kind"]) ? { kind: card.kind } : null;
  }
  if (card.kind !== "projection" || !exact(card, [
    "kind", "title", "state", "icon", "render_hint", "sub_anchor", "flag",
  ]) || !cleanText(card.title, 512) || !cleanText(card.state, 255) ||
      !cleanText(card.icon, 64) || !cleanText(card.render_hint, 64) ||
      !nullableText(card.sub_anchor, 1024) ||
      ![null, "moved", "outdated"].includes(card.flag as string | null)) return null;
  return card as unknown as ChatReferenceCard;
}

function message(value: unknown): ChatMessage | null {
  const row = record(value);
  if (!row || !exact(row, [
    "id", "author", "author_kind", "is_you", "content", "nodes", "edited", "state", "created_at",
  ]) || !isChatUlid(row.id) ||
      (typeof row.author !== "string" || !/^chat-author:[0-9a-f]{32}$/.test(row.author)) ||
      !["human", "agent", "service"].includes(row.author_kind as string) ||
      typeof row.is_you !== "boolean" || !cleanText(row.content, 32 * 1024, true) ||
      !Array.isArray(row.nodes) || row.nodes.length > 32 ||
      typeof row.edited !== "boolean" ||
      !["active", "edited", "deleted", "tombstoned"].includes(row.state as string) ||
      (row.created_at !== null &&
        (!Number.isSafeInteger(row.created_at) || (row.created_at as number) < 0))) return null;
  const nodes = row.nodes.map(messageNode);
  if (!nodes.every((node): node is ChatMessageNode => node !== null) ||
      [...row.content].filter((character) => character === "\uFFFC").length !== nodes.length) {
    return null;
  }
  return { ...row, nodes } as unknown as ChatMessage;
}

export function parseChatMessages(value: unknown): ChatMessagePage | null {
  const envelope = record(value);
  const subject = conversation(envelope?.conversation);
  const paging = page(envelope?.page);
  if (!envelope || !exact(envelope, ["conversation", "items", "page"]) || !subject ||
      !Array.isArray(envelope.items) || !paging || envelope.items.length > paging.limit) return null;
  const items = envelope.items.map(message);
  return items.every((item): item is ChatMessage => item !== null)
    ? { conversation: subject, items, page: paging }
    : null;
}

export function parseChatConversationReceipt(value: unknown): ChatConversationReceipt | null {
  const receipt = record(value);
  const subject = conversation(receipt?.conversation);
  return receipt && exact(receipt, ["conversation", "durable"]) && receipt.durable === true && subject
    ? { conversation: subject, durable: true }
    : null;
}

export function parseChatMessageReceipt(value: unknown): ChatMessageReceipt | null {
  const receipt = record(value);
  return receipt && exact(receipt, ["message_id", "durable"]) &&
    receipt.durable === true && isChatUlid(receipt.message_id)
    ? { message_id: receipt.message_id, durable: true }
    : null;
}

export function parseChatConversationDraft(value: unknown): ChatConversationDraft | null {
  const draft = record(value);
  if (!draft || !exact(draft, ["projectId", "channel", "topic", "clientNonce"]) ||
      !isProjectId(draft.projectId) || !cleanText(draft.channel, 255) ||
      !cleanText(draft.topic, 255) ||
      draft.channel.trim() !== draft.channel || draft.topic.trim() !== draft.topic ||
      !isClientNonce(draft.clientNonce)) return null;
  return {
    projectId: draft.projectId,
    channel: draft.channel,
    topic: draft.topic,
    clientNonce: draft.clientNonce,
  };
}

export function parseChatMessageDraft(value: unknown): ChatMessageDraft | null {
  const draft = record(value);
  if (!draft || !exact(draft, ["conversationId", "content", "references", "clientNonce"]) ||
      !isChatUlid(draft.conversationId) || !cleanText(draft.content, 32 * 1024) ||
      !draft.content.trim() || !Array.isArray(draft.references) || draft.references.length > 32 ||
      !draft.references.every(isStorableArtifactRef) ||
      [...draft.content].filter((character) => character === "\uFFFC").length !== draft.references.length ||
      !isClientNonce(draft.clientNonce)) return null;
  return {
    conversationId: draft.conversationId,
    content: draft.content,
    references: draft.references,
    clientNonce: draft.clientNonce,
  };
}
