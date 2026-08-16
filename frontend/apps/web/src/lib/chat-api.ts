import { action, json, query, redirect } from "@solidjs/router";

import { edgeGet, edgePost, GatewayError, isUnauthorized } from "../server/gateway";
import {
  isChatUlid,
  parseChatConversationDraft,
  parseChatConversationReceipt,
  parseChatConversations,
  parseChatMessageDraft,
  parseChatMessageReceipt,
  parseChatMessages,
  type ChatConversationDraft,
  type ChatConversationPage,
  type ChatConversationReceipt,
  type ChatMessageDraft,
  type ChatMessagePage,
  type ChatMessageReceipt,
} from "./chat-response";

export type {
  ChatAuthorKind,
  ChatConversation,
  ChatConversationPage,
  ChatConversationReceipt,
  ChatMessage,
  ChatMessageNode,
  ChatMessagePage,
  ChatMessageReceipt,
  ChatMessageState,
} from "./chat-response";

export type ChatErrorKind = "bad-input" | "not-found" | "conflict" | "unavailable" | "error";
export const CHAT_ERR_PREFIX = "CHAT_ERR:";

export class ChatRouteError extends Error {
  readonly kind: ChatErrorKind;
  constructor(kind: ChatErrorKind) {
    super(`${CHAT_ERR_PREFIX}${kind}`);
    this.name = "ChatRouteError";
    this.kind = kind;
  }
}

async function chatAuthed<T>(fetcher: () => Promise<T>): Promise<T> {
  try {
    return await fetcher();
  } catch (error) {
    if (isUnauthorized(error)) throw redirect("/login");
    if (error instanceof GatewayError) {
      if (error.status === 400) throw new ChatRouteError("bad-input");
      if (error.status === 404) throw new ChatRouteError("not-found");
      if (error.status === 409) throw new ChatRouteError("conflict");
      if (error.status === 503) throw new ChatRouteError("unavailable");
    }
    if (error instanceof ChatRouteError) throw error;
    throw new ChatRouteError("error");
  }
}

function segment(value: string): string {
  return encodeURIComponent(value);
}

/** Public channel topics visible through the authenticated principal's projects. */
export const getChatConversations = query(async (request: {
  cursor?: string;
  limit?: number;
} = {}): Promise<ChatConversationPage> => {
  "use server";
  if (request === null || typeof request !== "object" || Array.isArray(request) ||
      Object.keys(request).some((key) => key !== "cursor" && key !== "limit") ||
      (request.cursor !== undefined && !isChatUlid(request.cursor)) ||
      (request.limit !== undefined &&
        (!Number.isSafeInteger(request.limit) || request.limit < 1 || request.limit > 100))) {
    throw new ChatRouteError("bad-input");
  }
  const search = new URLSearchParams();
  if (request.cursor) search.set("cursor", request.cursor);
  if (request.limit) search.set("limit", String(request.limit));
  return chatAuthed(async () => {
    const response = await edgeGet(
      `/v1/chat/conversations${search.size ? `?${search.toString()}` : ""}`,
    );
    const decoded = parseChatConversations(response);
    if (!decoded) throw new ChatRouteError("error");
    return decoded;
  });
}, "chat-conversations");

/** Recent messages in one topic, oldest-to-newest within the bounded page. */
export const getChatMessages = query(async (request: {
  conversationId: string;
  before?: string;
  limit?: number;
}): Promise<ChatMessagePage> => {
  "use server";
  if (!request || typeof request !== "object" || Array.isArray(request) ||
      !isChatUlid(request.conversationId) ||
      (request.before !== undefined && !isChatUlid(request.before)) ||
      (request.limit !== undefined &&
        (!Number.isSafeInteger(request.limit) || request.limit < 1 || request.limit > 100)) ||
      Object.keys(request).some((key) => !["conversationId", "before", "limit"].includes(key))) {
    throw new ChatRouteError("bad-input");
  }
  const search = new URLSearchParams();
  if (request.before) search.set("before", request.before);
  if (request.limit) search.set("limit", String(request.limit));
  return chatAuthed(async () => {
    const response = await edgeGet(
      `/v1/chat/conversations/${segment(request.conversationId)}/messages${
        search.size ? `?${search.toString()}` : ""
      }`,
    );
    const decoded = parseChatMessages(response);
    if (!decoded || decoded.conversation.id !== request.conversationId) {
      throw new ChatRouteError("error");
    }
    return decoded;
  });
}, "chat-messages");

export type ChatMutation =
  | ({ op: "create-conversation" } & ChatConversationDraft)
  | ({ op: "post-message" } & ChatMessageDraft);

export type ChatMutationResult =
  | { ok: true; op: "create-conversation"; receipt: ChatConversationReceipt }
  | { ok: true; op: "post-message"; receipt: ChatMessageReceipt }
  | { ok: false; error: ChatErrorKind };

/** Chat writes use one narrow server action; tenant and author always come from the signed session. */
export const chatMutate = action(async (mutation: ChatMutation) => {
  "use server";
  const result = (value: ChatMutationResult) => json(value, { revalidate: [] });
  try {
    if (!mutation || typeof mutation !== "object") {
      return result({ ok: false, error: "bad-input" });
    }
    if (mutation.op === "create-conversation") {
      const parsed = parseChatConversationDraft({
        projectId: mutation.projectId,
        channel: mutation.channel,
        topic: mutation.topic,
        clientNonce: mutation.clientNonce,
      });
      if (!parsed || Object.keys(mutation).some((key) =>
        !["op", "projectId", "channel", "topic", "clientNonce"].includes(key))) {
        return result({ ok: false, error: "bad-input" });
      }
      const receipt = await chatAuthed(async () => {
        const decoded = parseChatConversationReceipt(
          await edgePost("/v1/chat/conversations", {
            project_id: parsed.projectId,
            channel: parsed.channel,
            topic: parsed.topic,
          }, { idempotencyKey: parsed.clientNonce }),
        );
        if (!decoded) throw new ChatRouteError("error");
        return decoded;
      });
      return result({ ok: true, op: mutation.op, receipt });
    }
    if (mutation.op === "post-message") {
      const parsed = parseChatMessageDraft({
        conversationId: mutation.conversationId,
        content: mutation.content,
        references: mutation.references,
        clientNonce: mutation.clientNonce,
      });
      if (!parsed || Object.keys(mutation).some((key) =>
        !["op", "conversationId", "content", "references", "clientNonce"].includes(key))) {
        return result({ ok: false, error: "bad-input" });
      }
      const receipt = await chatAuthed(async () => {
        const decoded = parseChatMessageReceipt(await edgePost(
          `/v1/chat/conversations/${segment(parsed.conversationId)}/messages`,
          {
            content: parsed.content,
            references: parsed.references,
          },
          { idempotencyKey: parsed.clientNonce },
        ));
        if (!decoded) throw new ChatRouteError("error");
        return decoded;
      });
      return result({ ok: true, op: mutation.op, receipt });
    }
    return result({ ok: false, error: "bad-input" });
  } catch (error) {
    if (error instanceof ChatRouteError) return result({ ok: false, error: error.kind });
    throw error;
  }
}, "chat-mutate");
