import type {
  ChatConversation,
  ChatConversationPage,
  ChatErrorKind,
  ChatMessage,
  ChatMessagePage,
} from "./chat-api";

const CHAT_ERR_PREFIX = "CHAT_ERR:";

export interface ChatChannelGroup {
  channel: string;
  topics: ChatConversation[];
}

export function chatErrorKind(error: unknown): ChatErrorKind {
  const message = error instanceof Error ? error.message : String(error ?? "");
  const encoded = message.startsWith(CHAT_ERR_PREFIX)
    ? message.slice(CHAT_ERR_PREFIX.length)
    : "";
  return encoded === "bad-input" || encoded === "not-found" || encoded === "conflict" ||
    encoded === "unavailable" ? encoded : "error";
}

export function chatHref(conversationId?: string): string {
  return conversationId ? `/chat?conversation=${encodeURIComponent(conversationId)}` : "/chat";
}

export function groupChatConversations(rows: ChatConversation[]): ChatChannelGroup[] {
  const channels = new Map<string, ChatConversation[]>();
  for (const row of rows) {
    const topics = channels.get(row.channel) ?? [];
    topics.push(row);
    channels.set(row.channel, topics);
  }
  return [...channels]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([channel, topics]) => ({
      channel,
      topics: [...topics].sort((left, right) => left.topic.localeCompare(right.topic)),
    }));
}

export function mergeConversationPages(
  first?: ChatConversationPage | null,
  continuations: ChatConversationPage[] = [],
): ChatConversation[] {
  const seen = new Set<string>();
  const rows: ChatConversation[] = [];
  for (const page of [first, ...continuations]) {
    for (const row of page?.items ?? []) {
      if (seen.has(row.id)) continue;
      seen.add(row.id);
      rows.push(row);
    }
  }
  return rows;
}

/** Earlier pages arrive nearest-first; reverse them before the current recent page. */
export function mergeMessagePages(
  recent?: ChatMessagePage | null,
  earlier: ChatMessagePage[] = [],
): ChatMessage[] {
  const seen = new Set<string>();
  const messages: ChatMessage[] = [];
  const pages = [...earlier].reverse().concat(recent ? [recent] : []);
  for (const page of pages) {
    for (const message of page.items) {
      if (seen.has(message.id)) continue;
      seen.add(message.id);
      messages.push(message);
    }
  }
  return messages;
}

export function chatAuthorLabel(message: ChatMessage): string {
  if (message.is_you) return "You";
  const suffix = message.author.slice(-6);
  if (message.author_kind === "agent") return `Agent · ${suffix}`;
  if (message.author_kind === "service") return `Service · ${suffix}`;
  return `Teammate · ${suffix}`;
}

export function chatTimestamp(unixSeconds: number | null): string {
  if (unixSeconds === null) return "Time unavailable";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(unixSeconds * 1_000));
}
