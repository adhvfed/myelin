import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import {
  browserApprovedCliClient,
  reviewerClient,
  uniqueName,
} from "../src/context.js";
import {
  activateExternalAgent,
  askAgent,
  beginAgentThreadRun,
  closeAgentRun,
} from "../src/journeys/agents.js";
import {
  listPrivateAgentThreads,
  parsePrivateAgentThread,
  startPrivateAgentThread,
} from "../src/journeys/agent-threads.js";
import { Conversation } from "../src/journeys/chat.js";
import { array } from "../src/json.js";

const THREE_DAYS_MS = 3 * 24 * 60 * 60 * 1_000;

describe("private work with an agent", () => {
  test("keeps one named problem and workspace private while fresh agent context resumes it", async () => {
    const founder = await browserApprovedCliClient();
    const teammate = await browserApprovedCliClient(reviewerClient);
    const companion = await activateExternalAgent(
      founder,
      uniqueName("Checkout companion"),
      ["chat.read_messages", "chat.post"],
    );
    const threadName = uniqueName("Investigate checkout race");
    const retryKey = `private-agent-thread-${randomUUID()}`;

    const first = await startPrivateAgentThread(founder, {
      name: threadName,
      agentId: companion.agent.id,
      retentionDays: 3,
      idempotencyKey: retryKey,
    });
    expect(first.created).toBe(true);
    expect(first.thread).toMatchObject({
      name: threadName,
      agent_id: companion.agent.id,
      project_id: null,
      workspace: { generation: 1, state: "ready", retention_days: 3 },
    });
    expect(Date.parse(first.thread.workspace.expires_at) - Date.parse(first.thread.created_at))
      .toBe(THREE_DAYS_MS);

    const retry = await startPrivateAgentThread(founder, {
      name: threadName,
      agentId: companion.agent.id,
      retentionDays: 3,
      idempotencyKey: retryKey,
      expectedStatus: 200,
    });
    expect(retry.created).toBe(false);
    expect(retry.thread).toEqual(first.thread);

    expect(await listPrivateAgentThreads(founder)).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: first.thread.id, name: threadName })]),
    );
    expect(await listPrivateAgentThreads(teammate)).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ id: first.thread.id })]),
    );
    const hiddenThread = await teammate.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}`,
      { expectedStatus: 404 },
    );
    expect(hiddenThread.body).toMatchObject({ error: { code: "not_found" } });

    const conversation = new Conversation(first.thread.conversation_id, {});
    const problem = "Please find why checkout cleanup occasionally races its final reader.";
    await conversation.post(founder, problem);
    const hiddenHistory = await teammate.json(
      `/v1/chat/conversations/${encodeURIComponent(conversation.id)}/messages?limit=10`,
      { expectedStatus: 404 },
    );
    expect(hiddenHistory.body).toMatchObject({ error: { code: "not_found" } });
    const hiddenPost = await teammate.json(
      `/v1/chat/conversations/${encodeURIComponent(conversation.id)}/messages`,
      {
        method: "POST",
        body: { content: "I should not be able to discover this private work." },
        expectedStatus: 404,
      },
    );
    expect(hiddenPost.body).toMatchObject({ error: { code: "not_found" } });

    const hiddenRun = await teammate.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/runs`,
      {
        method: "POST",
        body: {},
        idempotencyKey: `hidden-thread-run-${randomUUID()}`,
        expectedStatus: 404,
      },
    );
    expect(hiddenRun.body).toMatchObject({ error: { code: "not_found" } });

    const firstRunKey = `private-agent-run-${randomUUID()}`;
    const firstContext = await beginAgentThreadRun(founder, first.thread.id, {
      idempotencyKey: firstRunKey,
    });
    expect(firstContext.run.context).toEqual({
      thread_id: first.thread.id,
      thread_ref: first.thread.ref,
      conversation_id: first.thread.conversation_id,
      conversation_ref: first.thread.conversation_ref,
      workspace: {
        id: first.thread.workspace.id,
        generation: first.thread.workspace.generation,
        expires_at: first.thread.workspace.expires_at,
      },
    });
    expect(Date.parse(firstContext.run.expires_at)).toBeLessThanOrEqual(
      Date.parse(first.thread.workspace.expires_at),
    );
    const retriedContext = await beginAgentThreadRun(founder, first.thread.id, {
      idempotencyKey: firstRunKey,
      expectedStatus: 200,
    });
    expect(retriedContext.run.id).toBe(firstContext.run.id);
    expect(retriedContext.run.context).toEqual(firstContext.run.context);
    const rememberedProblem = await askAgent(firstContext, 1, "chat.read_messages", {
      conversation_id: conversation.id,
      limit: 10,
    });
    expect(array(rememberedProblem.items, "first agent context messages")).toEqual(
      expect.arrayContaining([expect.objectContaining({ content: problem })]),
    );
    await closeAgentRun(firstContext);

    const freshContext = await beginAgentThreadRun(founder, first.thread.id);
    expect(freshContext.run.id).not.toBe(firstContext.run.id);
    expect(freshContext.run.context).toEqual(firstContext.run.context);
    const resumedProblem = await askAgent(freshContext, 1, "chat.read_messages", {
      conversation_id: conversation.id,
      limit: 10,
    });
    expect(array(resumedProblem.items, "fresh agent context messages")).toEqual(
      expect.arrayContaining([expect.objectContaining({ content: problem })]),
    );
    await closeAgentRun(freshContext);

    const fetched = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}`,
    );
    expect(parsePrivateAgentThread(fetched.body.thread)).toEqual(first.thread);
    expect(fetched.headers.get("cache-control")).toBe("no-store");
    expect(fetched.body.thread).not.toHaveProperty("storage_locator");
    expect(fetched.body.thread).not.toHaveProperty("failure_reason");
    expect(fetched.body.thread).not.toHaveProperty("tenant_id");
  });
});
