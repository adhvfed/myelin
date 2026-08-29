import { action, json, query, redirect } from "@solidjs/router";

import { edgeGet, edgePost, GatewayError, isUnauthorized } from "../server/gateway";
import {
  isAgentThreadId,
  parseAgentActivationReceipt,
  parseAgentChoices,
  parseAgentThread,
  parseAgentThreadCreateReceipt,
  parseAgentThreadMessageReceipt,
  parseAgentThreadMessages,
  parseAgentThreads,
  parseWorkspaceSessions,
  type AgentChoicePage,
  type AgentActivationReceipt,
  type AgentThread,
  type AgentThreadCreateReceipt,
  type AgentThreadPage,
  type WorkspaceSessionPage,
} from "./agent-thread-response";
import type { ChatMessagePage } from "./chat-response";

export type AgentThreadErrorKind = "bad-input" | "not-found" | "conflict" | "unavailable" | "error";
export type AgentThreadMutationResult =
  | { ok: true; op: "create"; receipt: AgentThreadCreateReceipt }
  | { ok: true; op: "activate-agent"; receipt: AgentActivationReceipt }
  | { ok: true; op: "post-message"; messageId: string; threadId: string }
  | { ok: false; error: AgentThreadErrorKind };

const AGENT_THREAD_ERR_PREFIX = "AGENT_THREAD_ERR:";
const utf8 = new TextEncoder();
const PRIVATE_WORK_AGENT_TOOLS = [
  "chat.read_messages",
  "chat.post",
  "workspace.read_file",
  "workspace.write_file",
] as const;
const PRIVATE_WORK_COMMAND_TOOL = "workspace.exec" as const;

export class AgentThreadRouteError extends Error {
  readonly kind: AgentThreadErrorKind;
  constructor(kind: AgentThreadErrorKind) {
    super(`${AGENT_THREAD_ERR_PREFIX}${kind}`);
    this.name = "AgentThreadRouteError";
    this.kind = kind;
  }
}

function segment(value: string): string {
  return encodeURIComponent(value);
}

function cleanText(value: unknown, maximum: number): value is string {
  return typeof value === "string" && value.length > 0 && value.trim() === value &&
    utf8.encode(value).byteLength <= maximum && !/[\p{Cc}]/u.test(value);
}

function isUlid(value: unknown): value is string {
  return typeof value === "string" && /^[0-9A-HJKMNP-TV-Z]{26}$/.test(value);
}

function pageInput(
  value: { cursor?: string; limit?: number },
  cursor: (value: unknown) => value is string,
): { cursor?: string; limit: number } | null {
  if (!value || typeof value !== "object" || Array.isArray(value) ||
      Object.keys(value).some((key) => key !== "cursor" && key !== "limit") ||
      (value.cursor !== undefined && !cursor(value.cursor))) return null;
  const limit = value.limit ?? 100;
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 100) return null;
  return value.cursor === undefined ? { limit } : { cursor: value.cursor, limit };
}

function pageQuery(input: { cursor?: string; limit: number }): string {
  const query = new URLSearchParams({ limit: String(input.limit) });
  if (input.cursor) query.set("cursor", input.cursor);
  return query.toString();
}

async function authed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) throw new AgentThreadRouteError("bad-input");
      if (error.status === 403 || error.status === 404) throw new AgentThreadRouteError("not-found");
      if (error.status === 409) throw new AgentThreadRouteError("conflict");
      if (error.status === 503) throw new AgentThreadRouteError("unavailable");
    }
    if (error instanceof AgentThreadRouteError) throw error;
    throw new AgentThreadRouteError("error");
  }
}

export function agentThreadErrorKind(error: unknown): AgentThreadErrorKind {
  const message = error instanceof Error ? error.message : String(error ?? "");
  const encoded = message.startsWith(AGENT_THREAD_ERR_PREFIX)
    ? message.slice(AGENT_THREAD_ERR_PREFIX.length)
    : "";
  return ["bad-input", "not-found", "conflict", "unavailable"].includes(encoded)
    ? encoded as AgentThreadErrorKind
    : "error";
}

export const getAgentChoices = query(async (request: {
  cursor?: string;
  limit?: number;
} = {}): Promise<AgentChoicePage> => {
  "use server";
  const input = pageInput(request, isAgentThreadId);
  if (!input) throw new AgentThreadRouteError("bad-input");
  return authed(async () => {
    const response = parseAgentChoices(await edgeGet(`/v1/agents?${pageQuery(input)}`));
    if (!response) throw new AgentThreadRouteError("error");
    return response;
  });
}, "agent-thread-agent-choices");

export const getAgentThreads = query(async (request: {
  cursor?: string;
  limit?: number;
} = {}): Promise<AgentThreadPage> => {
  "use server";
  const input = pageInput(request, isAgentThreadId);
  if (!input) throw new AgentThreadRouteError("bad-input");
  return authed(async () => {
    const response = parseAgentThreads(await edgeGet(`/v1/agent-threads?${pageQuery(input)}`));
    if (!response) throw new AgentThreadRouteError("error");
    return response;
  });
}, "agent-threads");

export const getAgentThread = query(async (threadId: string): Promise<AgentThread> => {
  "use server";
  if (!isAgentThreadId(threadId)) throw new AgentThreadRouteError("bad-input");
  return authed(async () => {
    const response = parseAgentThread(await edgeGet(`/v1/agent-threads/${segment(threadId)}`));
    if (!response || response.id !== threadId) throw new AgentThreadRouteError("error");
    return response;
  });
}, "agent-thread");

export const getAgentThreadMessages = query(async (request: {
  threadId: string;
  before?: string;
  limit?: number;
}): Promise<ChatMessagePage> => {
  "use server";
  if (!request || typeof request !== "object" || Array.isArray(request) ||
      Object.keys(request).some((key) => !["threadId", "before", "limit"].includes(key)) ||
      !isAgentThreadId(request.threadId) || (request.before !== undefined && !isUlid(request.before)) ||
      (request.limit !== undefined &&
        (!Number.isSafeInteger(request.limit) || request.limit < 1 || request.limit > 100))) {
    throw new AgentThreadRouteError("bad-input");
  }
  const search = new URLSearchParams();
  if (request.before) search.set("before", request.before);
  if (request.limit) search.set("limit", String(request.limit));
  return authed(async () => {
    const response = parseAgentThreadMessages(await edgeGet(
      `/v1/agent-threads/${segment(request.threadId)}/messages${
        search.size ? `?${search.toString()}` : ""
      }`,
    ), request.threadId);
    if (!response) throw new AgentThreadRouteError("error");
    return response;
  });
}, "agent-thread-messages");

export const getWorkspaceSessions = query(async (request: {
  threadId: string;
  cursor?: string;
  limit?: number;
}): Promise<WorkspaceSessionPage> => {
  "use server";
  if (!request || !isAgentThreadId(request.threadId)) {
    throw new AgentThreadRouteError("bad-input");
  }
  const input = pageInput({ cursor: request.cursor, limit: request.limit }, isUlid);
  if (!input || Object.keys(request).some((key) => !["threadId", "cursor", "limit"].includes(key))) {
    throw new AgentThreadRouteError("bad-input");
  }
  return authed(async () => {
    const response = parseWorkspaceSessions(await edgeGet(
      `/v1/agent-threads/${segment(request.threadId)}/workspace-sessions?${pageQuery(input)}`,
    ));
    if (!response) throw new AgentThreadRouteError("error");
    return response;
  });
}, "agent-thread-workspace-sessions");

export const mutateAgentThread = action(async (input: unknown) => {
  "use server";
  const respond = (value: AgentThreadMutationResult) => json(value, { revalidate: [] });
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    return respond({ ok: false, error: "bad-input" });
  }
  const mutation = input as Record<string, unknown>;
  try {
    if (mutation.op === "activate-agent" && Object.keys(mutation).every((key) =>
      ["op", "name", "allowWorkspaceCommands", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 4 && cleanText(mutation.name, 80) &&
        typeof mutation.allowWorkspaceCommands === "boolean" && cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentActivationReceipt(await edgePost("/v1/agents", {
        name: mutation.name,
        runtime: "external",
        tools: mutation.allowWorkspaceCommands
          ? [...PRIVATE_WORK_AGENT_TOOLS, PRIVATE_WORK_COMMAND_TOOL]
          : PRIVATE_WORK_AGENT_TOOLS,
      }, { idempotencyKey: mutation.clientNonce }));
      return receipt
        ? respond({ ok: true, op: "activate-agent", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "create" && Object.keys(mutation).every((key) =>
      ["op", "name", "agentId", "retentionDays", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 5 && cleanText(mutation.name, 80) &&
        isAgentThreadId(mutation.agentId) && positiveRetention(mutation.retentionDays) &&
        cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentThreadCreateReceipt(await edgePost("/v1/agent-threads", {
        name: mutation.name,
        agent_id: mutation.agentId,
        retention_days: mutation.retentionDays,
      }, { idempotencyKey: mutation.clientNonce }));
      return receipt
        ? respond({ ok: true, op: "create", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "post-message" && Object.keys(mutation).every((key) =>
      ["op", "threadId", "content", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 4 && isAgentThreadId(mutation.threadId) &&
        cleanMessage(mutation.content) && cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentThreadMessageReceipt(await edgePost(
        `/v1/agent-threads/${segment(mutation.threadId)}/messages`,
        { content: mutation.content },
        { idempotencyKey: mutation.clientNonce },
      ), mutation.threadId);
      return receipt
        ? respond({
            ok: true,
            op: "post-message",
            messageId: receipt.message_id,
            threadId: receipt.thread_id,
          })
        : respond({ ok: false, error: "error" });
    }
    return respond({ ok: false, error: "bad-input" });
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) return respond({ ok: false, error: "bad-input" });
      if (error.status === 403 || error.status === 404) {
        return respond({ ok: false, error: "not-found" });
      }
      if (error.status === 409) return respond({ ok: false, error: "conflict" });
      if (error.status === 503) return respond({ ok: false, error: "unavailable" });
    }
    return respond({ ok: false, error: "error" });
  }
}, "agent-thread-mutate");

function positiveRetention(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 1 && (value as number) <= 30;
}

function cleanNonce(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 128 &&
    /^[A-Za-z0-9_-]+$/.test(value);
}

function cleanMessage(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0 && utf8.encode(value).byteLength <= 32 * 1024 &&
    ![...value].some((character) => {
      const point = character.codePointAt(0)!;
      return point === 0 || point === 0x7f || (point < 0x20 && character !== "\n" && character !== "\t");
    });
}
