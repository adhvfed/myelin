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
const PROJECT = "11111111-1111-1111-1111-111111111111";
const REF = `myelin://acme/chat/channel/${ID}`;
const conversation = {
  id: ID,
  ref: REF,
  project_id: PROJECT,
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
  nodes: [],
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

  it("decodes every structured node without losing its position", () => {
    const structured = {
      ...message,
      content: "Ask \uFFFC to compare \uFFFC with \uFFFC.",
      nodes: [
        { kind: "mention", principal_id: "p:alice" },
        { kind: "artifact_ref", ref: "myelin://acme/issue/issue/MYL-7" },
        { kind: "embed", ref: "myelin://acme/knowledge/page/runbook" },
      ],
    };
    expect(parseChatMessages({
      conversation,
      items: [structured],
      page: { next_cursor: null, limit: 50 },
    })?.items).toEqual([structured]);

    for (const invalid of [
      { ...structured, content: "One marker \uFFFC", nodes: structured.nodes.slice(0, 2) },
      { ...structured, nodes: [{ kind: "mention", principal_id: "alice smith" }] },
      { ...structured, nodes: [{ kind: "artifact_ref", ref: "https://example.test/work" }] },
      { ...structured, nodes: [{ kind: "embed", ref: structured.nodes[2]!.ref, secret: true }] },
    ]) {
      expect(parseChatMessages({
        conversation,
        items: [invalid],
        page: { next_cursor: null, limit: 50 },
      })).toBeNull();
    }
  });
});

describe("Chat mutation input", () => {
  it("accepts clean topic and message drafts", () => {
    expect(parseChatConversationDraft({
      projectId: PROJECT,
      channel: "engineering",
      topic: "release",
      clientNonce: "topic_01J-1",
    })).toEqual({
      projectId: PROJECT,
      channel: "engineering",
      topic: "release",
      clientNonce: "topic_01J-1",
    });
    expect(parseChatMessageDraft({
      conversationId: ID,
      content: "Ship carefully\nthen observe.",
      references: [],
      clientNonce: "browser_01J-1",
    })).not.toBeNull();
    expect(parseChatMessageDraft({
      conversationId: ID,
      content: "Track \uFFFC.",
      references: ["myelin://acme/issue/issue/MYL-7"],
      clientNonce: "browser_01J-2",
    })).not.toBeNull();
  });

  it("rejects whitespace labels, blank messages, controls, and extra scope", () => {
    expect(parseChatConversationDraft({
      projectId: PROJECT,
      channel: " engineering",
      topic: "release",
      clientNonce: "nonce",
    })).toBeNull();
    expect(parseChatMessageDraft({
      conversationId: ID,
      content: "  ",
      references: [],
      clientNonce: "nonce",
    })).toBeNull();
    expect(parseChatMessageDraft({
      conversationId: ID,
      content: "bad\0message",
      references: [],
      clientNonce: "nonce",
    })).toBeNull();
    expect(parseChatMessageDraft({
      conversationId: ID,
      content: "Missing marker",
      references: ["myelin://acme/issue/issue/MYL-7"],
      clientNonce: "nonce",
    })).toBeNull();
    expect(parseChatConversationDraft({
      projectId: PROJECT,
      channel: "engineering",
      topic: "release",
      clientNonce: "nonce",
      tenant: "x",
    }))
      .toBeNull();
    expect(parseChatConversationDraft({
      projectId: PROJECT,
      channel: "engineering",
      topic: "release",
      clientNonce: "response lost retry",
    })).toBeNull();
  });
});
