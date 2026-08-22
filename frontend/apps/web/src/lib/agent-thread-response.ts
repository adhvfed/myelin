import { parseChatMessages, type ChatMessagePage } from "./chat-response";

const utf8 = new TextEncoder();
const UUID = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
type WireRecord = Record<string, unknown>;

export type AgentThreadState = "provisioning" | "ready" | "expiring" | "deleted" | "failed";

export interface AgentChoice {
  id: string;
  name: string;
  runtime_ref: "external:mcp" | "hosted:luna";
  status: "active" | "suspended" | "disabled";
}

export interface AgentChoicePage {
  items: AgentChoice[];
  page: { next_cursor: string | null; limit: number };
}

export interface AgentActivationReceipt {
  agent: AgentChoice;
  created: boolean;
  durable: true;
}

export interface AgentThread {
  id: string;
  ref: string;
  name: string;
  agent_id: string;
  agent_ref: string;
  project_id: string | null;
  conversation_id: string;
  conversation_ref: string;
  workspace: {
    id: string;
    generation: number;
    state: AgentThreadState;
    retention_days: number;
    expires_at: string;
  };
  created_at: string;
  updated_at: string;
}

export interface AgentThreadPage {
  items: AgentThread[];
  page: { next_cursor: string | null; limit: number };
}

export interface AgentThreadCreateReceipt {
  thread: AgentThread;
  created: boolean;
  durable: true;
}

export interface WorkspaceSession {
  id: string;
  ref: string;
  method: "ssh";
  mode: "shell" | "command";
  terminal: boolean;
  workspace: { id: string; generation: number };
  started_at: string;
}

export interface WorkspaceSessionPage {
  items: WorkspaceSession[];
  page: { next_cursor: string | null; limit: number };
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

function cleanText(value: unknown, maximum: number): value is string {
  return typeof value === "string" && value.length > 0 && utf8.encode(value).byteLength <= maximum &&
    ![...value].some((character) => character.codePointAt(0)! < 0x20 || character === "\u007f");
}

export function isAgentThreadId(value: unknown): value is string {
  return typeof value === "string" && UUID.test(value);
}

function isUlid(value: unknown): value is string {
  return typeof value === "string" && ULID.test(value);
}

function timestamp(value: unknown): value is string {
  return cleanText(value, 64) && Number.isFinite(Date.parse(value));
}

function positiveInteger(value: unknown, maximum = Number.MAX_SAFE_INTEGER): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 1 && (value as number) <= maximum;
}

function stringArray(value: unknown, maximum: number): value is string[] {
  return Array.isArray(value) && value.length <= maximum && value.every((item) => cleanText(item, 4_096));
}

function toolRows(value: unknown): boolean {
  return Array.isArray(value) && value.length <= 128 && value.every((item) => {
    const row = record(item);
    return row && exact(row, ["name", "version", "ref"]) && cleanText(row.name, 255) &&
      positiveInteger(row.version, 65_535) && cleanText(row.ref, 4_096);
  });
}

function agentChoice(value: unknown): AgentChoice | null {
  const row = record(value);
  if (!row || !exact(row, [
    "id", "ref", "principal_id", "name", "runtime_ref", "on_behalf_of", "status",
    "selected_tools", "effective_tools", "grants", "created_at",
  ]) || !isAgentThreadId(row.id) || !cleanText(row.ref, 4_096) ||
      !cleanText(row.principal_id, 512) || !cleanText(row.name, 80) ||
      !["external:mcp", "hosted:luna"].includes(row.runtime_ref as string) ||
      !cleanText(row.on_behalf_of, 512) ||
      !["active", "suspended", "disabled"].includes(row.status as string) ||
      !toolRows(row.selected_tools) || !toolRows(row.effective_tools) ||
      !stringArray(row.grants, 512) || !timestamp(row.created_at)) return null;
  return {
    id: row.id,
    name: row.name,
    runtime_ref: row.runtime_ref,
    status: row.status,
  } as AgentChoice;
}

function page(
  value: unknown,
  cursor: (value: unknown) => value is string,
): { next_cursor: string | null; limit: number } | null {
  const row = record(value);
  if (!row || !exact(row, ["next_cursor", "limit"]) ||
      (row.next_cursor !== null && !cursor(row.next_cursor)) || !positiveInteger(row.limit, 100)) {
    return null;
  }
  return { next_cursor: row.next_cursor as string | null, limit: row.limit as number };
}

export function parseAgentChoices(value: unknown): AgentChoicePage | null {
  const envelope = record(value);
  const paging = page(envelope?.page, isAgentThreadId);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) ||
      !paging || envelope.items.length > paging.limit) return null;
  const items = envelope.items.map(agentChoice);
  return items.every((item): item is AgentChoice => item !== null) ? { items, page: paging } : null;
}

export function parseAgentActivationReceipt(value: unknown): AgentActivationReceipt | null {
  const receipt = record(value);
  const agent = agentChoice(receipt?.agent);
  const governance = record(receipt?.governance);
  return receipt && exact(receipt, ["agent", "created", "durable", "governance"]) && agent &&
    governance && exact(governance, ["policy_versions", "policy_revisions"]) &&
    policyCoordinates(governance.policy_versions) && policyCoordinates(governance.policy_revisions) &&
    typeof receipt.created === "boolean" && receipt.durable === true
    ? { agent, created: receipt.created, durable: true }
    : null;
}

function policyCoordinates(value: unknown): boolean {
  const coordinates = record(value);
  return Boolean(coordinates && exact(coordinates, [
    "agent", "delegation", "tenant", "trigger_actor",
  ]) && positiveInteger(coordinates.agent) && positiveInteger(coordinates.delegation) &&
    positiveInteger(coordinates.tenant) && positiveInteger(coordinates.trigger_actor));
}

function thread(value: unknown): AgentThread | null {
  const row = record(value);
  const workspace = record(row?.workspace);
  if (!row || !exact(row, [
    "id", "ref", "name", "agent_id", "agent_ref", "project_id", "conversation_id",
    "conversation_ref", "workspace", "created_at", "updated_at",
  ]) || !workspace || !exact(workspace, [
    "id", "generation", "state", "retention_days", "expires_at",
  ]) || !isAgentThreadId(row.id) || !cleanText(row.ref, 4_096) || !cleanText(row.name, 80) ||
      !isAgentThreadId(row.agent_id) || !cleanText(row.agent_ref, 4_096) ||
      (row.project_id !== null && !isAgentThreadId(row.project_id)) ||
      !isUlid(row.conversation_id) || !cleanText(row.conversation_ref, 4_096) ||
      !isAgentThreadId(workspace.id) || !positiveInteger(workspace.generation, 4_294_967_295) ||
      !["provisioning", "ready", "expiring", "deleted", "failed"].includes(workspace.state as string) ||
      !positiveInteger(workspace.retention_days, 30) || !timestamp(workspace.expires_at) ||
      !timestamp(row.created_at) || !timestamp(row.updated_at)) return null;
  const thread = row as unknown as AgentThread;
  if (!thread.ref.endsWith(`/agent/thread/${thread.id}`) ||
      !thread.agent_ref.endsWith(`/identity/agent/${thread.agent_id}`) ||
      !thread.conversation_ref.endsWith(`/chat/channel/${thread.conversation_id}`) ||
      Date.parse(thread.created_at) > Date.parse(thread.updated_at) ||
      Date.parse(thread.created_at) > Date.parse(thread.workspace.expires_at)) return null;
  return thread;
}

export function parseAgentThreads(value: unknown): AgentThreadPage | null {
  const envelope = record(value);
  const paging = page(envelope?.page, isAgentThreadId);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) ||
      !paging || envelope.items.length > paging.limit) return null;
  const items = envelope.items.map(thread);
  return items.every((item): item is AgentThread => item !== null) ? { items, page: paging } : null;
}

export function parseAgentThread(value: unknown): AgentThread | null {
  const envelope = record(value);
  return envelope && exact(envelope, ["thread"]) ? thread(envelope.thread) : null;
}

export function parseAgentThreadCreateReceipt(value: unknown): AgentThreadCreateReceipt | null {
  const receipt = record(value);
  const createdThread = thread(receipt?.thread);
  return receipt && exact(receipt, ["thread", "created", "durable"]) &&
    typeof receipt.created === "boolean" && receipt.durable === true && createdThread
    ? { thread: createdThread, created: receipt.created, durable: true }
    : null;
}

export function parseAgentThreadMessages(value: unknown, threadId: string): ChatMessagePage | null {
  if (!isAgentThreadId(threadId)) return null;
  const messages = parseChatMessages(value);
  return messages?.conversation.kind === "channel_private" &&
    messages.conversation.linked_ref?.endsWith(`/agent/thread/${threadId}`)
    ? messages
    : null;
}

function workspaceSession(value: unknown): WorkspaceSession | null {
  const row = record(value);
  const workspace = record(row?.workspace);
  if (!row || !exact(row, ["id", "ref", "method", "mode", "terminal", "workspace", "started_at"]) ||
      !workspace || !exact(workspace, ["id", "generation"]) || !isUlid(row.id) ||
      !cleanText(row.ref, 4_096) || row.method !== "ssh" ||
      !["shell", "command"].includes(row.mode as string) || typeof row.terminal !== "boolean" ||
      !isAgentThreadId(workspace.id) || !positiveInteger(workspace.generation, 4_294_967_295) ||
      !timestamp(row.started_at)) return null;
  return row as unknown as WorkspaceSession;
}

export function parseWorkspaceSessions(value: unknown): WorkspaceSessionPage | null {
  const envelope = record(value);
  const paging = page(envelope?.page, isUlid);
  if (!envelope || !exact(envelope, ["items", "page"]) || !Array.isArray(envelope.items) ||
      !paging || envelope.items.length > paging.limit) return null;
  const items = envelope.items.map(workspaceSession);
  return items.every((item): item is WorkspaceSession => item !== null)
    ? { items, page: paging }
    : null;
}

export function parseAgentThreadMessageReceipt(
  value: unknown,
  threadId: string,
): { message_id: string; thread_id: string; durable: true } | null {
  const receipt = record(value);
  return receipt && exact(receipt, ["message_id", "thread_id", "durable"]) &&
    isUlid(receipt.message_id) && receipt.thread_id === threadId && receipt.durable === true
    ? receipt as { message_id: string; thread_id: string; durable: true }
    : null;
}
