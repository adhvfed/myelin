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
  markAgentThreadReadyForReview,
  parsePrivateAgentThread,
  requestWorkspaceSshAccess,
  retryPrivateAgentThread,
  startPrivateAgentThread,
} from "../src/journeys/agent-threads.js";
import { Conversation } from "../src/journeys/chat.js";
import { GitProject } from "../src/git-project.js";
import { array, integer, record, string } from "../src/json.js";
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
  test("keeps project-bound work private until its owner marks the conversation ready", async () => {
    const founder = await browserApprovedCliClient();
    const teammate = await browserApprovedCliClient(reviewerClient);
    const collaborators = await founder.json(
      `/v1/projects/${encodeURIComponent(systemTestConfig.issues.projectId)}/collaborators?limit=100`,
    );
    expect(array(collaborators.body.items, "project collaborators")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          principal_id: systemTestConfig.reviewerPrincipal,
          display_name: expect.any(String),
        }),
      ]),
    );
    const companion = await activateExternalAgent(
      founder,
      uniqueName("Private draft companion"),
      ["chat.read_messages"],
    );
    const created = await startPrivateAgentThread(founder, {
      name: uniqueName("Draft before project review"),
      agentId: companion.agent.id,
      projectId: systemTestConfig.issues.projectId,
    });
    expect(created.thread).toMatchObject({
      project_id: systemTestConfig.issues.projectId,
      review_state: "draft",
      review_ready_at: null,
    });

    const privateMessage = "This diagnosis is private until I say it is ready.";
    await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/messages`,
      {
        method: "POST",
        body: { content: privateMessage },
        idempotencyKey: `private-draft-message-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    const hiddenRooms = await reviewerClient.json("/v1/chat/conversations?limit=100");
    expect(array(hiddenRooms.body.items, "collaborator rooms before review")).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ id: created.thread.conversation_id })]),
    );
    await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(created.thread.conversation_id)}/messages?limit=100`,
      { expectedStatus: 404 },
    );
    await teammate.json(`/v1/agent-threads/${encodeURIComponent(created.thread.id)}`, {
      expectedStatus: 404,
    });
    await teammate.json(`/v1/agent-threads/${encodeURIComponent(created.thread.id)}/ready`, {
      method: "POST",
      body: {},
      expectedStatus: 404,
    });

    const published = await markAgentThreadReadyForReview(founder, created.thread.id);
    expect(published.changed).toBe(true);
    expect(published.thread).toMatchObject({
      id: created.thread.id,
      conversation_id: created.thread.conversation_id,
      review_state: "ready",
      review_ready_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T/),
    });
    const replay = await markAgentThreadReadyForReview(founder, created.thread.id);
    expect(replay).toEqual({ changed: false, thread: published.thread });

    const visibleRooms = await reviewerClient.json("/v1/chat/conversations?limit=100");
    expect(array(visibleRooms.body.items, "collaborator rooms after review")).toEqual(
      expect.arrayContaining([expect.objectContaining({
        id: created.thread.conversation_id,
        kind: "channel_public",
        project_id: systemTestConfig.issues.projectId,
      })]),
    );
    const conversation = new Conversation(created.thread.conversation_id, {});
    expect(await conversation.messages(reviewerClient)).toEqual(
      expect.arrayContaining([expect.objectContaining({ content: privateMessage })]),
    );
    const reviewMessage = "Reviewed from a second project principal.";
    await conversation.post(reviewerClient, reviewMessage);
    const ownerHistory = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/messages?limit=100`,
    );
    expect(array(ownerHistory.body.items, "owner history after review")).toEqual(
      expect.arrayContaining([expect.objectContaining({ content: reviewMessage })]),
    );

    await teammate.json(`/v1/agent-threads/${encodeURIComponent(created.thread.id)}/runs`, {
      method: "POST",
      body: {},
      idempotencyKey: `non-owner-run-${randomUUID()}`,
      expectedStatus: 404,
    });
    const key = await generateEphemeralSshKey();
    try {
      await teammate.json(
        `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/ssh-access`,
        {
          method: "POST",
          body: { public_key: key.publicKey },
          idempotencyKey: `non-owner-ssh-${randomUUID()}`,
          expectedStatus: 404,
        },
      );
    } finally {
      await key.remove();
    }

    const freshOwner = await browserApprovedCliClient();
    const fetched = await freshOwner.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}`,
    );
    expect(parsePrivateAgentThread(fetched.body.thread)).toEqual(published.thread);
    const ownerRun = await beginAgentThreadRun(freshOwner, created.thread.id);
    await closeAgentRun(ownerRun);
  });

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
    await repository.writeFile("main", "README.md", initialReadme);
    const declaredEnvironment = [
      "parameters:",
      "  APP_PORT: { type: port, default: 8000 }",
      "services:",
      "  app:",
      "    process: python3 -m http.server {{APP_PORT}}",
      "    startup_message: App ready at http://localhost:{{APP_PORT}}",
      "entrypoint: app",
      "",
    ].join("\n");
    const initial = await repository.writeFile("main", "fed.yaml", declaredEnvironment);
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
      projectId: systemTestConfig.issues.projectId,
      idempotencyKey,
      workspace: {
        repository: repository.slug,
        instructions: "Keep the owner's exploratory notes private during review handoff.",
        files: [{
          path: ".myelin/private-review-context.md",
          contents: "This owner-only context must never enter the review branch.\n",
          strategy: "create",
        }],
      },
    });
    expect(created.thread.workspace.repository).toEqual({
      slug: repository.slug,
      base_ref: "refs/heads/main",
      base_oid: initial.commitOid,
      branch: expect.stringMatching(/^myelin\/thread-[0-9a-f]{20}$/),
    });
    expect(created.thread.workspace.environment).toEqual({
      provider: "service_federation",
      config_path: "fed.yaml",
      content_digest: expect.stringMatching(/^blake3:[0-9a-f]{64}$/),
      state: "declared",
    });

    await repository.updateFile(
      "main",
      "README.md",
      "# A newer trunk\n",
    );
    await repository.updateFile(
      "main",
      "fed.yaml",
      "services:\n  replacement:\n    process: false\nentrypoint: replacement\n",
    );
    const replay = await startPrivateAgentThread(founder, {
      name: created.thread.name,
      agentId: companion.agent.id,
      projectId: systemTestConfig.issues.projectId,
      idempotencyKey,
      workspace: {
        repository: repository.slug,
        instructions: "Keep the owner's exploratory notes private during review handoff.",
        files: [{
          path: ".myelin/private-review-context.md",
          contents: "This owner-only context must never enter the review branch.\n",
          strategy: "create",
        }],
      },
      expectedStatus: 200,
    });
    expect(replay).toEqual({ created: false, thread: created.thread });
    expect(await listPrivateAgentThreads(founder)).toEqual(
      expect.arrayContaining([created.thread]),
    );
    const fetched = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}`,
    );
    expect(parsePrivateAgentThread(fetched.body.thread)).toEqual(created.thread);
    expect(await retryPrivateAgentThread(founder, created.thread.id)).toEqual({
      created: false,
      thread: created.thread,
    });

    const run = await beginAgentThreadRun(founder, created.thread.id);
    try {
      const readme = await askAgent(run, 1, "workspace.read_file", { path: "README.md" });
      expect(readme).toMatchObject({ content: initialReadme });
      const environment = await askAgent(run, 2, "workspace.read_file", { path: "fed.yaml" });
      expect(environment).toMatchObject({ content: declaredEnvironment });
      const repositoryReceipt = record(
        (await askAgentToAct(
          run,
          3,
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
      const committed = record(
        (await askAgentToAct(
          run,
          4,
          "workspace.exec",
          {
            command: [
              "git config user.name 'Myelin cloud agent'",
              "git config user.email 'agent@myelin.invalid'",
              "printf 'export const reviewed = true;\\n' > review-ready.ts",
              "git add review-ready.ts",
              "git commit -m 'Prepare private cloud branch for review'",
              "git rev-parse HEAD",
            ].join(" && "),
            timeout_seconds: 10,
          },
          `commit-cloud-branch-${randomUUID()}`,
        )).data,
        "committed cloud branch command",
      );
      expect(string(committed.stdout, "committed cloud branch stdout"))
        .toContain("Prepare private cloud branch for review");
    } finally {
      await closeAgentRun(run);
    }

    const branch = created.thread.workspace.repository!.branch;
    await teammate.json(`${repository.path}/blob/${encodeURIComponent(branch)}/review-ready.ts`, {
      expectedStatus: 404,
    });
    await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/pull-request`,
      {
        method: "POST",
        body: {},
        idempotencyKey: `premature-review-handoff-${randomUUID()}`,
        expectedStatus: 409,
      },
    );
    await markAgentThreadReadyForReview(founder, created.thread.id);
    const handoffKey = `private-review-handoff-${randomUUID()}`;
    const opened = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/pull-request`,
      {
        method: "POST",
        body: {
          title: "Review the private cloud branch",
          body_md: "The owner has finished the private exploration and is ready for a human review.",
          reviewers: [systemTestConfig.reviewerPrincipal],
        },
        idempotencyKey: handoffKey,
        expectedStatus: 201,
      },
    );
    const handoff = record(opened.body.handoff, "private branch review handoff");
    const pullRequest = record(handoff.pull_request, "private branch pull request");
    const pullRequestNumber = integer(pullRequest.number, "private branch pull request number");
    expect(handoff).toMatchObject({
      thread_id: created.thread.id,
      repository: repository.slug,
      base_ref: "refs/heads/main",
      head_ref: `refs/heads/${branch}`,
      head_oid: expect.stringMatching(/^[0-9a-f]{40}$/),
      created_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T/),
      updated_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T/),
    });
    expect(opened.body).toMatchObject({ created: true, updated: false, durable: true });
    const replayedHandoff = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/pull-request`,
      {
        method: "POST",
        body: {
          title: "Review the private cloud branch",
          body_md: "The owner has finished the private exploration and is ready for a human review.",
          reviewers: [systemTestConfig.reviewerPrincipal],
        },
        idempotencyKey: handoffKey,
        expectedStatus: 200,
      },
    );
    expect(replayedHandoff.body).toEqual({
      handoff,
      created: false,
      updated: false,
      durable: true,
    });
    const loadedHandoff = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/pull-request`,
    );
    expect(loadedHandoff.body).toEqual({ handoff, durable: true });
    expect((await repository.readFile(branch, "review-ready.ts")).contents)
      .toBe("export const reviewed = true;\n");
    await founder.json(
      `${repository.path}/blob/${encodeURIComponent(branch)}/.myelin/private-review-context.md`,
      { expectedStatus: 404 },
    );
    await founder.json(`${repository.path}/blob/${encodeURIComponent(branch)}/AGENTS.md`, {
      expectedStatus: 404,
    });
    const review = await teammate.json(`${repository.path}/prs/${pullRequestNumber}`);
    expect(review.body).toMatchObject({
      number: pullRequestNumber,
      title: "Review the private cloud branch",
      head_ref: `refs/heads/${branch}`,
      head_oid: handoff.head_oid,
      pr_state: "open",
    });
    await teammate.json(`${repository.path}/prs/${pullRequestNumber}/reviews`, {
      method: "POST",
      body: { verdict: "approve" },
      idempotencyKey: `approve-private-review-${randomUUID()}`,
    });
    expect((await teammate.json(`${repository.path}/prs/${pullRequestNumber}/checks`)).body)
      .toMatchObject({ current_approvals: 1 });

    const followUpRun = await beginAgentThreadRun(founder, created.thread.id);
    try {
      const committed = record(
        (await askAgentToAct(
          followUpRun,
          1,
          "workspace.exec",
          {
            command: [
              "printf 'export const followedUp = true;\\n' > review-follow-up.ts",
              "git add review-follow-up.ts",
              "git commit -m 'Follow up on the private review'",
              "git rev-parse HEAD",
            ].join(" && "),
            timeout_seconds: 10,
          },
          `follow-up-cloud-review-${randomUUID()}`,
        )).data,
        "follow-up cloud branch command",
      );
      expect(string(committed.stdout, "follow-up cloud branch stdout"))
        .toContain("Follow up on the private review");
    } finally {
      await closeAgentRun(followUpRun);
    }

    const updateKey = `private-review-update-${randomUUID()}`;
    const updated = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/pull-request`,
      {
        method: "POST",
        body: {},
        idempotencyKey: updateKey,
      },
    );
    const updatedHandoff = record(updated.body.handoff, "updated private branch handoff");
    expect(updated.body).toMatchObject({ created: false, updated: true, durable: true });
    expect(updatedHandoff).toMatchObject({
      thread_id: created.thread.id,
      repository: repository.slug,
      head_ref: `refs/heads/${branch}`,
      head_oid: expect.stringMatching(/^[0-9a-f]{40}$/),
      pull_request: { number: pullRequestNumber },
      created_at: handoff.created_at,
    });
    expect(updatedHandoff.head_oid).not.toBe(handoff.head_oid);
    expect(updatedHandoff.updated_at).not.toBe(handoff.updated_at);
    expect((await repository.readFile(branch, "review-follow-up.ts")).contents)
      .toBe("export const followedUp = true;\n");
    expect((await teammate.json(`${repository.path}/prs/${pullRequestNumber}`)).body)
      .toMatchObject({
        number: pullRequestNumber,
        head_ref: `refs/heads/${branch}`,
        head_oid: updatedHandoff.head_oid,
        pr_state: "open",
      });
    expect((await teammate.json(`${repository.path}/prs/${pullRequestNumber}/checks`)).body)
      .toMatchObject({ current_approvals: 0 });
    const replayedUpdate = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/pull-request`,
      {
        method: "POST",
        body: {},
        idempotencyKey: updateKey,
      },
    );
    expect(replayedUpdate.body).toEqual({
      handoff: updatedHandoff,
      created: false,
      updated: false,
      durable: true,
    });

    const nextRun = await beginAgentThreadRun(founder, created.thread.id);
    try {
      await askAgentToAct(
        nextRun,
        1,
        "workspace.exec",
        {
          command: [
            "printf 'export const finalFollowUp = true;\\n' > review-final.ts",
            "git add review-final.ts",
            "git commit -m 'Finish the private review follow-up'",
          ].join(" && "),
          timeout_seconds: 10,
        },
        `final-cloud-review-${randomUUID()}`,
      );
    } finally {
      await closeAgentRun(nextRun);
    }
    const reusedIntent = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/pull-request`,
      {
        method: "POST",
        body: {},
        idempotencyKey: updateKey,
        expectedStatus: 409,
      },
    );
    expect(reusedIntent.body).toMatchObject({ error: { code: "conflict" } });
    const finalUpdate = await founder.json(
      `/v1/agent-threads/${encodeURIComponent(created.thread.id)}/pull-request`,
      {
        method: "POST",
        body: {},
        idempotencyKey: `private-review-final-${randomUUID()}`,
      },
    );
    expect(finalUpdate.body).toMatchObject({ created: false, updated: true, durable: true });
    const finalHandoff = record(finalUpdate.body.handoff, "final private branch handoff");
    expect(finalHandoff).toMatchObject({
      pull_request: { number: pullRequestNumber },
      head_oid: expect.stringMatching(/^[0-9a-f]{40}$/),
    });
    expect(finalHandoff.head_oid).not.toBe(updatedHandoff.head_oid);
  });

  test("layers owner instructions and create-only context files across agent contexts", async () => {
    const founder = await browserApprovedCliClient();
    const companion = await activateExternalAgent(
      founder,
      uniqueName("Personal workflow companion"),
      ["workspace.read_file"],
    );
    const repository = new GitProject(uniqueName("personalized-workspace"), founder);
    await repository.create();
    const repositoryGuidance = "# Repository guidance\n\nRun the project formatter.\n";
    await repository.writeFile("main", "AGENTS.md", repositoryGuidance);
    const personalGuidance = [
      "# My way of working",
      "",
      "Explain the tradeoff before changing an interface.",
      "Use the smallest real system journey that proves the change.",
    ].join("\n");
    const contextPath = ".myelin/checkout-context.md";
    const contextContents = `# Private checkout context\n\nTrace ${repository.slug} before changing it.\n`;
    const retryKey = `personalized-agent-thread-${randomUUID()}`;

    const created = await startPrivateAgentThread(founder, {
      name: uniqueName("Use my workspace playbook"),
      agentId: companion.agent.id,
      idempotencyKey: retryKey,
      workspace: {
        repository: repository.slug,
        instructions: personalGuidance,
        files: [{ path: contextPath, contents: contextContents, strategy: "create" }],
      },
    });
    expect(created.thread.workspace.personalization).toEqual({
      format: "workspace_augmentation_v1",
      content_digest: expect.stringMatching(/^blake3:[0-9a-f]{64}$/),
    });

    const firstContext = await beginAgentThreadRun(founder, created.thread.id);
    const firstAgents = await askAgent(firstContext, 1, "workspace.read_file", {
      path: "AGENTS.md",
    });
    expect(firstAgents).toMatchObject({
      content: expect.stringContaining(repositoryGuidance),
    });
    expect(record(firstAgents, "personalized AGENTS result").content).toEqual(
      expect.stringContaining(personalGuidance),
    );
    const firstContextFile = await askAgent(firstContext, 2, "workspace.read_file", {
      path: contextPath,
    });
    await closeAgentRun(firstContext);
    expect(record(firstContextFile, "personalized context-file result").content)
      .toBe(contextContents);

    const replay = await startPrivateAgentThread(founder, {
      name: created.thread.name,
      agentId: companion.agent.id,
      idempotencyKey: retryKey,
      workspace: {
        repository: repository.slug,
        instructions: personalGuidance,
        files: [{ path: contextPath, contents: contextContents, strategy: "create" }],
      },
      expectedStatus: 200,
    });
    expect(replay).toEqual({ created: false, thread: created.thread });

    await founder.json("/v1/agent-threads", {
      method: "POST",
      body: {
        name: created.thread.name,
        agent_id: companion.agent.id,
        workspace: {
          repository: repository.slug,
          instructions: personalGuidance,
          files: [{
            path: contextPath,
            contents: `${contextContents}Use a different retry intent.\n`,
            strategy: "create",
          }],
        },
      },
      idempotencyKey: retryKey,
      expectedStatus: 409,
    });

    const resumedContext = await beginAgentThreadRun(founder, created.thread.id);
    const resumedAgents = await askAgent(resumedContext, 1, "workspace.read_file", {
      path: "AGENTS.md",
    });
    const resumedContextFile = await askAgent(resumedContext, 2, "workspace.read_file", {
      path: contextPath,
    });
    await closeAgentRun(resumedContext);
    expect(resumedAgents).toEqual(firstAgents);
    expect(resumedContextFile).toEqual(firstContextFile);
    expect((await repository.readFile("main", "AGENTS.md")).contents).toBe(repositoryGuidance);
    await founder.json(
      `${repository.path}/blob/main/.myelin/checkout-context.md`,
      { expectedStatus: 404 },
    );
  });

  test("carries editable named-agent defaults into new work while existing threads keep their snapshot", async () => {
    const founder = await browserApprovedCliClient();
    const teammate = await browserApprovedCliClient(reviewerClient);
    const firstDefaults = "# Checkout companion defaults\n\nExplain the smallest safe change first.";
    const defaultContextPath = ".myelin/companion-context.md";
    const firstDefaultContext = "# Companion context\n\nStart from the checkout lease.\n";
    const companion = await activateExternalAgent(
      founder,
      uniqueName("Configurable workspace companion"),
      ["workspace.read_file"],
      {
        workspaceInstructions: firstDefaults,
        workspaceFiles: [{
          path: defaultContextPath,
          contents: firstDefaultContext,
          strategy: "create",
        }],
      },
    );
    expect(companion.agent.workspace_profile).toMatchObject({
      revision: 1,
      personalization: {
        format: "workspace_augmentation_v1",
        content_digest: expect.stringMatching(/^blake3:[0-9a-f]{64}$/),
      },
      updated_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T/),
    });

    const privateConfig = await founder.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/workspace-guidance`,
    );
    expect(privateConfig.body).toMatchObject({
      agent_id: companion.agent.id,
      revision: 1,
      instructions: firstDefaults,
      files: [{
        path: defaultContextPath,
        contents: firstDefaultContext,
        strategy: "create",
      }],
    });
    await teammate.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/workspace-guidance`,
      { expectedStatus: 404 },
    );

    const firstRetry = `agent-default-thread-${randomUUID()}`;
    const firstThread = await startPrivateAgentThread(founder, {
      name: uniqueName("Keep original agent defaults"),
      agentId: companion.agent.id,
      idempotencyKey: firstRetry,
    });
    const firstRun = await beginAgentThreadRun(founder, firstThread.thread.id);
    const firstAgents = await askAgent(firstRun, 1, "workspace.read_file", {
      path: "AGENTS.md",
    });
    const firstContext = await askAgent(firstRun, 2, "workspace.read_file", {
      path: defaultContextPath,
    });
    await closeAgentRun(firstRun);
    expect(record(firstAgents, "first named-agent AGENTS result").content).toContain(firstDefaults);
    expect(record(firstContext, "first named-agent context result").content)
      .toBe(firstDefaultContext);

    const secondDefaults = "# Checkout companion defaults\n\nStart with the failing full-system journey.";
    const secondDefaultContext = "# Companion context\n\nStart from the durable retry ledger.\n";
    const updated = await founder.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/workspace-guidance`,
      {
        method: "POST",
        body: {
          instructions: secondDefaults,
          files: [{
            path: defaultContextPath,
            contents: secondDefaultContext,
            strategy: "create",
          }],
          expected_revision: 1,
        },
        idempotencyKey: `agent-default-update-${randomUUID()}`,
        expectedStatus: 200,
      },
    );
    expect(updated.body).toMatchObject({
      changed: true,
      durable: true,
      agent: { workspace_profile: { revision: 2 } },
    });
    const finalDefaults = `${secondDefaults}\nPreserve configured context files when an older client edits instructions.`;
    const legacyCompatibleUpdate = await founder.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/workspace-guidance`,
      {
        method: "POST",
        body: { instructions: finalDefaults, expected_revision: 2 },
        idempotencyKey: `agent-default-compatible-update-${randomUUID()}`,
        expectedStatus: 200,
      },
    );
    expect(legacyCompatibleUpdate.body).toMatchObject({
      changed: true,
      durable: true,
      agent: { workspace_profile: { revision: 3 } },
    });
    expect((await founder.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/workspace-guidance`,
    )).body).toMatchObject({
      revision: 3,
      instructions: finalDefaults,
      files: [{
        path: defaultContextPath,
        contents: secondDefaultContext,
        strategy: "create",
      }],
    });
    await founder.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/workspace-guidance`,
      {
        method: "POST",
        body: { instructions: "stale replacement", expected_revision: 1 },
        idempotencyKey: `agent-default-stale-${randomUUID()}`,
        expectedStatus: 409,
      },
    );

    const replay = await startPrivateAgentThread(founder, {
      name: firstThread.thread.name,
      agentId: companion.agent.id,
      idempotencyKey: firstRetry,
      expectedStatus: 200,
    });
    expect(replay).toEqual({ created: false, thread: firstThread.thread });
    const replayRun = await beginAgentThreadRun(founder, replay.thread.id);
    const replayAgents = await askAgent(replayRun, 1, "workspace.read_file", {
      path: "AGENTS.md",
    });
    const replayContext = await askAgent(replayRun, 2, "workspace.read_file", {
      path: defaultContextPath,
    });
    await closeAgentRun(replayRun);
    expect(replayAgents).toEqual(firstAgents);
    expect(replayContext).toEqual(firstContext);
    expect(JSON.stringify(replayAgents)).not.toContain(finalDefaults);
    expect(JSON.stringify(replayContext)).not.toContain(secondDefaultContext);

    const taskInstructions = "For this task, preserve the public API shape.";
    const secondThread = await startPrivateAgentThread(founder, {
      name: uniqueName("Use revised agent defaults"),
      agentId: companion.agent.id,
      workspace: { instructions: taskInstructions },
    });
    const secondRun = await beginAgentThreadRun(founder, secondThread.thread.id);
    const secondAgents = await askAgent(secondRun, 1, "workspace.read_file", {
      path: "AGENTS.md",
    });
    const secondContext = await askAgent(secondRun, 2, "workspace.read_file", {
      path: defaultContextPath,
    });
    await closeAgentRun(secondRun);
    const combined = string(
      record(secondAgents, "revised named-agent AGENTS result").content,
      "revised named-agent AGENTS contents",
    );
    expect(combined).toContain(finalDefaults);
    expect(combined).toContain("## Thread-specific instructions");
    expect(combined).toContain(taskInstructions);
    expect(combined).not.toContain(firstDefaults);
    expect(record(secondContext, "revised named-agent context result").content)
      .toBe(secondDefaultContext);
  });

  test("narrows a named agent's tool palette, closes old authority, and constrains fresh work", async () => {
    const founder = await browserApprovedCliClient();
    const teammate = await browserApprovedCliClient(reviewerClient);
    const instructions = "# Palette proof\n\nRead the assigned workspace before changing it.";
    const companion = await activateExternalAgent(
      founder,
      uniqueName("Least-authority companion"),
      ["workspace.read_file", "workspace.write_file"],
      { workspaceInstructions: instructions },
    );
    const thread = await startPrivateAgentThread(founder, {
      name: uniqueName("Constrain fresh agent work"),
      agentId: companion.agent.id,
    });
    const oldRun = await beginAgentThreadRun(founder, thread.thread.id);
    expect(oldRun.run.selected_tools.map((tool) => tool.name)).toEqual([
      "workspace.read_file",
      "workspace.write_file",
    ]);
    expect(record(await askAgent(oldRun, 1, "workspace.read_file", { path: "AGENTS.md" }),
      "workspace before palette reduction").content).toContain(instructions);

    await teammate.json(`/v1/agents/${encodeURIComponent(companion.agent.id)}/tools`, {
      method: "POST",
      body: {
        expected_tools: ["workspace.read_file", "workspace.write_file"],
        tools: ["workspace.read_file"],
      },
      idempotencyKey: `foreign-agent-tools-${randomUUID()}`,
      expectedStatus: 404,
    });
    const updateBody = {
      expected_tools: ["workspace.read_file", "workspace.write_file"],
      tools: ["workspace.read_file"],
    };
    const updated = await founder.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/tools`,
      {
        method: "POST",
        body: updateBody,
        idempotencyKey: `agent-tools-${randomUUID()}`,
        expectedStatus: 200,
      },
    );
    expect(updated.body).toMatchObject({
      agent: {
        id: companion.agent.id,
        selected_tools: [{ name: "workspace.read_file", version: 1 }],
      },
      changed: true,
      terminated_runs: 1,
      durable: true,
      governance: {
        policy_versions: {
          agent: expect.any(Number),
          delegation: expect.any(Number),
          tenant: expect.any(Number),
          trigger_actor: expect.any(Number),
        },
        policy_revisions: {
          agent: expect.any(Number),
          delegation: expect.any(Number),
          tenant: expect.any(Number),
          trigger_actor: expect.any(Number),
        },
      },
    });
    await systemClient.json("/v1/whoami", {
      token: oldRun.credential.token,
      tokenScheme: "agent",
      expectedStatus: 401,
    });

    const exactRetry = await founder.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/tools`,
      {
        method: "POST",
        body: updateBody,
        idempotencyKey: `agent-tools-retry-${randomUUID()}`,
        expectedStatus: 200,
      },
    );
    expect(exactRetry.body).toMatchObject({
      agent: { id: companion.agent.id },
      changed: false,
      terminated_runs: 0,
      durable: true,
    });
    await founder.json(`/v1/agents/${encodeURIComponent(companion.agent.id)}/tools`, {
      method: "POST",
      body: {
        expected_tools: ["workspace.write_file"],
        tools: ["workspace.read_file", "workspace.write_file"],
      },
      idempotencyKey: `stale-agent-tools-${randomUUID()}`,
      expectedStatus: 409,
    });

    const freshRun = await beginAgentThreadRun(founder, thread.thread.id);
    expect(freshRun.run.selected_tools.map((tool) => tool.name)).toEqual([
      "workspace.read_file",
    ]);
    expect(record(await askAgent(freshRun, 1, "workspace.read_file", { path: "AGENTS.md" }),
      "workspace after palette reduction").content).toContain(instructions);
    await closeAgentRun(freshRun);
  });

  test("renames only the owner's agent identity without interrupting its live private work", async () => {
    const founder = await browserApprovedCliClient();
    const teammate = await browserApprovedCliClient(reviewerClient);
    const originalName = uniqueName("Private pairing companion");
    const renamedName = uniqueName("Continuity companion");
    const companion = await activateExternalAgent(
      founder,
      originalName,
      ["workspace.read_file"],
      { workspaceInstructions: "# Continuity proof\n\nKeep this private workspace resumable." },
    );
    const occupied = await activateExternalAgent(
      founder,
      uniqueName("Already named companion"),
      ["workspace.read_file"],
    );
    const thread = await startPrivateAgentThread(founder, {
      name: uniqueName("Keep working through a rename"),
      agentId: companion.agent.id,
    });
    const liveRun = await beginAgentThreadRun(founder, thread.thread.id);
    expect(record(await askAgent(liveRun, 1, "workspace.read_file", { path: "AGENTS.md" }),
      "workspace before identity rename").content).toContain("Continuity proof");

    const updateBody = { expected_name: originalName, name: renamedName };
    await founder.json(`/v1/agents/${encodeURIComponent(companion.agent.id)}/name`, {
      method: "POST",
      body: { ...updateBody, replace_identity: true },
      idempotencyKey: `malformed-agent-rename-${randomUUID()}`,
      expectedStatus: 400,
    });
    await teammate.json(`/v1/agents/${encodeURIComponent(companion.agent.id)}/name`, {
      method: "POST",
      body: updateBody,
      idempotencyKey: `foreign-agent-rename-${randomUUID()}`,
      expectedStatus: 404,
    });
    const renamed = await founder.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/name`,
      {
        method: "POST",
        body: updateBody,
        idempotencyKey: `agent-rename-${randomUUID()}`,
      },
    );
    expect(renamed.body).toMatchObject({
      agent: {
        id: companion.agent.id,
        name: renamedName,
        runtime_ref: companion.agent.runtime_ref,
        selected_tools: companion.agent.selected_tools,
        grants: companion.agent.grants,
        workspace_profile: companion.agent.workspace_profile,
      },
      changed: true,
      durable: true,
    });

    const exactRetry = await founder.json(
      `/v1/agents/${encodeURIComponent(companion.agent.id)}/name`,
      {
        method: "POST",
        body: updateBody,
        idempotencyKey: `agent-rename-retry-${randomUUID()}`,
      },
    );
    expect(exactRetry.body).toMatchObject({
      agent: { id: companion.agent.id, name: renamedName },
      changed: false,
      durable: true,
    });
    await founder.json(`/v1/agents/${encodeURIComponent(companion.agent.id)}/name`, {
      method: "POST",
      body: { expected_name: originalName, name: uniqueName("Stale rename") },
      idempotencyKey: `stale-agent-rename-${randomUUID()}`,
      expectedStatus: 409,
    });
    await founder.json(`/v1/agents/${encodeURIComponent(companion.agent.id)}/name`, {
      method: "POST",
      body: { expected_name: renamedName, name: occupied.agent.name },
      idempotencyKey: `occupied-agent-rename-${randomUUID()}`,
      expectedStatus: 409,
    });

    expect(record(await askAgent(liveRun, 2, "workspace.read_file", { path: "AGENTS.md" }),
      "workspace after identity rename").content).toContain("Continuity proof");
    const fetched = await founder.json(`/v1/agents/${encodeURIComponent(companion.agent.id)}`);
    expect(fetched.body).toMatchObject({ agent: { id: companion.agent.id, name: renamedName } });
    const listed = await founder.json("/v1/agents?limit=100");
    expect(array(listed.body.items, "owner's named-agent roster")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: companion.agent.id, name: renamedName })]),
    );
    await closeAgentRun(liveRun);
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
