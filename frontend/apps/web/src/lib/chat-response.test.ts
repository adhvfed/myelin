import { describe, expect, it } from "vitest";

import {
  parseChatConversationDraft,
  parseChatConversationReceipt,
  parseChatConversations,
  parseChatMessageDraft,
  parseChatMessageReceipt,
  parseChatMessages,
} from "./chat-response";

const ID = "01J00000000000000000000000";
const NEXT = "01J00000000000000000000001";
const conversation = {
  id: ID,
  channel: "engineering",
  topic: "release readiness",
  linked_ref: null,
  pinned_canvas: null,
};
const message = {
  id: NEXT,
  author: "chat-author:0123456789abcdef0123456789abcdef",
  author_kind: "human",
  is_you: true,
  content: "The rollout is green.",
  edited: false,
  state: "active",
  created_at: 1_700_000_000,
};

describe("Chat wire projection", () => {
  it("strictly decodes bounded conversations, messages, and receipts", () => {
    expect(parseChatConversations({
      items: [conversation],
      page: { next_cursor: null, limit: 50 },
    })?.items).toEqual([conversation]);
    expect(parseChatMessages({
      conversation,
      items: [message],
      page: { next_cursor: null, limit: 50 },
    })?.items).toEqual([message]);
    expect(parseChatConversationReceipt({ conversation, durable: true })).toEqual({
      conversation,
      durable: true,
    });
    expect(parseChatMessageReceipt({ message_id: NEXT, durable: true })).toEqual({
      message_id: NEXT,
      durable: true,
    });
  });

  it("rejects surplus, malformed, and over-limit wire data", () => {
    expect(parseChatConversations({
      items: [{ ...conversation, internal: true }],
      page: { next_cursor: null, limit: 50 },
    })).toBeNull();
    expect(parseChatMessages({
      conversation,
      items: [{ ...message, author: "raw-user-id" }],
      page: { next_cursor: null, limit: 50 },
    })).toBeNull();
    expect(parseChatMessages({
      conversation,
      items: [message, message],
      page: { next_cursor: null, limit: 1 },
    })).toBeNull();
    expect(parseChatMessageReceipt({ message_id: "not-an-id", durable: true })).toBeNull();
  });
});

describe("Chat mutation input", () => {
  it("accepts clean topic and message drafts", () => {
    expect(parseChatConversationDraft({ channel: "engineering", topic: "release" })).toEqual({
      channel: "engineering",
      topic: "release",
    });
    expect(parseChatMessageDraft({
      conversationId: ID,
      content: "Ship carefully\nthen observe.",
      clientNonce: "browser_01J-1",
    })).not.toBeNull();
  });

  it("rejects whitespace labels, blank messages, controls, and extra scope", () => {
    expect(parseChatConversationDraft({ channel: " engineering", topic: "release" })).toBeNull();
    expect(parseChatMessageDraft({
      conversationId: ID,
      content: "  ",
      clientNonce: "nonce",
    })).toBeNull();
    expect(parseChatMessageDraft({
      conversationId: ID,
      content: "bad\0message",
      clientNonce: "nonce",
    })).toBeNull();
    expect(parseChatConversationDraft({ channel: "engineering", topic: "release", tenant: "x" }))
      .toBeNull();
  });
});
