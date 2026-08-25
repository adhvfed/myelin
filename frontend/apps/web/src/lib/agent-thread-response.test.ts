import { describe, expect, it } from "vitest";

import {
  parseAgentActivationReceipt,
  parseAgentChoices,
  parseAgentThread,
  parseAgentThreadCreateReceipt,
  parseAgentThreadMessageReceipt,
  parseAgentThreadMessages,
  parseAgentThreads,
  parseWorkspaceSessions,
} from "./agent-thread-response";

const THREAD = "22222222-2222-4222-8222-222222222222";
const AGENT = "33333333-3333-4333-8333-333333333333";
const WORKSPACE = "44444444-4444-4444-8444-444444444444";
const CONVERSATION = "01J00000000000000000000000";
const MESSAGE = "01J00000000000000000000001";
const SESSION = "01J00000000000000000000002";
const agent = {
  id: AGENT,
  ref: `myelin://acme/identity/agent/${AGENT}`,
  principal_id: `agent:${AGENT}`,
  name: "Checkout companion",
  runtime_ref: "external:mcp",
  on_behalf_of: "developer",
  status: "active",
  selected_tools: [{
    name: "chat.post",
    version: 1,
    ref: "myelin://acme/agent/tool/chat.post@1",
  }],
  effective_tools: [],
  grants: ["chat.post"],
  created_at: "2026-08-22T12:00:00Z",
};
const governance = {
  policy_versions: { agent: 1, delegation: 1, tenant: 1, trigger_actor: 1 },
  policy_revisions: { agent: 11, delegation: 12, tenant: 13, trigger_actor: 14 },
};
const subject = {
  id: THREAD,
  ref: `myelin://acme/agent/thread/${THREAD}`,
  name: "Investigate checkout race",
  agent_id: AGENT,
  agent_ref: `myelin://acme/identity/agent/${AGENT}`,
  project_id: null,
  conversation_id: CONVERSATION,
  conversation_ref: `myelin://acme/chat/channel/${CONVERSATION}`,
  workspace: {
    id: WORKSPACE,
    generation: 1,
    state: "ready",
    retention_days: 3,
    expires_at: "2026-08-25T12:00:00Z",
  },
  created_at: "2026-08-22T12:00:00Z",
  updated_at: "2026-08-22T12:00:01Z",
};
const conversation = {
  id: CONVERSATION,
  ref: `myelin://acme/chat/channel/${CONVERSATION}`,
  kind: "channel_private",
  project_id: null,
  channel: subject.name,
  topic: "Private agent work",
  linked_ref: subject.ref,
  pinned_canvas: null,
  retention_days: 3,
};

describe("private agent work wire projection", () => {
  it("decodes threads, messages, agents, and accountable workspace entries", () => {
    expect(parseAgentThreads({ items: [subject], page: { next_cursor: null, limit: 100 } })?.items)
      .toEqual([subject]);
    expect(parseAgentThread({ thread: subject })).toEqual(subject);
    expect(parseAgentThreadCreateReceipt({ thread: subject, created: true, durable: true }))
      .toEqual({ thread: subject, created: true, durable: true });
    expect(parseAgentThreadMessages({
      conversation,
      items: [{
        id: MESSAGE,
        author: "chat-author:0123456789abcdef0123456789abcdef",
        author_kind: "human",
        is_you: true,
        content: "The final reader still owns the lease.",
        nodes: [],
        thread_root_id: null,
        reply_count: 0,
        edited: false,
        state: "active",
        created_at: 1_700_000_000,
      }],
      page: { next_cursor: null, limit: 100 },
    }, THREAD)?.conversation).toEqual(conversation);
    expect(parseWorkspaceSessions({
      items: [{
        id: SESSION,
        ref: `myelin://acme/agent/workspace/${WORKSPACE}#ssh-session-${SESSION}`,
        method: "ssh",
        mode: "command",
        terminal: true,
        workspace: { id: WORKSPACE, generation: 1 },
        started_at: "2026-08-22T12:02:00Z",
      }],
      page: { next_cursor: null, limit: 100 },
    })?.items).toHaveLength(1);
    expect(parseAgentChoices({
      items: [agent],
      page: { next_cursor: null, limit: 100 },
    })?.items).toEqual([{
      id: AGENT,
      name: "Checkout companion",
      runtime_ref: "external:mcp",
      status: "active",
    }]);
    expect(parseAgentActivationReceipt({ agent, created: true, durable: true, governance })).toEqual({
      agent: {
        id: AGENT,
        name: "Checkout companion",
        runtime_ref: "external:mcp",
        status: "active",
      },
      created: true,
      durable: true,
    });
  });

  it("refuses crossed aggregates and surplus connection material", () => {
    expect(parseAgentThread({ thread: { ...subject, agent_id: WORKSPACE } })).toBeNull();
    expect(parseAgentThreadMessages({
      conversation: { ...conversation, kind: "channel_public" },
      items: [],
      page: { next_cursor: null, limit: 100 },
    }, THREAD)).toBeNull();
    expect(parseWorkspaceSessions({
      items: [{
        id: SESSION,
        ref: "myelin://acme/agent/workspace/x",
        method: "ssh",
        mode: "shell",
        terminal: true,
        workspace: { id: WORKSPACE, generation: 1 },
        started_at: "2026-08-22T12:02:00Z",
        host: "ssh.internal",
      }],
      page: { next_cursor: null, limit: 100 },
    })).toBeNull();
    expect(parseAgentThreadMessageReceipt({
      message_id: MESSAGE,
      thread_id: AGENT,
      durable: true,
    }, THREAD)).toBeNull();
    expect(parseAgentActivationReceipt({
      agent,
      created: true,
      durable: true,
      governance: {
        ...governance,
        policy_revisions: { ...governance.policy_revisions, tenant: -1 },
      },
    })).toBeNull();
  });
});
