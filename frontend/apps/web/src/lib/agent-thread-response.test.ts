import { describe, expect, it } from "vitest";

import {
  agentThreadAwaitingRecovery,
  agentThreadWorkspaceAvailable,
  externalAgentCanConnect,
  parseAgentActivationReceipt,
  parseAgentChoice,
  parseAgentChoices,
  parseAgentThread,
  parseAgentThreadCreateReceipt,
  parseAgentThreadMessageReceipt,
  parseAgentThreadMessages,
  parseAgentThreadReviewReceipt,
  parseAgentThreads,
  parseAgentWorkspaceGuidanceConfig,
  parseAgentWorkspaceGuidanceReceipt,
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
  workspace_profile: { revision: 0, instructions: null, updated_at: null },
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
  review_state: "draft",
  review_ready_at: null,
  workspace: {
    id: WORKSPACE,
    generation: 1,
    state: "ready",
    retention_days: 3,
    expires_at: "2026-08-25T12:00:00Z",
    repository: null,
    personalization: null,
    environment: null,
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
  it("decodes the exact repository and environment receipt for a private cloud branch", () => {
    const repository = {
      slug: "platform/api",
      base_ref: "refs/heads/main",
      base_oid: "0123456789abcdef0123456789abcdef01234567",
      branch: "myelin/thread-0123456789abcdef0123",
    };
    const environment = {
      provider: "service_federation",
      config_path: "fed.yaml",
      content_digest: `blake3:${"a".repeat(64)}`,
      state: "declared",
    };
    const parsed = parseAgentThread({
      thread: { ...subject, workspace: { ...subject.workspace, repository, environment } },
    });
    expect(parsed?.workspace.repository).toEqual(repository);
    expect(parsed?.workspace.environment).toEqual(environment);
    expect(parseAgentThread({
      thread: {
        ...subject,
        workspace: { ...subject.workspace, repository: { ...repository, base_oid: "short" } },
      },
    })).toBeNull();
    expect(parseAgentThread({
      thread: {
        ...subject,
        workspace: { ...subject.workspace, repository: null, environment },
      },
    })).toBeNull();
    expect(parseAgentThread({
      thread: {
        ...subject,
        workspace: {
          ...subject.workspace,
          repository,
          environment: { ...environment, content_digest: "blake3:short" },
        },
      },
    })).toBeNull();
  });

  it("ends client authority at the exact workspace deadline", () => {
    const decoded = parseAgentThread({ thread: subject })!;
    expect(agentThreadWorkspaceAvailable(decoded, Date.parse("2026-08-25T11:59:59Z"))).toBe(true);
    expect(agentThreadWorkspaceAvailable(decoded, Date.parse(subject.workspace.expires_at)))
      .toBe(false);
    expect(agentThreadWorkspaceAvailable({
      ...decoded,
      workspace: { ...decoded.workspace, state: "expiring" },
    }, Date.parse("2026-08-24T00:00:00Z"))).toBe(false);

    const choice = parseAgentChoices({
      items: [agent],
      page: { next_cursor: null, limit: 100 },
    })!.items[0];
    expect(externalAgentCanConnect(choice)).toBe(true);
    expect(externalAgentCanConnect({ ...choice!, status: "suspended" })).toBe(false);
    expect(externalAgentCanConnect(undefined)).toBe(false);
  });

  it("polls only receipts that automatic recovery can still advance", () => {
    const decoded = parseAgentThread({ thread: subject })!;
    expect(agentThreadAwaitingRecovery({
      ...decoded,
      workspace: { ...decoded.workspace, state: "provisioning" },
    })).toBe(true);
    expect(agentThreadAwaitingRecovery({
      ...decoded,
      workspace: { ...decoded.workspace, state: "failed" },
    })).toBe(true);
    for (const state of ["ready", "expiring", "deleted"] as const) {
      expect(agentThreadAwaitingRecovery({
        ...decoded,
        workspace: { ...decoded.workspace, state },
      })).toBe(false);
    }
  });

  it("decodes threads, messages, agents, and accountable workspace entries", () => {
    expect(parseAgentThreads({ items: [subject], page: { next_cursor: null, limit: 100 } })?.items)
      .toEqual([subject]);
    expect(parseAgentThread({ thread: subject })).toEqual(subject);
    expect(parseAgentThreadCreateReceipt({ thread: subject, created: true, durable: true }))
      .toEqual({ thread: subject, created: true, durable: true });
    const ready = {
      ...subject,
      project_id: "55555555-5555-4555-8555-555555555555",
      review_state: "ready",
      review_ready_at: "2026-08-22T12:00:02Z",
    };
    expect(parseAgentThreadReviewReceipt({ thread: ready, changed: true, durable: true }))
      .toEqual({ thread: ready, changed: true, durable: true });
    expect(parseAgentThreadReviewReceipt({ thread: subject, changed: true, durable: true }))
      .toBeNull();
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
        ref: `myelin://acme/agent/session/${SESSION}`,
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
      selected_tools: ["chat.post"],
      effective_tools: [],
      workspace_profile: { revision: 0, instructions: null, updated_at: null },
    }]);
    expect(parseAgentChoice({ agent })).toEqual({
      id: AGENT,
      name: "Checkout companion",
      runtime_ref: "external:mcp",
      status: "active",
      selected_tools: ["chat.post"],
      effective_tools: [],
      workspace_profile: { revision: 0, instructions: null, updated_at: null },
    });
    expect(parseAgentActivationReceipt({ agent, created: true, durable: true, governance })).toEqual({
      agent: {
        id: AGENT,
        name: "Checkout companion",
        runtime_ref: "external:mcp",
        status: "active",
        selected_tools: ["chat.post"],
        effective_tools: [],
        workspace_profile: { revision: 0, instructions: null, updated_at: null },
      },
      created: true,
      durable: true,
    });
  });

  it("keeps editable workspace instructions behind the owner-only config projection", () => {
    const updatedAt = "2026-08-22T12:04:00Z";
    const contentDigest = `blake3:${"a".repeat(64)}`;
    const configuredAgent = {
      ...agent,
      workspace_profile: {
        revision: 2,
        instructions: { path: "AGENTS.md", strategy: "append", content_digest: contentDigest },
        updated_at: updatedAt,
      },
    };

    expect(parseAgentChoice({ agent: configuredAgent })?.workspace_profile).toEqual(
      configuredAgent.workspace_profile,
    );
    expect(parseAgentWorkspaceGuidanceConfig({
      agent_id: AGENT,
      revision: 2,
      instructions: "Run the repository checks before handing work back.",
      updated_at: updatedAt,
    })).toEqual({
      agent_id: AGENT,
      revision: 2,
      instructions: "Run the repository checks before handing work back.",
      updated_at: updatedAt,
    });
    expect(parseAgentWorkspaceGuidanceReceipt({
      agent: configuredAgent,
      changed: true,
      durable: true,
    })?.agent.workspace_profile.revision).toBe(2);

    expect(parseAgentWorkspaceGuidanceConfig({
      agent_id: AGENT,
      revision: 2,
      instructions: "private text",
      updated_at: updatedAt,
      content_digest: contentDigest,
    })).toBeNull();
    expect(parseAgentChoice({
      agent: { ...configuredAgent, workspace_profile: {
        ...configuredAgent.workspace_profile,
        instructions: { ...configuredAgent.workspace_profile.instructions, plaintext: "private" },
      } },
    })).toBeNull();
    expect(parseAgentWorkspaceGuidanceConfig({
      agent_id: AGENT,
      revision: 0,
      instructions: "impossible pre-revision defaults",
      updated_at: null,
    })).toBeNull();
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
    expect(parseWorkspaceSessions({
      items: [{
        id: SESSION,
        ref: `myelin://acme/agent/session/${SESSION}`,
        method: "ssh",
        mode: "file_transfer",
        terminal: false,
        workspace: { id: WORKSPACE, generation: 1 },
        started_at: "2026-08-22T12:02:00Z",
      }],
      page: { next_cursor: null, limit: 100 },
    })?.items[0]?.mode).toBe("file_transfer");
    const sessionPage = (ref: string) => ({
      items: [{
        id: SESSION,
        ref,
        method: "ssh",
        mode: "shell",
        terminal: true,
        workspace: { id: WORKSPACE, generation: 1 },
        started_at: "2026-08-22T12:02:00Z",
      }],
      page: { next_cursor: null, limit: 100 },
    });
    expect(parseWorkspaceSessions(sessionPage(
      `myelin://acme/agent/workspace/${WORKSPACE}#ssh-session-${SESSION}`,
    ))).toBeNull();
    expect(parseWorkspaceSessions(sessionPage(
      "myelin://acme/agent/session/01J00000000000000000000003",
    ))).toBeNull();
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
    expect(parseAgentChoices({
      items: [{ ...agent, selected_tools: [agent.selected_tools[0], agent.selected_tools[0]] }],
      page: { next_cursor: null, limit: 100 },
    })).toBeNull();
  });
});
