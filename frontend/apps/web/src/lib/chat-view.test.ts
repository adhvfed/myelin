import { describe, expect, it } from "vitest";

import type { ChatConversation, ChatMessage, ChatMessagePage } from "./api";
import {
  chatAuthorLabel,
  chatErrorKind,
  chatHref,
  groupChatConversations,
  mergeMessagePages,
} from "./chat-view";

const conversation = (id: string, channel: string, topic: string): ChatConversation => ({
  id,
  ref: `myelin://acme/chat/channel/${id}`,
  kind: "channel_public",
  project_id: "11111111-1111-1111-1111-111111111111",
  channel,
  topic,
  linked_ref: null,
  pinned_canvas: null,
  retention_days: null,
});

const message = (id: string, isYou = false): ChatMessage => ({
  id,
  author: "chat-author:0123456789abcdef0123456789abcdef",
  author_kind: "human",
  is_you: isYou,
  content: id,
  nodes: [],
  edited: false,
  state: "active",
  created_at: 1,
});

const page = (items: ChatMessage[]): ChatMessagePage => ({
  conversation: conversation("01J00000000000000000000000", "eng", "release"),
  items,
  page: { next_cursor: null, limit: 100 },
});

describe("Chat view helpers", () => {
  it("groups channels and topics into a stable navigation", () => {
    expect(groupChatConversations([
      conversation("01J00000000000000000000002", "product", "launch"),
      conversation("01J00000000000000000000001", "eng", "zeta"),
      conversation("01J00000000000000000000000", "eng", "alpha"),
    ]).map((group) => [group.channel, group.topics.map((topic) => topic.topic)]))
      .toEqual([["eng", ["alpha", "zeta"]], ["product", ["launch"]]]);
  });

  it("orders earlier message pages chronologically and removes overlap", () => {
    expect(mergeMessagePages(
      page([message("03"), message("04")]),
      [page([message("02"), message("03")]), page([message("01"), message("02")])],
    ).map((row) => row.id)).toEqual(["01", "02", "03", "04"]);
  });

  it("keeps route errors dignified and author identities pseudonymous", () => {
    expect(chatErrorKind(new Error("CHAT_ERR:unavailable"))).toBe("unavailable");
    expect(chatErrorKind(new Error("database address"))).toBe("error");
    expect(chatHref("01J id")).toBe("/chat?conversation=01J%20id");
    expect(chatAuthorLabel(message("01", true))).toBe("You");
    expect(chatAuthorLabel(message("01"))).toBe("Teammate · abcdef");
  });
});
