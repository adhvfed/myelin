import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import {
  browserApprovedCliClient,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import {
  activateExternalAgent,
  askAgent,
  askAgentToAct,
  askAgentToBeDenied,
  beginAgentRun,
  beginAgentThreadRun,
  closeAgentRun,
} from "../src/journeys/agents.js";
import {
  listPrivateAgentThreads,
  parsePrivateAgentThread,
  requestWorkspaceSshAccess,
  startPrivateAgentThread,
} from "../src/journeys/agent-threads.js";
import { Conversation } from "../src/journeys/chat.js";
import { array, record, string } from "../src/json.js";
import { reconcileAgentThreads } from "../src/operator.js";
import { connectToWorkspace, generateEphemeralSshKey } from "../src/ssh.js";

const THREE_DAYS_MS = 3 * 24 * 60 * 60 * 1_000;

describe("private work with an agent", () => {
  test("keeps one named problem and workspace private while fresh agent context resumes it", async () => {
    const founder = await browserApprovedCliClient();
    const teammate = await browserApprovedCliClient(reviewerClient);
    const companion = await activateExternalAgent(
      founder,
      uniqueName("Checkout companion"),
      [
        "chat.read_messages",
        "chat.post",
        "workspace.read_file",
        "workspace.write_file",
      ],
    );
    const threadName = uniqueName("Investigate checkout race");
    const retryKey = `private-agent-thread-${randomUUID()}`;
    const notebookPath = "notes/continuity.md";
    const notebook = `${uniqueName("Checkout investigation")}\n\nThe final reader still owns the lease.`;
    const ownerNotePath = "notes/from-owner.md";
    const ownerNote = `${uniqueName("Owner observation")}: cleanup waits for the final reader.`;

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
    const postedProblem = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/messages`,
      {
        method: "POST",
        body: { content: problem },
        idempotencyKey: `private-agent-problem-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    expect(postedProblem.body).toMatchObject({
      thread_id: first.thread.id,
      durable: true,
    });
    const threadHistory = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/messages?limit=10`,
    );
    expect(array(threadHistory.body.items, "private thread messages")).toEqual(
      expect.arrayContaining([expect.objectContaining({ content: problem })]),
    );

    await teammate.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/messages?limit=10`,
      { expectedStatus: 404 },
    );
    await teammate.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/messages`,
      {
        method: "POST",
        body: { content: "I should not be able to discover this private work." },
        idempotencyKey: `hidden-thread-message-${randomUUID()}`,
        expectedStatus: 404,
      },
    );
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

    const sshKey = await generateEphemeralSshKey();
    const anotherSshKey = await generateEphemeralSshKey();
    try {
      const hiddenSsh = await teammate.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/ssh-access`,
        {
          method: "POST",
          body: { public_key: sshKey.publicKey },
          idempotencyKey: `hidden-workspace-ssh-${randomUUID()}`,
          expectedStatus: 404,
        },
      );
      expect(hiddenSsh.body).toMatchObject({ error: { code: "not_found" } });

      const sshRetryKey = `workspace-ssh-${randomUUID()}`;
      const sshAccess = await requestWorkspaceSshAccess(
        founder,
        first.thread.id,
        sshKey.publicKey,
        { idempotencyKey: sshRetryKey },
      );
      expect(sshAccess.created).toBe(true);
      expect(sshAccess.workspace).toEqual({
        id: first.thread.workspace.id,
        generation: first.thread.workspace.generation,
      });
      expect(sshAccess.access.username).toMatch(/^ws1_[A-Za-z0-9_-]+$/);
      expect(sshAccess.access.public_key_fingerprint).toMatch(/^SHA256:[A-Za-z0-9+/]{43}$/);
      expect(sshAccess.access.host_public_key).toMatch(/^ssh-ed25519 [A-Za-z0-9+/=]+$/);
      expect(sshAccess.access.host_key_fingerprint).toMatch(/^SHA256:[A-Za-z0-9+/]{43}$/);
      expect(Date.parse(sshAccess.access.expires_at)).toBeLessThanOrEqual(Date.now() + 5 * 60_000);
      expect(Date.parse(sshAccess.access.expires_at)).toBeLessThanOrEqual(
        Date.parse(first.thread.workspace.expires_at),
      );

      const replayedSshAccess = await requestWorkspaceSshAccess(
        founder,
        first.thread.id,
        sshKey.publicKey,
        { idempotencyKey: sshRetryKey, expectedStatus: 200 },
      );
      expect(replayedSshAccess).toEqual({ ...sshAccess, created: false });
      const changedKey = await founder.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/ssh-access`,
        {
          method: "POST",
          body: { public_key: anotherSshKey.publicKey },
          idempotencyKey: sshRetryKey,
          expectedStatus: 409,
        },
      );
      expect(changedKey.body).toMatchObject({ error: { code: "conflict" } });
      expect(JSON.stringify(sshAccess)).not.toContain(sshKey.privateKeyPath);
      expect(JSON.stringify(sshAccess)).not.toContain("private_key");
    } finally {
      await Promise.all([sshKey.remove(), anotherSshKey.remove()]);
    }

    const unboundRun = await beginAgentRun(founder, companion.agent.id);
    const unboundWorkspace = await askAgentToBeDenied(
      unboundRun,
      1,
      "workspace.read_file",
      { path: notebookPath },
    );
    expect(unboundWorkspace).toContain("did not find a visible resource");
    await closeAgentRun(unboundRun);

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
    const notebookWriteKey = `private-agent-notebook-${randomUUID()}`;
    const writtenNotebook = await askAgentToAct(
      firstContext,
      2,
      "workspace.write_file",
      { path: notebookPath, content: notebook },
      notebookWriteKey,
    );
    const replayedWrite = await askAgentToAct(
      firstContext,
      3,
      "workspace.write_file",
      { path: notebookPath, content: notebook },
      notebookWriteKey,
    );
    expect(replayedWrite).toEqual(writtenNotebook);
    expect(writtenNotebook.ref).toEqual(
      expect.stringContaining(`/agent/workspace/${first.thread.workspace.id}`),
    );
    const writtenFile = record(writtenNotebook.data, "workspace write receipt");
    expect(writtenFile).toMatchObject({
      path: notebookPath,
      byte_len: Buffer.byteLength(notebook),
      workspace_generation: first.thread.workspace.generation,
    });

    const workspaceKey = await generateEphemeralSshKey();
    try {
      const workspaceAccess = await requestWorkspaceSshAccess(
        founder,
        first.thread.id,
        workspaceKey.publicKey,
      );
      const workspace = await connectToWorkspace(workspaceKey, workspaceAccess.access);
      expect(await workspace.hasInteractiveTerminal()).toBe(true);
      expect(await workspace.readText(notebookPath)).toBe(notebook);
      await workspace.writeText(ownerNotePath, ownerNote);
    } finally {
      await workspaceKey.remove();
    }
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
    const resumedNotebook = await askAgent(
      freshContext,
      2,
      "workspace.read_file",
      { path: notebookPath },
    );
    expect(resumedNotebook).toMatchObject({
      path: notebookPath,
      content: notebook,
      byte_len: Buffer.byteLength(notebook),
      content_digest: string(writtenFile.content_digest, "workspace write digest"),
      workspace_generation: first.thread.workspace.generation,
    });
    const noteFromOwner = await askAgent(
      freshContext,
      3,
      "workspace.read_file",
      { path: ownerNotePath },
    );
    expect(noteFromOwner).toMatchObject({
      path: ownerNotePath,
      content: ownerNote,
      byte_len: Buffer.byteLength(ownerNote),
      workspace_generation: first.thread.workspace.generation,
    });
    const fetched = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}`,
    );
    expect(parsePrivateAgentThread(fetched.body.thread)).toEqual(first.thread);
    expect(fetched.headers.get("cache-control")).toBe("no-store");
    expect(fetched.body.thread).not.toHaveProperty("storage_locator");
    expect(fetched.body.thread).not.toHaveProperty("failure_reason");
    expect(fetched.body.thread).not.toHaveProperty("tenant_id");

    const expiryKey = await generateEphemeralSshKey();
    try {
      const accessBeforeExpiry = await requestWorkspaceSshAccess(
        founder,
        first.thread.id,
        expiryKey.publicKey,
      );
      const workspaceBeforeExpiry = await connectToWorkspace(expiryKey, accessBeforeExpiry.access);
      expect(await workspaceBeforeExpiry.readText(notebookPath)).toBe(notebook);

      const inaccessible = await reconcileAgentThreads(first.thread.workspace.expires_at);
      expect(inaccessible.madeInaccessible).toBeGreaterThanOrEqual(1);
      expect(inaccessible.cleanupFailures).toBe(0);

      const expiring = await founder.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}`,
      );
      expect(parsePrivateAgentThread(expiring.body.thread).workspace.state).toBe("expiring");
      await founder.json(
        `/v1/chat/conversations/${encodeURIComponent(conversation.id)}/messages?limit=10`,
        { expectedStatus: 404 },
      );
      await founder.json(
        `/v1/chat/conversations/${encodeURIComponent(conversation.id)}/messages`,
        {
          method: "POST",
          body: { content: "An expired thread cannot quietly become live again." },
          expectedStatus: 404,
        },
      );
      await founder.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/messages?limit=10`,
        { expectedStatus: 404 },
      );
      await founder.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/messages`,
        {
          method: "POST",
          body: { content: "An expired aggregate cannot quietly become live again." },
          idempotencyKey: `expired-thread-message-${randomUUID()}`,
          expectedStatus: 404,
        },
      );
      await founder.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/runs`,
        {
          method: "POST",
          body: {},
          idempotencyKey: `expired-thread-run-${randomUUID()}`,
          expectedStatus: 409,
        },
      );
      await founder.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/ssh-access`,
        {
          method: "POST",
          body: { public_key: expiryKey.publicKey },
          idempotencyKey: `expired-workspace-ssh-${randomUUID()}`,
          expectedStatus: 409,
        },
      );
      await systemClient.json("/v1/whoami", {
        token: freshContext.credential.token,
        tokenScheme: "agent",
        expectedStatus: 401,
      });
      await expect(workspaceBeforeExpiry.readText(notebookPath)).rejects.toThrow(
        "the host-key-pinned workspace SSH command failed",
      );

      const cleanupClock = new Date(
        Date.parse(first.thread.workspace.expires_at) + 30_000,
      ).toISOString();
      const cleaned = await reconcileAgentThreads(cleanupClock);
      expect(cleaned.deleted).toBeGreaterThanOrEqual(1);
      expect(cleaned.cleanupFailures).toBe(0);

      const deleted = await founder.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}`,
      );
      const receipt = parsePrivateAgentThread(deleted.body.thread);
      expect(receipt.workspace).toMatchObject({
        id: first.thread.workspace.id,
        generation: first.thread.workspace.generation,
        state: "deleted",
        expires_at: first.thread.workspace.expires_at,
      });
      expect(await listPrivateAgentThreads(founder)).not.toEqual(
        expect.arrayContaining([expect.objectContaining({ id: first.thread.id })]),
      );

      const retriedReceipt = await startPrivateAgentThread(founder, {
        name: threadName,
        agentId: companion.agent.id,
        retentionDays: 3,
        idempotencyKey: retryKey,
        expectedStatus: 200,
      });
      expect(retriedReceipt).toEqual({ created: false, thread: receipt });
    } finally {
      await expiryKey.remove();
    }
  }, 120_000);
});
