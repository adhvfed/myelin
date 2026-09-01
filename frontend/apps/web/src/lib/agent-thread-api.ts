import { action, json, query, redirect } from "@solidjs/router";

import { edgeGet, edgePost, GatewayError, isUnauthorized } from "../server/gateway";
import {
  isAgentThreadId,
  parseAgentActivationReceipt,
  parseAgentWorkspaceGuidanceReceipt,
  parseAgentWorkspaceGuidanceConfig,
  parseAgentChoice,
  parseAgentChoices,
  parseAgentLifecycleReceipt,
  parseAgentNameReceipt,
  parseAgentToolPaletteReceipt,
  parseAgentThread,
  parseAgentThreadCreateReceipt,
  parseAgentThreadReviewReceipt,
  parseAgentThreadReviewHandoff,
  parseAgentThreadReviewHandoffReceipt,
  parseAgentThreadRenewalReceipt,
  parseAgentThreadMessageReceipt,
  parseAgentThreadMessages,
  parseAgentThreads,
  parseWorkspaceSessions,
  type AgentChoice,
  type AgentChoicePage,
  type AgentActivationReceipt,
  type AgentLifecycleReceipt,
  type AgentNameReceipt,
  type AgentToolPaletteReceipt,
  type AgentWorkspaceGuidanceReceipt,
  type AgentWorkspaceGuidanceConfig,
  type AgentThread,
  type AgentThreadCreateReceipt,
  type AgentThreadPage,
  type AgentThreadReviewReceipt,
  type AgentThreadReviewHandoff,
  type AgentThreadReviewHandoffReceipt,
  type AgentThreadRenewalReceipt,
  type WorkspaceSessionPage,
} from "./agent-thread-response";
import type { ChatMessagePage } from "./chat-response";
import { validAgentWorkspaceAugmentation } from "./agent-workspace-augmentation";
import {
  parseAgentToolDefinitionPage,
  validAgentToolPalette,
  type AgentToolDefinition,
} from "./agent-tool-palette";

export type AgentThreadErrorKind = "bad-input" | "not-found" | "conflict" | "unavailable" | "error";
export type AgentThreadMutationResult =
  | { ok: true; op: "create"; receipt: AgentThreadCreateReceipt }
  | { ok: true; op: "retry"; receipt: AgentThreadCreateReceipt }
  | { ok: true; op: "mark-ready"; receipt: AgentThreadReviewReceipt }
  | { ok: true; op: "publish-review"; receipt: AgentThreadReviewHandoffReceipt }
  | { ok: true; op: "renew"; receipt: AgentThreadRenewalReceipt }
  | { ok: true; op: "activate-agent"; receipt: AgentActivationReceipt }
  | { ok: true; op: "rename-agent"; receipt: AgentNameReceipt }
  | { ok: true; op: "configure-agent"; receipt: AgentWorkspaceGuidanceReceipt }
  | { ok: true; op: "configure-agent-tools"; receipt: AgentToolPaletteReceipt }
  | { ok: true; op: "lifecycle-agent"; receipt: AgentLifecycleReceipt }
  | { ok: true; op: "post-message"; messageId: string; threadId: string }
  | { ok: false; error: AgentThreadErrorKind; detail?: string };

const AGENT_THREAD_ERR_PREFIX = "AGENT_THREAD_ERR:";
const utf8 = new TextEncoder();
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

export const getAgentChoice = query(async (agentId: string): Promise<AgentChoice> => {
  "use server";
  if (!isAgentThreadId(agentId)) throw new AgentThreadRouteError("bad-input");
  return authed(async () => {
    const response = parseAgentChoice(await edgeGet(`/v1/agents/${segment(agentId)}`));
    if (!response || response.id !== agentId) throw new AgentThreadRouteError("error");
    return response;
  });
}, "agent-thread-agent-choice");

export const getAgentWorkspaceGuidance = query(async (
  agentId: string,
): Promise<AgentWorkspaceGuidanceConfig> => {
  "use server";
  if (!isAgentThreadId(agentId)) throw new AgentThreadRouteError("bad-input");
  return authed(async () => {
    const response = parseAgentWorkspaceGuidanceConfig(await edgeGet(
      `/v1/agents/${segment(agentId)}/workspace-guidance`,
    ));
    if (!response || response.agent_id !== agentId) throw new AgentThreadRouteError("error");
    return response;
  });
}, "agent-workspace-guidance");

export const getAgentToolDefinitions = query(async (): Promise<AgentToolDefinition[]> => {
  "use server";
  return authed(async () => {
    const tools: AgentToolDefinition[] = [];
    let cursor: string | null = null;
    const visited = new Set<string>();
    do {
      const query = new URLSearchParams({ limit: "100" });
      if (cursor) query.set("cursor", cursor);
      const page = parseAgentToolDefinitionPage(await edgeGet(`/v1/tools?${query}`));
      if (!page) throw new AgentThreadRouteError("error");
      tools.push(...page.items.filter((tool) => tool.exposed_over_mcp));
      if (tools.length > 128) throw new AgentThreadRouteError("error");
      cursor = page.page.next_cursor;
      if (cursor !== null && visited.has(cursor)) throw new AgentThreadRouteError("error");
      if (cursor !== null) visited.add(cursor);
    } while (cursor !== null);
    if (new Set(tools.map((tool) => tool.name)).size !== tools.length) {
      throw new AgentThreadRouteError("error");
    }
    return tools;
  });
}, "agent-tool-definitions");

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

export const getAgentThreadReviewHandoff = query(async (
  threadId: string,
): Promise<AgentThreadReviewHandoff | null> => {
  "use server";
  if (!isAgentThreadId(threadId)) throw new AgentThreadRouteError("bad-input");
  try {
    return await authed(async () => {
      const response = parseAgentThreadReviewHandoff(await edgeGet(
        `/v1/agent-threads/${segment(threadId)}/pull-request`,
      ));
      if (!response || response.thread_id !== threadId) {
        throw new AgentThreadRouteError("error");
      }
      return response;
    });
  } catch (error) {
    if (error instanceof AgentThreadRouteError && error.kind === "not-found") return null;
    throw error;
  }
}, "agent-thread-review-handoff");

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
      ["op", "name", "runtime", "tools", "workspaceInstructions", "workspaceFiles", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 7 && cleanText(mutation.name, 80) &&
        ["external", "hosted"].includes(mutation.runtime as string) &&
        validAgentToolPalette(mutation.tools) && cleanNonce(mutation.clientNonce)) {
      if (typeof mutation.workspaceInstructions !== "string" ||
          (mutation.workspaceInstructions !== "" &&
            (utf8.encode(mutation.workspaceInstructions).byteLength > 12 * 1_024 ||
              mutation.workspaceInstructions.trim().length === 0 ||
              mutation.workspaceInstructions.includes("\0"))) ||
          !validAgentWorkspaceAugmentation(
            mutation.workspaceInstructions === "" ? null : mutation.workspaceInstructions,
            mutation.workspaceFiles,
          )) {
        return respond({ ok: false, error: "bad-input" });
      }
      const receipt = parseAgentActivationReceipt(await edgePost("/v1/agents", {
        name: mutation.name,
        runtime: mutation.runtime,
        tools: mutation.tools,
        ...(mutation.workspaceInstructions === ""
          ? {}
          : { workspace_instructions: mutation.workspaceInstructions }),
        ...(mutation.workspaceFiles.length === 0
          ? {}
          : { workspace_files: mutation.workspaceFiles }),
      }, { idempotencyKey: mutation.clientNonce }));
      return receipt
        ? respond({ ok: true, op: "activate-agent", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "rename-agent" && Object.keys(mutation).every((key) =>
      ["op", "agentId", "expectedName", "name", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 5 && isAgentThreadId(mutation.agentId) &&
        cleanText(mutation.expectedName, 80) && cleanText(mutation.name, 80) &&
        cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentNameReceipt(await edgePost(
        `/v1/agents/${segment(mutation.agentId)}/name`,
        { expected_name: mutation.expectedName, name: mutation.name },
        { idempotencyKey: mutation.clientNonce },
      ));
      return receipt
        ? respond({ ok: true, op: "rename-agent", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "configure-agent-tools" && Object.keys(mutation).every((key) =>
      ["op", "agentId", "expectedTools", "tools", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 5 && isAgentThreadId(mutation.agentId) &&
        validAgentToolPalette(mutation.expectedTools) && validAgentToolPalette(mutation.tools) &&
        cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentToolPaletteReceipt(await edgePost(
        `/v1/agents/${segment(mutation.agentId)}/tools`,
        { expected_tools: mutation.expectedTools, tools: mutation.tools },
        { idempotencyKey: mutation.clientNonce },
      ));
      return receipt
        ? respond({ ok: true, op: "configure-agent-tools", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "configure-agent" && Object.keys(mutation).every((key) =>
      ["op", "agentId", "workspaceInstructions", "workspaceFiles", "expectedRevision", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 6 && isAgentThreadId(mutation.agentId) &&
        (mutation.workspaceInstructions === null ||
          (typeof mutation.workspaceInstructions === "string" &&
            utf8.encode(mutation.workspaceInstructions).byteLength <= 12 * 1_024 &&
            mutation.workspaceInstructions.trim().length > 0 &&
            !mutation.workspaceInstructions.includes("\0"))) &&
        validAgentWorkspaceAugmentation(mutation.workspaceInstructions, mutation.workspaceFiles) &&
        Number.isSafeInteger(mutation.expectedRevision) &&
        (mutation.expectedRevision as number) >= 0 && cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentWorkspaceGuidanceReceipt(await edgePost(
        `/v1/agents/${segment(mutation.agentId)}/workspace-guidance`,
        {
          instructions: mutation.workspaceInstructions,
          files: mutation.workspaceFiles,
          expected_revision: mutation.expectedRevision,
        },
        { idempotencyKey: mutation.clientNonce },
      ));
      return receipt
        ? respond({ ok: true, op: "configure-agent", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "lifecycle-agent" && Object.keys(mutation).every((key) =>
      ["op", "agentId", "action", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 4 && isAgentThreadId(mutation.agentId) &&
        ["suspend", "resume", "retire"].includes(mutation.action as string) &&
        cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentLifecycleReceipt(await edgePost(
        `/v1/agents/${segment(mutation.agentId)}/${mutation.action as string}`,
        {},
        { idempotencyKey: mutation.clientNonce },
      ));
      return receipt
        ? respond({ ok: true, op: "lifecycle-agent", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "create" && Object.keys(mutation).every((key) =>
      ["op", "name", "agentId", "projectId", "retentionDays", "repository", "baseRef", "branch", "instructions", "workspaceFiles", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 11 && cleanText(mutation.name, 80) &&
        isAgentThreadId(mutation.agentId) && positiveRetention(mutation.retentionDays) &&
        typeof mutation.projectId === "string" &&
        (mutation.projectId === "" || isAgentThreadId(mutation.projectId)) &&
        typeof mutation.repository === "string" &&
        (mutation.repository === "" || cleanText(mutation.repository, 255)) &&
        typeof mutation.baseRef === "string" && cleanText(mutation.baseRef, 1_024) &&
        typeof mutation.branch === "string" &&
        (mutation.branch === "" || cleanText(mutation.branch, 1_024)) &&
        typeof mutation.instructions === "string" &&
        (mutation.instructions === "" || (utf8.encode(mutation.instructions).byteLength <= 12 * 1_024 &&
          mutation.instructions.trim().length > 0 && !mutation.instructions.includes("\0"))) &&
        validAgentWorkspaceAugmentation(
          mutation.instructions === "" ? null : mutation.instructions,
          mutation.workspaceFiles,
        ) &&
        cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentThreadCreateReceipt(await edgePost("/v1/agent-threads", {
        name: mutation.name,
        agent_id: mutation.agentId,
        ...(mutation.projectId === "" ? {} : { project_id: mutation.projectId }),
        retention_days: mutation.retentionDays,
        ...(mutation.repository === "" && mutation.instructions === "" &&
          mutation.workspaceFiles.length === 0 ? {} : {
          workspace: {
            ...(mutation.repository === "" ? {} : {
              repository: mutation.repository,
              base_ref: mutation.baseRef,
              ...(mutation.branch === "" ? {} : { branch: mutation.branch }),
            }),
            ...(mutation.instructions === "" ? {} : { instructions: mutation.instructions }),
            ...(mutation.workspaceFiles.length === 0 ? {} : { files: mutation.workspaceFiles }),
          },
        }),
      }, { idempotencyKey: mutation.clientNonce }));
      return receipt
        ? respond({ ok: true, op: "create", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "retry" && Object.keys(mutation).every((key) =>
      ["op", "threadId"].includes(key)) && Object.keys(mutation).length === 2 &&
        isAgentThreadId(mutation.threadId)) {
      const receipt = parseAgentThreadCreateReceipt(await edgePost(
        `/v1/agent-threads/${segment(mutation.threadId)}/retry`,
        {},
      ));
      return receipt
        ? respond({ ok: true, op: "retry", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "mark-ready" && Object.keys(mutation).every((key) =>
      ["op", "threadId"].includes(key)) && Object.keys(mutation).length === 2 &&
        isAgentThreadId(mutation.threadId)) {
      const receipt = parseAgentThreadReviewReceipt(await edgePost(
        `/v1/agent-threads/${segment(mutation.threadId)}/ready`,
        {},
      ));
      return receipt
        ? respond({ ok: true, op: "mark-ready", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "publish-review" && Object.keys(mutation).every((key) =>
      ["op", "threadId", "title", "bodyMd", "reviewers", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 6 && isAgentThreadId(mutation.threadId) &&
        cleanText(mutation.title, 512) &&
        (mutation.bodyMd === null || cleanText(mutation.bodyMd, 65_536)) &&
        Array.isArray(mutation.reviewers) && mutation.reviewers.length <= 100 &&
        mutation.reviewers.every((reviewer) => cleanText(reviewer, 255)) &&
        new Set(mutation.reviewers).size === mutation.reviewers.length &&
        cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentThreadReviewHandoffReceipt(await edgePost(
        `/v1/agent-threads/${segment(mutation.threadId)}/pull-request`,
        {
          title: mutation.title,
          ...(mutation.bodyMd === null ? {} : { body_md: mutation.bodyMd }),
          reviewers: mutation.reviewers,
        },
        { idempotencyKey: mutation.clientNonce },
      ));
      return receipt
        ? respond({ ok: true, op: "publish-review", receipt })
        : respond({ ok: false, error: "error" });
    }
    if (mutation.op === "renew" && Object.keys(mutation).every((key) =>
      ["op", "threadId", "retentionDays", "clientNonce"].includes(key)) &&
        Object.keys(mutation).length === 4 && isAgentThreadId(mutation.threadId) &&
        positiveRetention(mutation.retentionDays) && cleanNonce(mutation.clientNonce)) {
      const receipt = parseAgentThreadRenewalReceipt(await edgePost(
        `/v1/agent-threads/${segment(mutation.threadId)}/renew`,
        { retention_days: mutation.retentionDays },
        { idempotencyKey: mutation.clientNonce },
      ));
      return receipt
        ? respond({ ok: true, op: "renew", receipt })
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
      if (error.status === 409) {
        const detail = mutation.op === "publish-review" && error.code === "conflict" &&
            cleanText(error.message, 2_048)
          ? error.message
          : undefined;
        return respond({ ok: false, error: "conflict", ...(detail ? { detail } : {}) });
      }
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
