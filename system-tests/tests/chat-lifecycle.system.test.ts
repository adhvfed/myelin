import { randomUUID } from "node:crypto";

import { describe, expect, test } from "vitest";

import type { SystemTestClient } from "../src/client.js";
import {
  browserApprovedCliClient,
  reviewerClient,
  systemClient,
  uniqueName,
} from "../src/context.js";
import { eventually } from "../src/eventually.js";
import { GitProject } from "../src/git-project.js";
import {
  listPrivateAgentThreads,
  startPrivateAgentThread,
} from "../src/journeys/agent-threads.js";
import { activateExternalAgent } from "../src/journeys/agents.js";
import { Conversation } from "../src/journeys/chat.js";
import { findInboxItem } from "../src/journeys/inbox.js";
import { awaitActiveIssue } from "../src/journeys/issues.js";
import { proposeChange } from "../src/journeys/pull-requests.js";
import { awaitBacklink } from "../src/journeys/refs.js";
import { array, record, string } from "../src/json.js";
import { systemTestConfig } from "../src/config.js";

async function createKnowledgePage(title: string, visibility: "private" | "team") {
  const created = await systemClient.json("/v1/knowledge/pages", {
    method: "POST",
    body: { title, template: "blank", visibility },
    idempotencyKey: `knowledge-card-${randomUUID()}`,
    expectedStatus: 201,
  });
  const page = record(created.body.page, `${visibility} Knowledge page`);
  return {
    ref: string(page.ref, `${visibility} Knowledge page reference`),
    title: string(page.title, `${visibility} Knowledge page title`),
  };
}

async function createPrivatePullRequest(
  owner: SystemTestClient,
  label: string,
) {
  const coordinate = label.replaceAll(" ", "-");
  const repository = new GitProject(uniqueName(`${coordinate}-repository`), owner);
  await repository.create();
  await repository.writeFile("main", "README.md", `# ${repository.slug}\n`);
  const title = uniqueName(`${label} pull request`);
  const commitTitle = uniqueName(`${label} implementation`);
  const branch = uniqueName(`${coordinate}-branch`);
  const opened = await proposeChange(owner, repository, {
    branch,
    path: "plan.md",
    contents: `# ${title}\n`,
    title,
    commitMessage: commitTitle,
  });
  return {
    repositoryRef: `myelin://${systemTestConfig.tenant}/git/repo/${repository.slug}`,
    repositoryTitle: repository.slug,
    pullRequestRef: string(opened.pullRequest.ref, `${label} pull request reference`),
    pullRequestTitle: title,
    pullRequestState: string(opened.pullRequest.pr_state, `${label} pull request state`),
    commitRef: `myelin://${systemTestConfig.tenant}/git/commit/${repository.slug}:${opened.headOid}`,
    commitTitle,
    branchRef: `myelin://${systemTestConfig.tenant}/git/ref/${repository.slug}:${encodeSubjectComponent(`refs/heads/${branch}`)}`,
    branchTitle: `${repository.slug} · ${branch}`,
  };
}

function encodeSubjectComponent(value: string): string {
  return [...new TextEncoder().encode(value)]
    .map((byte) =>
      (byte >= 48 && byte <= 57) || (byte >= 65 && byte <= 90) ||
          (byte >= 97 && byte <= 122) || byte === 45 || byte === 95
        ? String.fromCharCode(byte)
        : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`
    )
    .join("");
}

async function createPrivateConversation(owner: SystemTestClient, label: string) {
  const project = await owner.json("/v1/projects", {
    method: "POST",
    body: {
      name: uniqueName(`${label} private project`),
      issue_prefix: `T${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`,
    },
    idempotencyKey: `private-conversation-project-${randomUUID()}`,
    expectedStatus: 201,
  });
  const projectId = string(
    record(project.body.project, `${label} private project`).id,
    `${label} private project id`,
  );
  const topic = uniqueName(`${label} private topic`);
  const conversation = await Conversation.open(owner, {
    projectId,
    channel: uniqueName(`${label}-private`),
    topic,
  });
  return {
    ref: string(conversation.created.ref, `${label} private conversation reference`),
    topic,
  };
}

describe("chat collaboration lifecycle", () => {
  test("requires a private thread instead of turning a public mention into agent work", async () => {
    const founder = await browserApprovedCliClient();
    const agent = await activateExternalAgent(
      founder,
      uniqueName("Public-room helper"),
      ["chat.read_messages", "chat.post"],
    );
    const room = await Conversation.open(founder, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("public-agent-mentions"),
      topic: "A mention is conversation, not permission to spend or provision",
    });

    const message = "\uFFFC please keep an eye on this discussion.";
    const rejectedMention = await founder.json(
      `/v1/chat/conversations/${encodeURIComponent(room.id)}/messages`,
      {
        method: "POST",
        body: {
          content: message,
          nodes: [{ kind: "mention", principal_id: agent.agent.principal_id }],
        },
        idempotencyKey: `public-agent-mention-${randomUUID()}`,
        expectedStatus: 400,
      },
    );
    expect(rejectedMention.body).toMatchObject({
      error: {
        code: "bad_request",
        message: "Chat mention recipient must be an active member of this conversation",
      },
    });
    expect(await room.messages(founder)).toEqual([]);
    expect(
      (await listPrivateAgentThreads(founder))
        .filter((thread) => thread.agent_id === agent.agent.id),
      "a public mention must not silently provision a private thread or workspace",
    ).toEqual([]);
  });

  test("turns a teammate's words into one durable nudge without leaking private rooms", async () => {
    const privateProject = await systemClient.json("/v1/projects", {
      method: "POST",
      body: {
        name: uniqueName("Private mention project"),
        issue_prefix: `M${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`,
      },
      idempotencyKey: `private-mention-project-${randomUUID()}`,
      expectedStatus: 201,
    });
    const privateProjectId = string(
      record(privateProject.body.project, "private mention project").id,
      "private mention project id",
    );
    const privateRoom = await Conversation.open(systemClient, {
      projectId: privateProjectId,
      channel: uniqueName("private-mentions"),
      topic: "Keep the room and its notifications private",
    });
    const privateProbe = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(privateRoom.id)}/messages`,
      {
        method: "POST",
        body: {
          content: "\uFFFC should never learn that this room exists.",
          nodes: [{ kind: "mention", principal_id: systemTestConfig.reviewerPrincipal }],
        },
        idempotencyKey: `private-mention-${randomUUID()}`,
        expectedStatus: 400,
      },
    );
    expect(privateProbe.body).toMatchObject({ error: { code: "bad_request" } });
    expect(await privateRoom.messages(systemClient)).toEqual([]);

    const room = await Conversation.open(systemClient, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("release-mentions"),
      topic: "Ask a teammate without configuring Slack",
    });
    const message = "\uFFFC please review the release while the context is fresh.";
    const retryKey = `chat-mention-${randomUUID()}`;
    const firstMessageId = await room.post(reviewerClient, message, {
      nodes: [{ kind: "mention", principal_id: systemTestConfig.principal }],
      idempotencyKey: retryKey,
    });
    const replayedMessageId = await room.post(reviewerClient, message, {
      nodes: [{ kind: "mention", principal_id: systemTestConfig.principal }],
      idempotencyKey: retryKey,
    });
    expect(replayedMessageId).toBe(firstMessageId);

    const [storedMention] = (await room.messages(systemClient)).filter(
      (item) => item.id === firstMessageId,
    );
    expect(storedMention).toMatchObject({
      content: message,
      is_you: false,
      nodes: [{ kind: "mention", principal_id: systemTestConfig.principal }],
    });

    const messageRef =
      `myelin://${systemTestConfig.tenant}/chat/message/${firstMessageId}` +
      `#message-${firstMessageId}`;
    const notification = await eventually(
      () => findInboxItem(systemClient, messageRef),
      { description: "the real Chat mention to reach the mentioned teammate's durable inbox" },
    );
    expect(notification).toMatchObject({
      subject: messageRef,
      subsystem: "chat",
      reason: "mentioned",
      class: "direct",
      coalesce_count: 1,
      state: "unread",
    });
    expect(await findInboxItem(reviewerClient, messageRef)).toBeUndefined();
  });

  test("lets project collaborators talk while private project rooms stay private", async () => {
    const channel = uniqueName("system-chat");
    const topic = "Coordinate the externally tested release";
    const projectId = systemTestConfig.issues.projectId;
    const conversationRetryKey = `conversation-${randomUUID()}`;
    const created = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { project_id: projectId, channel, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 201,
    });
    const conversation = record(created.body.conversation, "created conversation");
    const conversationId = string(conversation.id, "conversation id");
    expect(created.body).toMatchObject({ durable: true });
    expect(conversation).toMatchObject({
      ref: `myelin://${systemTestConfig.tenant}/chat/channel/${conversationId}`,
      project_id: projectId,
      channel,
      topic,
    });

    const retriedCreate = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { project_id: projectId, channel, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 200,
    });
    expect(retriedCreate.body).toMatchObject({
      durable: true,
      conversation: { id: conversationId, project_id: projectId, channel, topic },
    });

    const conflictingRetry = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: { project_id: projectId, channel: `${channel}-different`, topic },
      idempotencyKey: conversationRetryKey,
      expectedStatus: 409,
    });
    expect(conflictingRetry.body).toMatchObject({ error: { code: "conflict" } });

    const listed = await reviewerClient.json("/v1/chat/conversations?limit=100");
    expect(array(listed.body.items, "conversation list items")).toEqual(
      expect.arrayContaining([expect.objectContaining({ id: conversationId, channel })]),
    );

    const privateProject = await systemClient.json("/v1/projects", {
      method: "POST",
      body: {
        name: uniqueName("Private project room"),
        issue_prefix: `C${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`,
      },
      idempotencyKey: `private-chat-project-${randomUUID()}`,
      expectedStatus: 201,
    });
    const privateProjectId = string(
      record(privateProject.body.project, "private Chat project").id,
      "private Chat project id",
    );
    const privateRoom = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: {
        project_id: privateProjectId,
        channel,
        topic,
      },
      idempotencyKey: `private-chat-room-${randomUUID()}`,
      expectedStatus: 201,
    });
    const privateRoomId = string(
      record(privateRoom.body.conversation, "private project conversation").id,
      "private project conversation id",
    );
    const foundersRooms = await systemClient.json("/v1/chat/conversations?limit=100");
    expect(array(foundersRooms.body.items, "founder's project conversations")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: conversationId, project_id: projectId, channel, topic }),
        expect.objectContaining({ id: privateRoomId, project_id: privateProjectId, channel, topic }),
      ]),
    );
    const peerRooms = await reviewerClient.json("/v1/chat/conversations?limit=100");
    expect(array(peerRooms.body.items, "peer's project conversations")).not.toEqual(
      expect.arrayContaining([expect.objectContaining({ id: privateRoomId })]),
    );
    const absentPrivateHistory = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(privateRoomId)}/messages?limit=10`,
      { expectedStatus: 404 },
    );
    expect(absentPrivateHistory.body).toMatchObject({ error: { code: "not_found" } });
    const absentPrivatePost = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(privateRoomId)}/messages`,
      {
        method: "POST",
        body: { content: "I should not be able to discover this room." },
        idempotencyKey: `private-chat-probe-${randomUUID()}`,
        expectedStatus: 404,
      },
    );
    expect(absentPrivatePost.body).toMatchObject({ error: { code: "not_found" } });

    const sharedWork = await awaitActiveIssue(systemClient, uniqueName("Shared work with private context"));
    const sharedWorkRef = string(sharedWork.ref, "shared work reference");
    const privateContext = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(privateRoomId)}/messages`,
      {
        method: "POST",
        body: {
          content: "The private room may discuss shared work without revealing itself: ￼",
          references: [sharedWorkRef],
        },
        idempotencyKey: `private-chat-context-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    const privateMessageRef = `myelin://${systemTestConfig.tenant}/chat/message/${string(
      privateContext.body.message_id,
      "private context message id",
    )}`;
    await awaitBacklink(systemClient, sharedWorkRef, privateMessageRef, "links");

    const peerBacklinks = await reviewerClient.json(
      `/v1/refs/backlinks?ref=${encodeURIComponent(sharedWorkRef)}`,
    );
    expect(
      array(peerBacklinks.body.items, "peer-visible backlinks")
        .map((item) => record(item, "peer-visible backlink"))
        .some((item) => item.root_ref === privateMessageRef),
    ).toBe(false);

    const firstRetryKey = `author-${randomUUID()}`;
    const first = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The lifecycle suite is running against the real backend." },
        idempotencyKey: firstRetryKey,
        expectedStatus: 201,
      },
    );
    expect(first.body).toMatchObject({ durable: true });
    const firstMessageId = string(first.body.message_id, "first message id");
    expect(firstMessageId).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);

    const replay = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The lifecycle suite is running against the real backend." },
        idempotencyKey: firstRetryKey,
        expectedStatus: 201,
      },
    );
    expect(replay.body).toEqual(first.body);

    const reviewerMessage = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "Confirmed from a second principal." },
        idempotencyKey: `reviewer-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    const reviewerMessageId = string(reviewerMessage.body.message_id, "reviewer message id");

    const finalMessage = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: { content: "The paged history is consistent." },
        idempotencyKey: `author-final-${randomUUID()}`,
        expectedStatus: 201,
      },
    );
    const finalMessageId = string(finalMessage.body.message_id, "final message id");

    const recent = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=2`,
    );
    const recentItems = array(recent.body.items, "recent conversation messages").map((item) =>
      record(item, "recent conversation message"),
    );
    expect(recentItems.map((item) => item.id)).toEqual([reviewerMessageId, finalMessageId]);
    const recentPage = record(recent.body.page, "recent message page");
    expect(recentPage).toMatchObject({ limit: 2, next_cursor: reviewerMessageId });

    const older = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=2&before=${encodeURIComponent(reviewerMessageId)}`,
    );
    expect(array(older.body.items, "older conversation messages")).toEqual([
      expect.objectContaining({ id: firstMessageId }),
    ]);
    expect(older.body).toMatchObject({ page: { limit: 2, next_cursor: null } });

    const messages = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=100`,
    );
    expect(messages.body).toMatchObject({ conversation: { id: conversationId, channel } });
    expect(array(messages.body.items, "conversation messages")).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          content: "The lifecycle suite is running against the real backend.",
          is_you: true,
          state: "active",
        }),
        expect.objectContaining({
          content: "Confirmed from a second principal.",
          is_you: false,
          state: "active",
        }),
        expect.objectContaining({
          content: "The paged history is consistent.",
          is_you: true,
          state: "active",
        }),
      ]),
    );
  });

  test("shows useful reference cards without copying private work into chat", async () => {
    const sharedTitle = uniqueName("Coordinate the referenced rollout");
    const sharedIssue = await awaitActiveIssue(systemClient, sharedTitle);
    const sharedIssueRef = string(sharedIssue.ref, "shared issue reference");
    const sharedState = string(sharedIssue.state, "shared issue state");
    expect(sharedIssueRef).toMatch(/^myelin:\/\/[^/]+\/issue\/issue\/MYL-\d+$/);

    const privateProject = await systemClient.json("/v1/projects", {
      method: "POST",
      body: {
        name: uniqueName("Private reference cards"),
        issue_prefix: `R${randomUUID().replaceAll("-", "").slice(0, 7).toUpperCase()}`,
      },
      idempotencyKey: `private-reference-project-${randomUUID()}`,
      expectedStatus: 201,
    });
    const privateProjectId = string(
      record(privateProject.body.project, "private reference project").id,
      "private reference project id",
    );
    const privateTitle = uniqueName("Leadership compensation review");
    const privateIssue = await awaitActiveIssue(systemClient, privateTitle, {
      projectId: privateProjectId,
    });
    const privateIssueRef = string(privateIssue.ref, "private issue reference");

    const created = await systemClient.json("/v1/chat/conversations", {
      method: "POST",
      body: {
        project_id: systemTestConfig.issues.projectId,
        channel: uniqueName("system-chat-ref"),
        topic: "Share work without copying it",
      },
      idempotencyKey: `conversation-ref-${randomUUID()}`,
      expectedStatus: 201,
    });
    const conversation = record(created.body.conversation, "reference conversation");
    const conversationId = string(conversation.id, "reference conversation id");

    await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages`,
      {
        method: "POST",
        body: {
          content: "Follow \uFFFC, but keep \uFFFC private.",
          references: [sharedIssueRef, privateIssueRef],
        },
        idempotencyKey: `message-ref-${randomUUID()}`,
        expectedStatus: 201,
      },
    );

    const history = await reviewerClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=10`,
    );
    expect(array(history.body.items, "teammate's referenced conversation messages")).toEqual([
      expect.objectContaining({
        content: "Follow \uFFFC, but keep \uFFFC private.",
        nodes: [
          {
            kind: "artifact_ref",
            ref: sharedIssueRef,
            card: {
              kind: "projection",
              title: sharedTitle,
              state: sharedState,
              icon: "issue",
              render_hint: "issue",
              sub_anchor: null,
              flag: null,
            },
          },
          {
            kind: "artifact_ref",
            ref: privateIssueRef,
            card: { kind: "tombstone" },
          },
        ],
      }),
    ]);
    expect(JSON.stringify(history.body)).not.toContain(privateTitle);

    const authorsHistory = await systemClient.json(
      `/v1/chat/conversations/${encodeURIComponent(conversationId)}/messages?limit=10`,
    );
    expect(array(authorsHistory.body.items, "author's referenced conversation messages"))
      .toEqual([
        expect.objectContaining({
          nodes: [
            expect.objectContaining({
              ref: sharedIssueRef,
              card: expect.objectContaining({ kind: "projection", title: sharedTitle }),
            }),
            expect.objectContaining({
              ref: privateIssueRef,
              card: expect.objectContaining({ kind: "projection", title: privateTitle }),
            }),
          ],
        }),
      ]);
  });

  test("lets each engineer recognize their private Git work without exposing the other's", async () => {
    const foundersWork = await createPrivatePullRequest(systemClient, "founder rollout");
    const reviewersWork = await createPrivatePullRequest(reviewerClient, "reviewer rollout");
    const room = await Conversation.open(systemClient, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("private-git-cards"),
      topic: "Coordinate changes without broadening repository access",
    });
    await room.post(
      systemClient,
      "Compare my repository \uFFFC, change \uFFFC, commit \uFFFC, and branch \uFFFC with yours \uFFFC, \uFFFC, \uFFFC, and \uFFFC.",
      {
        references: [
          foundersWork.repositoryRef,
          foundersWork.pullRequestRef,
          foundersWork.commitRef,
          foundersWork.branchRef,
          reviewersWork.repositoryRef,
          reviewersWork.pullRequestRef,
          reviewersWork.commitRef,
          reviewersWork.branchRef,
        ],
      },
    );

    const foundersHistory = await room.messages(systemClient);
    expect(foundersHistory).toEqual([
      expect.objectContaining({
        nodes: [
          expect.objectContaining({
            ref: foundersWork.repositoryRef,
            card: expect.objectContaining({
              kind: "projection",
              title: foundersWork.repositoryTitle,
              state: "active",
              icon: "git",
              render_hint: "git_repository",
            }),
          }),
          expect.objectContaining({
            ref: foundersWork.pullRequestRef,
            card: expect.objectContaining({
              kind: "projection",
              title: foundersWork.pullRequestTitle,
              state: foundersWork.pullRequestState,
              icon: "pull_request",
              render_hint: "git_pull_request",
            }),
          }),
          expect.objectContaining({
            ref: foundersWork.commitRef,
            card: expect.objectContaining({
              kind: "projection",
              title: foundersWork.commitTitle,
              state: "committed",
              icon: "commit",
              render_hint: "git_commit",
            }),
          }),
          expect.objectContaining({
            ref: foundersWork.branchRef,
            card: expect.objectContaining({
              kind: "projection",
              title: foundersWork.branchTitle,
              state: "branch",
              icon: "branch",
              render_hint: "git_ref",
            }),
          }),
          expect.objectContaining({
            ref: reviewersWork.repositoryRef,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: reviewersWork.pullRequestRef,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: reviewersWork.commitRef,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: reviewersWork.branchRef,
            card: { kind: "tombstone" },
          }),
        ],
      }),
    ]);
    expect(JSON.stringify(foundersHistory)).not.toContain(reviewersWork.pullRequestTitle);
    expect(JSON.stringify(foundersHistory)).not.toContain(reviewersWork.commitTitle);
    expect(JSON.stringify(foundersHistory)).not.toContain(reviewersWork.branchTitle);

    const reviewersHistory = await room.messages(reviewerClient);
    expect(reviewersHistory).toEqual([
      expect.objectContaining({
        nodes: [
          expect.objectContaining({
            ref: foundersWork.repositoryRef,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: foundersWork.pullRequestRef,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: foundersWork.commitRef,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: foundersWork.branchRef,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: reviewersWork.repositoryRef,
            card: expect.objectContaining({
              kind: "projection",
              title: reviewersWork.repositoryTitle,
              state: "active",
              icon: "git",
              render_hint: "git_repository",
            }),
          }),
          expect.objectContaining({
            ref: reviewersWork.pullRequestRef,
            card: expect.objectContaining({
              kind: "projection",
              title: reviewersWork.pullRequestTitle,
              state: reviewersWork.pullRequestState,
              icon: "pull_request",
              render_hint: "git_pull_request",
            }),
          }),
          expect.objectContaining({
            ref: reviewersWork.commitRef,
            card: expect.objectContaining({
              kind: "projection",
              title: reviewersWork.commitTitle,
              state: "committed",
              icon: "commit",
              render_hint: "git_commit",
            }),
          }),
          expect.objectContaining({
            ref: reviewersWork.branchRef,
            card: expect.objectContaining({
              kind: "projection",
              title: reviewersWork.branchTitle,
              state: "branch",
              icon: "branch",
              render_hint: "git_ref",
            }),
          }),
        ],
      }),
    ]);
    expect(JSON.stringify(reviewersHistory)).not.toContain(foundersWork.pullRequestTitle);
    expect(JSON.stringify(reviewersHistory)).not.toContain(foundersWork.commitTitle);
    expect(JSON.stringify(reviewersHistory)).not.toContain(foundersWork.branchTitle);
  });

  test("keeps a shared runbook legible while a private notebook stays nameless", async () => {
    const sharedPage = await createKnowledgePage(
      uniqueName("Release handover runbook"),
      "team",
    );
    const privatePage = await createKnowledgePage(
      uniqueName("Founder succession notes"),
      "private",
    );
    const room = await Conversation.open(systemClient, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("knowledge-cards"),
      topic: "Bring living documentation into the work without copying it",
    });
    await room.post(
      systemClient,
      "Read \uFFFC before release; leave \uFFFC with its owner.",
      { references: [sharedPage.ref, privatePage.ref] },
    );

    const teammatesHistory = await room.messages(reviewerClient);
    expect(teammatesHistory).toEqual([
      expect.objectContaining({
        nodes: [
          {
            kind: "artifact_ref",
            ref: sharedPage.ref,
            card: {
              kind: "projection",
              title: sharedPage.title,
              state: "active",
              icon: "knowledge",
              render_hint: "knowledge_page",
              sub_anchor: null,
              flag: null,
            },
          },
          {
            kind: "artifact_ref",
            ref: privatePage.ref,
            card: { kind: "tombstone" },
          },
        ],
      }),
    ]);
    expect(JSON.stringify(teammatesHistory)).not.toContain(privatePage.title);

    const authorsHistory = await room.messages(systemClient);
    expect(authorsHistory).toEqual([
      expect.objectContaining({
        nodes: [
          expect.objectContaining({
            ref: sharedPage.ref,
            card: expect.objectContaining({ kind: "projection", title: sharedPage.title }),
          }),
          expect.objectContaining({
            ref: privatePage.ref,
            card: expect.objectContaining({ kind: "projection", title: privatePage.title }),
          }),
        ],
      }),
    ]);
  });

  test("lets each engineer carry a private conversation without revealing the other's topic", async () => {
    const foundersTopic = await createPrivateConversation(systemClient, "founder planning");
    const reviewersTopic = await createPrivateConversation(reviewerClient, "reviewer planning");
    const room = await Conversation.open(systemClient, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("private-conversation-cards"),
      topic: "Resume private context without moving it into the public room",
    });
    await room.post(
      systemClient,
      "I will continue in ￼; keep your notes in ￼.",
      { references: [foundersTopic.ref, reviewersTopic.ref] },
    );

    const foundersHistory = await room.messages(systemClient);
    expect(foundersHistory).toEqual([
      expect.objectContaining({
        nodes: [
          expect.objectContaining({
            ref: foundersTopic.ref,
            card: expect.objectContaining({
              kind: "projection",
              title: foundersTopic.topic,
              state: "active",
              icon: "chat",
              render_hint: "chat_conversation",
            }),
          }),
          expect.objectContaining({
            ref: reviewersTopic.ref,
            card: { kind: "tombstone" },
          }),
        ],
      }),
    ]);
    expect(JSON.stringify(foundersHistory)).not.toContain(reviewersTopic.topic);

    const reviewersHistory = await room.messages(reviewerClient);
    expect(reviewersHistory).toEqual([
      expect.objectContaining({
        nodes: [
          expect.objectContaining({
            ref: foundersTopic.ref,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: reviewersTopic.ref,
            card: expect.objectContaining({
              kind: "projection",
              title: reviewersTopic.topic,
              state: "active",
              icon: "chat",
              render_hint: "chat_conversation",
            }),
          }),
        ],
      }),
    ]);
    expect(JSON.stringify(reviewersHistory)).not.toContain(foundersTopic.topic);
  });

  test("lets each engineer resume named private agent work without exposing the other workspace", async () => {
    const founder = await browserApprovedCliClient();
    const reviewer = await browserApprovedCliClient(reviewerClient);
    const foundersAgent = await activateExternalAgent(
      founder,
      uniqueName("Founder's private-work companion"),
      ["chat.read_messages"],
    );
    const reviewersAgent = await activateExternalAgent(
      reviewer,
      uniqueName("Reviewer's private-work companion"),
      ["chat.read_messages"],
    );
    const foundersThread = (await startPrivateAgentThread(founder, {
      name: uniqueName("Founder investigates checkout contention"),
      agentId: foundersAgent.agent.id,
      retentionDays: 3,
    })).thread;
    const reviewersThread = (await startPrivateAgentThread(reviewer, {
      name: uniqueName("Reviewer investigates rollout safety"),
      agentId: reviewersAgent.agent.id,
      retentionDays: 3,
    })).thread;
    const room = await Conversation.open(founder, {
      projectId: systemTestConfig.issues.projectId,
      channel: uniqueName("private-agent-thread-cards"),
      topic: "Carry private work by reference, never by copied context",
    });
    await room.post(
      founder,
      "I will resume ￼; your private investigation remains in ￼.",
      { references: [foundersThread.ref, reviewersThread.ref] },
    );

    const foundersHistory = await room.messages(founder);
    expect(foundersHistory).toEqual([
      expect.objectContaining({
        nodes: [
          expect.objectContaining({
            ref: foundersThread.ref,
            card: expect.objectContaining({
              kind: "projection",
              title: foundersThread.name,
              state: "ready",
              icon: "agent",
              render_hint: "agent_thread",
            }),
          }),
          expect.objectContaining({
            ref: reviewersThread.ref,
            card: { kind: "tombstone" },
          }),
        ],
      }),
    ]);
    expect(JSON.stringify(foundersHistory)).not.toContain(reviewersThread.name);
    expect(JSON.stringify(foundersHistory)).not.toContain(reviewersThread.workspace.id);

    const reviewersHistory = await room.messages(reviewer);
    expect(reviewersHistory).toEqual([
      expect.objectContaining({
        nodes: [
          expect.objectContaining({
            ref: foundersThread.ref,
            card: { kind: "tombstone" },
          }),
          expect.objectContaining({
            ref: reviewersThread.ref,
            card: expect.objectContaining({
              kind: "projection",
              title: reviewersThread.name,
              state: "ready",
              icon: "agent",
              render_hint: "agent_thread",
            }),
          }),
        ],
      }),
    ]);
    expect(JSON.stringify(reviewersHistory)).not.toContain(foundersThread.name);
    expect(JSON.stringify(reviewersHistory)).not.toContain(foundersThread.workspace.id);
  });
});
