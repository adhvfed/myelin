import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import {
  browserApprovedCliClient,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { systemTestConfig } from "../src/config.js";
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
  retryPrivateAgentThread,
  startPrivateAgentThread,
} from "../src/journeys/agent-threads.js";
import { Conversation } from "../src/journeys/chat.js";
import { GitProject } from "../src/git-project.js";
import { array, record, string } from "../src/json.js";
import { reconcileAgentThreads } from "../src/operator.js";
import {
  connectMultiplexedToWorkspace,
  connectToWorkspace,
  expectWorkspaceCommandRejected,
  generateEphemeralSshKey,
  type WorkspaceOverSsh,
} from "../src/ssh.js";

const THREE_DAYS_MS = 3 * 24 * 60 * 60 * 1_000;

describe("private work with an agent", () => {
  test("starts from an exact repository commit on a durable private branch", async () => {
    const founder = await browserApprovedCliClient();
    const companion = await activateExternalAgent(
      founder,
      uniqueName("Cloud branch companion"),
      ["workspace.read_file", "workspace.exec"],
    );
    const repository = new GitProject(uniqueName("cloud-workspace"), founder);
    await repository.create();
    const initialReadme = "# A real project\n\nCheckout the durable source, not a synthetic fixture.\n";
    const initial = await repository.writeFile("main", "README.md", initialReadme);
    const idempotencyKey = `repository-agent-thread-${randomUUID()}`;

    const teammate = await browserApprovedCliClient(reviewerClient);
    const teammateCompanion = await activateExternalAgent(
      teammate,
      uniqueName("Other cloud branch companion"),
      ["workspace.read_file"],
    );
    const hiddenThreadName = uniqueName("Cannot see somebody else's source");
    const hiddenSource = await teammate.json("/v1/agent-threads", {
      method: "POST",
      body: {
        name: hiddenThreadName,
        agent_id: teammateCompanion.agent.id,
        workspace: { repository: repository.slug },
      },
      idempotencyKey: `hidden-repository-thread-${randomUUID()}`,
      expectedStatus: 404,
    });
    expect(hiddenSource.body).toMatchObject({ error: { code: "not_found" } });
    expect(await listPrivateAgentThreads(teammate)).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ name: hiddenThreadName })]),
    );

    const invalidRef = await founder.json("/v1/agent-threads", {
      method: "POST",
      body: {
        name: uniqueName("Invalid tag workspace"),
        agent_id: companion.agent.id,
        workspace: { repository: repository.slug, base_ref: "refs/tags/v1" },
      },
      idempotencyKey: `invalid-repository-ref-${randomUUID()}`,
      expectedStatus: 400,
    });
    expect(invalidRef.body).toMatchObject({ error: { code: "bad_request" } });

    const created = await startPrivateAgentThread(founder, {
      name: uniqueName("Talk through my cloud branch"),
      agentId: companion.agent.id,
      idempotencyKey,
      workspace: { repository: repository.slug },
    });
    expect(created.thread.workspace.repository).toEqual({
      slug: repository.slug,
      base_ref: "refs/heads/main",
      base_oid: initial.commitOid,
      branch: expect.stringMatching(/^myelin\/thread-[0-9a-f]{20}$/),
    });

    await repository.updateFile(
      "main",
      "README.md",
      "# A newer trunk\n",
    );
    const replay = await startPrivateAgentThread(founder, {
      name: created.thread.name,
      agentId: companion.agent.id,
      idempotencyKey,
      workspace: { repository: repository.slug },
      expectedStatus: 200,
    });
    expect(replay).toEqual({ created: false, thread: created.thread });

    const run = await beginAgentThreadRun(founder, created.thread.id);
    try {
      const readme = await askAgent(run, 1, "workspace.read_file", { path: "README.md" });
      expect(readme).toMatchObject({ content: initialReadme });
      const repositoryReceipt = record(
        (await askAgentToAct(
          run,
          2,
          "workspace.exec",
          {
            command: "git branch --show-current && git rev-parse HEAD && git remote get-url origin",
            timeout_seconds: 10,
          },
          `inspect-cloud-branch-${randomUUID()}`,
        )).data,
        "repository workspace command",
      );
      expect(string(repositoryReceipt.stdout, "repository workspace stdout").trim().split("\n"))
        .toEqual([
          created.thread.workspace.repository!.branch,
          initial.commitOid,
          expect.stringContaining(`/${repository.slug}.git`),
        ]);
    } finally {
      await closeAgentRun(run);
    }
  });

  test("refuses an agent runtime that cannot resume private work before provisioning anything", async () => {
    const founder = await browserApprovedCliClient();
    const threadName = uniqueName("Hosted thread that must not exist");
    const activated = await founder.json("/v1/agents", {
      method: "POST",
      body: {
        name: uniqueName("Hosted automation specialist"),
        runtime: "hosted",
        tools: ["chat.read_messages"],
      },
      idempotencyKey: `hosted-agent-${randomUUID()}`,
      expectedStatus: 201,
    });
    const hostedAgent = record(activated.body.agent, "hosted agent");

    const rejected = await founder.json("/v1/agent-threads", {
      method: "POST",
      body: {
        name: threadName,
        agent_id: string(hostedAgent.id, "hosted agent id"),
        retention_days: 3,
      },
      idempotencyKey: `hosted-agent-thread-${randomUUID()}`,
      expectedStatus: 400,
    });

    expect(rejected.body).toMatchObject({ error: { code: "bad_request" } });
    expect(await listPrivateAgentThreads(founder)).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ name: threadName })]),
    );
  });

  test("lists the newest named work first and binds continuation cursors to its owner", async () => {
    const founder = await browserApprovedCliClient();
    const teammate = await browserApprovedCliClient(reviewerClient);
    const companion = await activateExternalAgent(
      founder,
      uniqueName("Recent work companion"),
      ["chat.read_messages", "workspace.read_file"],
    );
    const created = [];
    for (const label of ["Oldest", "Middle", "Newest"]) {
      created.push(await startPrivateAgentThread(founder, {
        name: uniqueName(`${label} private investigation`),
        agentId: companion.agent.id,
      }));
      await new Promise((resolve) => setTimeout(resolve, 2));
    }

    const first = await founder.json("/v1/agent-threads?limit=2");
    const firstItems = array(first.body.items, "newest private thread page")
      .map((item) => parsePrivateAgentThread(item));
    expect(firstItems.map((thread) => thread.id)).toEqual([
      created[2]!.thread.id,
      created[1]!.thread.id,
    ]);
    expect(first.body.page).toEqual({
      next_cursor: created[1]!.thread.id,
      limit: 2,
    });

    const second = await founder.json(
      `/v1/agent-threads?limit=2&cursor=${encodeURIComponent(created[1]!.thread.id)}`,
    );
    const secondItems = array(second.body.items, "continued private thread page")
      .map((item) => parsePrivateAgentThread(item));
    expect(secondItems[0]?.id).toBe(created[0]!.thread.id);

    for (const [client, cursor] of [
      [founder, randomUUID()],
      [teammate, created[1]!.thread.id],
    ] as const) {
      const rejected = await client.json(
        `/v1/agent-threads?limit=2&cursor=${encodeURIComponent(cursor)}`,
        { expectedStatus: 400 },
      );
      expect(rejected.body).toMatchObject({ error: { code: "bad_request" } });
    }
  });

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
        "workspace.exec",
      ],
    );
    const threadName = uniqueName("Investigate checkout race");
    const retryKey = `private-agent-thread-${randomUUID()}`;
    const notebookPath = "notes/continuity.md";
    const notebook = `${uniqueName("Checkout investigation")}\n\nThe final reader still owns the lease.`;
    const ownerRevisedNotebook = `${notebook}\nOwner note: preserve this newer diagnosis.`;
    const ownerNotePath = "notes/from-owner.md";
    const ownerNote = `${uniqueName("Owner observation")}: cleanup waits for the final reader.`;
    const executionLogPath = "notes/agent-command.log";
    const executionMarker = `agent-command-${randomUUID()}`;

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

    const recoveredAfterReload = await retryPrivateAgentThread(founder, first.thread.id);
    expect(recoveredAfterReload).toEqual({ created: false, thread: first.thread });
    await teammate.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/retry`,
      {
        method: "POST",
        body: {},
        expectedStatus: 404,
      },
    );

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
    await teammate.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/workspace-sessions?limit=10`,
      { expectedStatus: 404 },
    );

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

      const historyBeforeRejectedLaunch = await founder.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/workspace-sessions?limit=10`,
      );
      expect(array(historyBeforeRejectedLaunch.body.items, "workspace history before rejection"))
        .toEqual([]);
      const commandOneBytePastTheConfinementLimit = "x".repeat(65_537);
      await expectWorkspaceCommandRejected(
        sshKey,
        sshAccess.access,
        commandOneBytePastTheConfinementLimit,
      );
      await expectWorkspaceCommandRejected(sshKey, sshAccess.access, "printf should-not-run");
      const historyAfterRejectedLaunch = await founder.json(
        `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/workspace-sessions?limit=10`,
      );
      expect(array(historyAfterRejectedLaunch.body.items, "workspace history after rejection"))
        .toEqual([]);
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

    const commandKey = `private-agent-command-${randomUUID()}`;
    const command = [
      `printf '%s\\n' '${executionMarker}' >> ${executionLogPath}`,
      "printf 'workspace command complete\\n'",
    ].join("; ");
    const executed = await askAgentToAct(
      firstContext,
      4,
      "workspace.exec",
      { command, timeout_seconds: 10 },
      commandKey,
    );
    const replayedExecution = await askAgentToAct(
      firstContext,
      5,
      "workspace.exec",
      { command, timeout_seconds: 10 },
      commandKey,
    );
    expect(replayedExecution).toEqual(executed);
    expect(record(executed.data, "workspace command result")).toMatchObject({
      exit_code: 0,
      stdout: "workspace command complete\n",
      stderr: "",
      timed_out: false,
      cancelled: false,
      output_limit_exceeded: false,
      workspace_generation: first.thread.workspace.generation,
    });

    const useFreshWorkspace = async <T>(
      action: (workspace: WorkspaceOverSsh) => Promise<T>,
    ): Promise<T> => {
      const key = await generateEphemeralSshKey();
      try {
        const access = await requestWorkspaceSshAccess(founder, first.thread.id, key.publicKey);
        return await action(await connectToWorkspace(key, access.access));
      } finally {
        await key.remove();
      }
    };

    const workspaceKey = await generateEphemeralSshKey();
    try {
      const workspaceAccess = await requestWorkspaceSshAccess(
        founder,
        first.thread.id,
        workspaceKey.publicKey,
      );
      const workspace = await connectToWorkspace(workspaceKey, workspaceAccess.access);
      expect(await workspace.hasInteractiveTerminal()).toBe(true);
      await expect(workspace.readText(notebookPath)).rejects.toThrow(
        "the host-key-pinned workspace SSH command failed",
      );
    } finally {
      await workspaceKey.remove();
    }
    expect(await useFreshWorkspace((workspace) => workspace.readText(notebookPath))).toBe(notebook);
    expect(await useFreshWorkspace((workspace) => workspace.readText(executionLogPath)))
      .toBe(`${executionMarker}\n`);
    await useFreshWorkspace((workspace) => workspace.writeText(ownerNotePath, ownerNote));
    await useFreshWorkspace((workspace) => workspace.writeText(notebookPath, ownerRevisedNotebook));
    const replayAfterOwnerEdit = await askAgentToAct(
      firstContext,
      6,
      "workspace.write_file",
      { path: notebookPath, content: notebook },
      notebookWriteKey,
    );
    expect(replayAfterOwnerEdit).toEqual(writtenNotebook);
    const conflictingWriteRetry = await askAgentToBeDenied(
      firstContext,
      7,
      "workspace.write_file",
      { path: notebookPath, content: `${notebook}\nstale retry` },
      notebookWriteKey,
    );
    expect(conflictingWriteRetry).toContain(
      "idempotency key was already used for another workspace mutation",
    );
    expect(await useFreshWorkspace((workspace) => workspace.readText(notebookPath)))
      .toBe(ownerRevisedNotebook);

    const editorKey = await generateEphemeralSshKey();
    const editorPath = "notes/editor-multiplexing.txt";
    const editorText = "one pinned SSH transport, multiple confined channels";
    const standardEditorPath = "notes/standard-editor-sftp.txt";
    const standardEditorText = "edited through the standard OpenSSH SFTP client";
    try {
      const editorAccess = await requestWorkspaceSshAccess(
        founder,
        first.thread.id,
        editorKey.publicKey,
      );
      const editorWorkspace = await connectMultiplexedToWorkspace(
        editorKey,
        editorAccess.access,
      );
      try {
        await Promise.all([
          editorWorkspace.waitForText(editorPath, editorText),
          editorWorkspace.writeText(editorPath, editorText),
        ]);
        expect(await editorWorkspace.readText(editorPath)).toBe(editorText);
        expect(await editorWorkspace.readText(notebookPath)).toBe(ownerRevisedNotebook);
        await editorWorkspace.writeTextViaSftp(standardEditorPath, standardEditorText);
        expect(await editorWorkspace.readTextViaSftp(standardEditorPath)).toBe(standardEditorText);
        expect(await editorWorkspace.readText(standardEditorPath)).toBe(standardEditorText);
      } finally {
        await editorWorkspace.close();
      }
    } finally {
      await editorKey.remove();
    }
    const workspaceHistory = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/workspace-sessions?limit=20`,
    );
    const workspaceSessions = array(
      workspaceHistory.body.items,
      "private workspace access history",
    );
    for (const candidate of workspaceSessions) {
      const session = record(candidate, "private workspace session");
      expect(session.ref).toBe(
        `myelin://${systemTestConfig.tenant}/agent/session/${string(session.id, "workspace session id")}`,
      );
    }
    expect(workspaceSessions).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          method: "ssh",
          mode: "command",
          terminal: true,
          workspace: {
            id: first.thread.workspace.id,
            generation: first.thread.workspace.generation,
          },
        }),
        expect.objectContaining({
          method: "ssh",
          mode: "file_transfer",
          terminal: false,
          workspace: {
            id: first.thread.workspace.id,
            generation: first.thread.workspace.generation,
          },
        }),
        expect.objectContaining({
          method: "ssh",
          mode: "command",
          terminal: false,
          workspace: {
            id: first.thread.workspace.id,
            generation: first.thread.workspace.generation,
          },
        }),
      ]),
    );
    expect(workspaceHistory.body.page).toEqual({ next_cursor: null, limit: 20 });
    expect(workspaceHistory.headers.get("cache-control")).toBe("no-store");
    expect(JSON.stringify(workspaceHistory.body)).not.toMatch(
      /fingerprint|grant|host|locator|public_key|route|username/i,
    );
    expect(workspaceSessions.every((session) => !("command" in record(session, "workspace session"))))
      .toBe(true);
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
      content: ownerRevisedNotebook,
      byte_len: Buffer.byteLength(ownerRevisedNotebook),
      workspace_generation: first.thread.workspace.generation,
    });
    expect(resumedNotebook.content_digest).not.toBe(
      string(writtenFile.content_digest, "workspace write digest"),
    );
    const executionLog = await askAgent(
      freshContext,
      3,
      "workspace.read_file",
      { path: executionLogPath },
    );
    expect(executionLog).toMatchObject({
      path: executionLogPath,
      content: `${executionMarker}\n`,
      workspace_generation: first.thread.workspace.generation,
    });
    const noteFromOwner = await askAgent(
      freshContext,
      4,
      "workspace.read_file",
      { path: ownerNotePath },
    );
    expect(noteFromOwner).toMatchObject({
      path: ownerNotePath,
      content: ownerNote,
      byte_len: Buffer.byteLength(ownerNote),
      workspace_generation: first.thread.workspace.generation,
    });
    const standardEditorFile = await askAgent(
      freshContext,
      5,
      "workspace.read_file",
      { path: standardEditorPath },
    );
    expect(standardEditorFile).toMatchObject({
      path: standardEditorPath,
      content: standardEditorText,
      byte_len: Buffer.byteLength(standardEditorText),
      workspace_generation: first.thread.workspace.generation,
    });
    const answer = "The cleanup races because the final reader still owns its lease.";
    const answerKey = `private-agent-answer-${randomUUID()}`;
    const postedAnswer = await askAgentToAct(
      freshContext,
      6,
      "chat.post",
      { conversation_id: conversation.id, content: answer },
      answerKey,
    );
    const replayedAnswer = await askAgentToAct(
      freshContext,
      7,
      "chat.post",
      { conversation_id: conversation.id, content: answer },
      answerKey,
    );
    expect(replayedAnswer).toEqual(postedAnswer);
    const answeredHistory = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(first.thread.id)}/messages?limit=10`,
    );
    expect(
      array(answeredHistory.body.items, "answered private thread messages")
        .filter((message) => record(message, "private thread message").content === answer),
    ).toEqual([
      expect.objectContaining({
        author_kind: "agent",
        is_you: false,
        state: "active",
      }),
    ]);
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
      expect(await useFreshWorkspace((workspace) => workspace.readText(notebookPath)))
        .toBe(ownerRevisedNotebook);

      const runningCommandPath = "notes/running-command.txt";
      const runningCommand = askAgentToAct(
        freshContext,
        8,
        "workspace.exec",
        {
          command: [
            `printf 'running\\n' > ${runningCommandPath}`,
            "sleep 30",
            `printf 'finished\\n' >> ${runningCommandPath}`,
          ].join("; "),
          timeout_seconds: 60,
        },
        `private-agent-cancel-${randomUUID()}`,
      );
      await useFreshWorkspace((workspace) => workspace.waitForText(runningCommandPath, "running"));

      const inaccessible = await reconcileAgentThreads(first.thread.workspace.expires_at);
      expect(inaccessible.madeInaccessible).toBeGreaterThanOrEqual(1);
      expect(inaccessible.cleanupFailures).toBe(0);
      const cancelledCommand = record(
        (await runningCommand).data,
        "cancelled workspace command",
      );
      expect(cancelledCommand).toMatchObject({
        timed_out: false,
        cancelled: true,
        output_limit_exceeded: false,
        workspace_generation: first.thread.workspace.generation,
      });
      expect(cancelledCommand.exit_code).not.toBe(0);
      expect(Number(cancelledCommand.elapsed_ms)).toBeLessThan(30_000);

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
